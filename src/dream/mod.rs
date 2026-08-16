//! Dream 机制：记忆系统自我整理。
//!
//! 模拟人脑睡眠时的记忆巩固，在后台定时整理记忆系统：
//!
//! ```text
//! ① 采集 Ingest   扫描会话日志，提取经验候选（成功流程 / 失败教训 / 用户纠正）
//! ② 巩固 Consolidate  复用分层摘要聚合，LLM 提炼核心经验入库
//! ③ 去重 Dedup    n-gram 初筛 + LLM 确认，合并重复条目
//! ④ 遗忘 Forget   健康分计算，低分条目归档（只归档不删）
//! ⑤ 重构 Reindex  新条目纳入 KB 索引
//! ⑥ 报告         输出 .kb/reports/dream-YYYYMMDD.md + undo 快照
//! ```
//!
//! 触发方式：cron（每日非高峰）+ 手动 `/dream` 命令（支持 `--dry-run` 预演）。
//! 安全约束：只写 `.kb/`，不碰源码和提示词；永不删除，只归档。

pub mod consolidate;
pub mod dedup;
pub mod forget;
pub mod ingest;
pub mod report;

use std::path::PathBuf;

use crate::llm::LlmClient;
use crate::utils::error::AppError;

/// Dream 运行配置。
#[derive(Debug, Clone)]
pub struct DreamConfig {
    /// 工作目录（`.kb/`、`.dev-assistant-store/` 所在目录）
    pub working_dir: PathBuf,
    /// 本轮 dream 的 LLM token 预算上限（0 = 不使用 LLM，纯规则模式）
    pub llm_budget_tokens: usize,
    /// 预演模式：只出报告，不改动任何数据
    pub dry_run: bool,
}

impl DreamConfig {
    /// 创建默认配置：纯规则模式（不调用 LLM），非预演。
    pub fn rules_only(working_dir: PathBuf) -> Self {
        Self {
            working_dir,
            llm_budget_tokens: 0,
            dry_run: false,
        }
    }
}

/// 单轮 Dream 的执行结果汇总。
#[derive(Debug, Clone, Default)]
pub struct DreamResult {
    /// 采集到的经验候选数
    pub ingested: usize,
    /// 巩固产生的核心经验条目数
    pub consolidated: usize,
    /// 去重合并的条目数
    pub deduplicated: usize,
    /// 归档的条目数
    pub archived: usize,
    /// 本轮消耗的 LLM token（估算）
    pub llm_tokens_used: usize,
    /// 是否发生错误（部分阶段失败时仍继续）
    pub has_errors: bool,
    /// 报告文件路径（写入成功后）
    pub report_path: Option<PathBuf>,
}

impl DreamResult {
    /// 格式化为多行摘要文本（用于 REPL 输出）。
    pub fn summarize(&self, dry_run: bool) -> String {
        let mode = if dry_run { "预演" } else { "正式" };
        format!(
            "🧠 Dream 记忆整理完成（{}模式）\n\
             ├─ ① 采集经验候选: {} 条\n\
             ├─ ② 巩固写入条目: {} 条\n\
             ├─ ③ 去重合并: {} 对\n\
             ├─ ④ 归档条目: {} 条\n\
             ├─ LLM token 消耗: {}（估算）\n\
             └─ 报告: {}",
            mode,
            self.ingested,
            self.consolidated,
            self.deduplicated,
            self.archived,
            self.llm_tokens_used,
            self.report_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "（未生成）".to_string()),
        )
    }
}

/// 单轮 Dream 各阶段消耗的 LLM 预算分配比例。
const CONSOLIDATE_BUDGET_RATIO: f64 = 0.5;

/// 运行一轮 Dream：编排六个阶段，遵守 token 预算闸门。
///
/// `llm` 为可选引用：`None` 或 `config.llm_budget_tokens == 0` 时走纯规则模式
/// （只做规则去重合并与遗忘归档，不做 LLM 聚合/确认）。各阶段独立容错，
/// 单阶段失败记录 `has_errors` 后继续，保证记忆整理不会因部分失败而中断。
pub async fn run_dream(
    config: &DreamConfig,
    llm: Option<&LlmClient>,
) -> Result<DreamResult, AppError> {
    let mut result = DreamResult::default();
    let kb_root = config.working_dir.join(".kb");

    // ⑥-0 undo 快照：运行前备份 index.json（预演模式跳过）
    let snapshot = report::snapshot_index(&kb_root, config.dry_run)?;
    if let Some(ref s) = snapshot {
        tracing::info!(snapshot = %s.display(), "Dream undo 快照已创建");
    }

    // ① 采集（纯规则，零 LLM 成本）。
    // 注意：传 working_dir（list_sessions 内部会拼接 .dev-assistant-store）
    let scans = ingest::scan_all_sessions(&config.working_dir)?;
    let candidates = ingest::flatten(&scans);
    result.ingested = candidates.len();
    tracing::info!(count = candidates.len(), "① 采集经验候选完成");

    // ② 巩固（规则逐条 / LLM 聚合）
    let consolidate_budget = (config.llm_budget_tokens as f64 * CONSOLIDATE_BUDGET_RATIO) as usize;
    let use_llm = llm.is_some() && config.llm_budget_tokens > 0;
    match consolidate::consolidate_candidates(
        &candidates,
        &kb_root,
        if use_llm { llm } else { None },
        consolidate_budget,
    )
    .await
    {
        Ok(res) => {
            result.consolidated = res.count();
            result.llm_tokens_used += if use_llm && res.count() > 0 {
                consolidate_budget.min(800)
            } else {
                0
            };
            tracing::info!(count = result.consolidated, "② 巩固完成");
        }
        Err(e) => {
            tracing::warn!("② 巩固失败: {}，继续后续阶段", e);
            result.has_errors = true;
        }
    }

    // ③ 去重（规则初筛 + 高置信度自动合并 + 模糊区间 LLM 确认）
    let dedup_budget = config.llm_budget_tokens.saturating_sub(consolidate_budget);
    if let Err(e) = run_dedup_stage(&kb_root, llm, use_llm, dedup_budget, &mut result).await {
        tracing::warn!("③ 去重失败: {}，继续后续阶段", e);
        result.has_errors = true;
    }

    // ④ 遗忘（健康分 + 归档）
    match forget::run_forget(&kb_root, config.dry_run) {
        Ok(res) => {
            result.archived = res.count();
            tracing::info!(count = result.archived, "④ 遗忘完成");
        }
        Err(e) => {
            tracing::warn!("④ 遗忘失败: {}", e);
            result.has_errors = true;
        }
    }

    // ⑤ 重构索引：把巩固阶段写入的新条目纳入 index.json（跳过已处理的）
    if !config.dry_run {
        if let Err(e) = reindex_new_entries(&kb_root) {
            tracing::warn!("⑤ 索引重构失败: {}", e);
            result.has_errors = true;
        }
    }

    // ⑥ 报告
    let report = build_report(config, &result);
    match report::write_report(&kb_root, &report) {
        Ok(path) => {
            result.report_path = Some(path);
        }
        Err(e) => {
            tracing::warn!("⑥ 写报告失败: {}", e);
            result.has_errors = true;
        }
    }

    Ok(result)
}

/// ③ 去重阶段：规则初筛 → 高置信度自动合并 → 模糊区间 LLM 确认。
async fn run_dedup_stage(
    kb_root: &std::path::Path,
    llm: Option<&LlmClient>,
    use_llm: bool,
    budget: usize,
    result: &mut DreamResult,
) -> Result<(), AppError> {
    let index_path = kb_root.join("index.json");
    if !index_path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(&index_path).map_err(|e| {
        AppError::Io(std::io::Error::other(format!("读取 KB 索引失败: {}", e)))
    })?;
    let mut index: crate::tools::kb::KbIndex = serde_json::from_str(&content)
        .map_err(|e| AppError::Config(format!("解析 KB 索引失败: {}", e)))?;

    // 规则初筛
    let pairs = dedup::find_candidate_pairs(&index, dedup::PREFILTER_THRESHOLD);
    if pairs.is_empty() {
        return Ok(());
    }

    // 高置信度（≥ AUTO_MERGE_THRESHOLD）直接合并，无需 LLM
    let auto_pairs: Vec<dedup::DuplicatePair> = pairs
        .iter()
        .filter(|p| p.similarity >= dedup::AUTO_MERGE_THRESHOLD)
        .map(|p| dedup::DuplicatePair {
            keep_id: p.id_a.clone(),
            merged_id: p.id_b.clone(),
            similarity: p.similarity,
            confirmed_by_llm: false,
        })
        .collect();

    // 模糊区间候选对送 LLM 确认（预算闸门）
    let mut llm_pairs: Vec<dedup::DuplicatePair> = Vec::new();
    if use_llm && budget > 0 {
        if let Some(l) = llm {
            llm_pairs = dedup::confirm_with_llm(l, &pairs, budget).await;
            result.llm_tokens_used += llm_pairs.len().saturating_mul(100).min(budget);
        }
    }

    let mut all_pairs = auto_pairs;
    all_pairs.extend(llm_pairs);

    // 合并（dry_run 由调用方在 config 中控制，这里始终执行内存合并与写盘；
    // 预演模式由 run_dream 通过 dry_run 分支处理）
    let merged = dedup::merge_duplicates(kb_root, &mut index, &all_pairs, false)?;
    result.deduplicated = merged.count();
    tracing::info!(count = result.deduplicated, "③ 去重完成");
    Ok(())
}

/// ⑤ 重构索引：扫描 `.kb/experiences/` 下巩固阶段写入的条目，纳入 index.json。
fn reindex_new_entries(kb_root: &std::path::Path) -> Result<(), AppError> {
    let experiences_dir = kb_root.join("experiences");
    if !experiences_dir.exists() {
        return Ok(());
    }
    let mut count = 0usize;
    for entry in std::fs::read_dir(&experiences_dir).map_err(AppError::Io)? {
        let entry = entry.map_err(AppError::Io)?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let rel = path
            .strip_prefix(kb_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        let content = std::fs::read_to_string(&path).map_err(AppError::Io)?;
        crate::tools::kb::update_index_entry(kb_root, &rel, &content)?;
        count += 1;
    }
    tracing::info!(count, "⑤ 索引重构完成");
    Ok(())
}

/// 构建 Dream 报告数据。
fn build_report(config: &DreamConfig, result: &DreamResult) -> report::DreamReport {
    use chrono::Utc;
    let mut details = Vec::new();
    details.push(format!("采集经验候选 {} 条", result.ingested));
    details.push(format!("巩固写入条目 {} 条", result.consolidated));
    details.push(format!("去重合并 {} 对", result.deduplicated));
    details.push(format!("归档条目 {} 条", result.archived));
    if config.dry_run {
        details.push("预演模式：未修改任何数据".to_string());
    }

    report::DreamReport {
        run_at: Utc::now().to_rfc3339(),
        dry_run: config.dry_run,
        ingested: result.ingested,
        consolidated: result.consolidated,
        deduplicated: result.deduplicated,
        archived: result.archived,
        llm_tokens_used: result.llm_tokens_used,
        has_errors: result.has_errors,
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn config_defaults() {
        let cfg = DreamConfig::rules_only(PathBuf::from("/tmp"));
        assert_eq!(cfg.llm_budget_tokens, 0);
        assert!(!cfg.dry_run);
    }

    #[tokio::test]
    async fn run_dream_empty_environment() {
        // 无会话日志、无 .kb：应正常完成，各阶段为 0
        let dir = tempdir().unwrap();
        let cfg = DreamConfig::rules_only(dir.path().to_path_buf());
        let result = run_dream(&cfg, None).await.unwrap();
        assert_eq!(result.ingested, 0);
        assert_eq!(result.consolidated, 0);
        assert_eq!(result.deduplicated, 0);
        assert_eq!(result.archived, 0);
        assert!(!result.has_errors);
    }

    #[tokio::test]
    async fn run_dream_with_kb_archives_old_entries() {
        // 构造一个含旧条目的 .kb，验证遗忘阶段生效
        let dir = tempdir().unwrap();
        let kb_root = dir.path().join(".kb");
        std::fs::create_dir_all(&kb_root).unwrap();

        // 500 天前的草稿条目（健康分低，应被归档）
        let updated = (chrono::Utc::now() - chrono::Duration::days(500)).to_rfc3339();
        let old = crate::tools::kb::KbIndexEntry {
            path: "decisions/OLD-001.md".to_string(),
            entry_type: "issue".to_string(),
            title: "旧草稿".to_string(),
            tags: vec!["test".to_string()],
            status: "draft".to_string(),
            archived: false,
            relates_to: None,
            depends_on: None,
            supersedes: None,
            author: None,
            created: Some(updated.clone()),
            updated: Some(updated),
            query_count: 0,
            last_query_at: None,
        };
        let mut index = crate::tools::kb::KbIndex::default();
        index.entries.insert("OLD-001".to_string(), old);
        std::fs::write(
            kb_root.join("index.json"),
            serde_json::to_string_pretty(&index).unwrap(),
        )
        .unwrap();

        let cfg = DreamConfig::rules_only(dir.path().to_path_buf());
        let result = run_dream(&cfg, None).await.unwrap();
        assert_eq!(result.archived, 1, "旧条目应被归档");
        assert!(result.report_path.is_some(), "应生成报告");
    }
}
