//! 分层摘要系统 (Layered Summary System)。
//!
//! 实现设计文档 `docs/small-context-optimization-design.md` §3.5 的分层摘要：
//!
//! ```text
//! 层级 0: 原始对话（完整保留在上下文中）
//!   → 最近 N 轮完整对话
//! 层级 1: 轮次摘要（每轮对话的摘要）
//!   → 存储: .kb/summaries/{session_id}/round-{n}.md  (~300 tokens)
//! 层级 2: 阶段摘要（每 5 轮摘要的聚合）
//!   → 存储: .kb/summaries/{session_id}/phase-{n}.md   (~500 tokens)
//! 层级 3: 会话摘要（整个会话的最终摘要）
//!   → 存储: .kb/summaries/{session_id}/final.md       (~1000 tokens)
//! ```
//!
//! 恢复时：从 `final.md` 开始，按需回溯到 `phase-*` 或 `round-*` 级别。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::token_counter::TokenCounter;
use crate::llm::{LlmClient, LlmMessage, LlmResponse};
use crate::utils::error::AppError;

/// 每个阶段聚合的轮次数。
pub const ROUNDS_PER_PHASE: usize = 5;

/// 轮次摘要文件前缀。
const ROUND_PREFIX: &str = "round-";
/// 阶段摘要文件前缀。
const PHASE_PREFIX: &str = "phase-";
/// 会话摘要文件名。
const FINAL_FILE: &str = "final.md";

/// 一条轮次摘要（层级 1）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundSummary {
    /// 轮次编号（从 1 开始）
    pub round: usize,
    /// 摘要内容（Markdown）
    pub content: String,
    /// 估算 token 数
    pub tokens: usize,
}

/// 一条阶段摘要（层级 2）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseSummary {
    /// 阶段编号（从 1 开始）
    pub phase: usize,
    /// 覆盖的轮次范围
    pub round_start: usize,
    pub round_end: usize,
    /// 摘要内容（Markdown）
    pub content: String,
    /// 估算 token 数
    pub tokens: usize,
}

/// 全部层级摘要（恢复时按需回溯使用）。
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // constructed only by SummaryStore::load_all (reserved for checkpoint recovery)
pub struct LayeredSummaries {
    /// 轮次摘要（升序）
    pub rounds: Vec<RoundSummary>,
    /// 阶段摘要（升序）
    pub phases: Vec<PhaseSummary>,
    /// 会话摘要（可选）
    pub final_summary: Option<String>,
}

/// 分层摘要存储。
///
/// 管理 `.kb/summaries/{session_id}/` 目录下的轮次/阶段/会话摘要文件。
pub struct SummaryStore {
    /// 会话 ID
    #[allow(dead_code)] // reserved for future use
    session_id: String,
    /// 摘要根目录（`.kb/summaries/{session_id}`）
    root: PathBuf,
}

impl SummaryStore {
    /// 创建摘要存储。`kb_root` 是 KnowledgeBase 根目录（`.kb/`）。
    ///
    /// 若 `session_id` 为空，使用 `"default"`。
    pub fn new(session_id: &str, kb_root: &Path) -> Self {
        let sid = if session_id.trim().is_empty() {
            "default".to_string()
        } else {
            session_id.to_string()
        };
        let root = kb_root.join("summaries").join(&sid);
        if let Err(e) = std::fs::create_dir_all(&root) {
            tracing::warn!("创建摘要目录失败: {}: {}", root.display(), e);
        }
        Self { session_id: sid, root }
    }

    /// 会话 ID。
    #[allow(dead_code)] // reserved for future use
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 摘要根目录。
    #[allow(dead_code)] // reserved for future use
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 保存一条轮次摘要（层级 1）。
    pub fn save_round(&self, round: usize, content: &str) -> Result<(), AppError> {
        let path = self.root.join(format!("{}{}.md", ROUND_PREFIX, round));
        self.write_summary_file(&path, "round", round, content)
    }

    /// 保存一条阶段摘要（层级 2）。
    pub fn save_phase(
        &self,
        round_start: usize,
        round_end: usize,
        content: &str,
    ) -> Result<(), AppError> {
        let phase = phase_number(round_start);
        let path = self.root.join(format!("{}{}.md", PHASE_PREFIX, phase));
        let frontmatter = format!(
            "---\nid: phase-{}\ntype: phase-summary\nphase: {}\nround_start: {}\nround_end: {}\n---\n",
            phase, phase, round_start, round_end
        );
        std::fs::write(&path, format!("{}\n{}", frontmatter, content)).map_err(AppError::Io)?;
        Ok(())
    }

    /// 保存会话摘要（层级 3，final.md）。
    pub fn save_final(&self, content: &str) -> Result<(), AppError> {
        let path = self.root.join(FINAL_FILE);
        let frontmatter = "---\nid: final\ntype: session-summary\ntitle: 会话摘要\n---\n";
        std::fs::write(&path, format!("{}\n{}", frontmatter, content)).map_err(AppError::Io)?;
        Ok(())
    }

    /// 加载所有轮次摘要（按 round 升序）。
    pub fn load_rounds(&self) -> Result<Vec<RoundSummary>, AppError> {
        let mut rounds: BTreeMap<usize, RoundSummary> = BTreeMap::new();
        for entry in std::fs::read_dir(&self.root).map_err(AppError::Io)? {
            let entry = entry.map_err(AppError::Io)?;
            let fname = entry.file_name().to_string_lossy().to_string();
            if !fname.starts_with(ROUND_PREFIX) || !fname.ends_with(".md") {
                continue;
            }
            let num: usize = fname
                .trim_start_matches(ROUND_PREFIX)
                .trim_end_matches(".md")
                .parse()
                .unwrap_or(0);
            if num == 0 {
                continue;
            }
            let content = std::fs::read_to_string(entry.path()).unwrap_or_default();
            let body = strip_frontmatter(&content);
            rounds.insert(
                num,
                RoundSummary {
                    round: num,
                    content: body.to_string(),
                    tokens: TokenCounter::estimate(body),
                },
            );
        }
        Ok(rounds.into_values().collect())
    }

    /// 加载所有阶段摘要（按 phase 升序）。
    pub fn load_phases(&self) -> Result<Vec<PhaseSummary>, AppError> {
        let mut phases: BTreeMap<usize, PhaseSummary> = BTreeMap::new();
        for entry in std::fs::read_dir(&self.root).map_err(AppError::Io)? {
            let entry = entry.map_err(AppError::Io)?;
            let fname = entry.file_name().to_string_lossy().to_string();
            if !fname.starts_with(PHASE_PREFIX) || !fname.ends_with(".md") {
                continue;
            }
            let num: usize = fname
                .trim_start_matches(PHASE_PREFIX)
                .trim_end_matches(".md")
                .parse()
                .unwrap_or(0);
            if num == 0 {
                continue;
            }
            let content = std::fs::read_to_string(entry.path()).unwrap_or_default();
            phases.insert(num, parse_phase_meta(&content, num));
        }
        Ok(phases.into_values().collect())
    }

    /// 加载会话摘要（若有）。
    #[allow(dead_code)] // reserved for checkpoint recovery
    pub fn load_final(&self) -> Result<Option<String>, AppError> {
        let path = self.root.join(FINAL_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path).map_err(AppError::Io)?;
        Ok(Some(strip_frontmatter(&content).to_string()))
    }

    /// 加载全部层级摘要（用于恢复时按需回溯）。
    #[allow(dead_code)] // reserved for checkpoint recovery
    pub fn load_all(&self) -> Result<LayeredSummaries, AppError> {
        Ok(LayeredSummaries {
            rounds: self.load_rounds()?,
            phases: self.load_phases()?,
            final_summary: self.load_final()?,
        })
    }

    /// 写入带 frontmatter 的摘要文件。
    fn write_summary_file(
        &self,
        path: &Path,
        kind: &str,
        num: usize,
        content: &str,
    ) -> Result<(), AppError> {
        let frontmatter = format!("---\nid: {}-{}\ntype: {}-summary\n---\n", kind, num, kind);
        std::fs::write(path, format!("{}\n{}", frontmatter, content)).map_err(AppError::Io)?;
        Ok(())
    }
}

/// 计算轮次对应的阶段编号（1 基）。
pub fn phase_number(round: usize) -> usize {
    round.div_ceil(ROUNDS_PER_PHASE)
}

/// 去掉 YAML frontmatter（`---` 包裹的部分）。
fn strip_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---") {
        if let Some(end) = rest.find("---") {
            return rest[end + 3..].trim_start();
        }
    }
    trimmed
}

/// 解析阶段摘要中的 round_start / round_end。
fn parse_phase_meta(content: &str, default_phase: usize) -> PhaseSummary {
    let body = strip_frontmatter(content);
    let mut round_start = (default_phase.saturating_sub(1)) * ROUNDS_PER_PHASE + 1;
    let mut round_end = round_start + ROUNDS_PER_PHASE - 1;
    if let Some(idx) = content.find("round_start:") {
        if let Some(line_end) = content[idx..].find('\n') {
            if let Ok(v) = content[idx + "round_start:".len()..idx + line_end].trim().parse::<usize>() {
                round_start = v;
            }
        }
    }
    if let Some(idx) = content.find("round_end:") {
        if let Some(line_end) = content[idx..].find('\n') {
            if let Ok(v) = content[idx + "round_end:".len()..idx + line_end].trim().parse::<usize>() {
                round_end = v;
            }
        }
    }
    PhaseSummary {
        phase: default_phase,
        round_start,
        round_end,
        content: body.to_string(),
        tokens: TokenCounter::estimate(body),
    }
}

/// 用 LLM 将多条下层摘要聚合为一条上层摘要。
///
/// 用于：
/// - 阶段摘要：多条轮次摘要（层级 1）→ 一条阶段摘要（层级 2）
/// - 会话摘要：多条阶段摘要（层级 2）→ 一条会话摘要（层级 3）
///
/// 若 items 为空返回空字符串；若 LLM 失败返回空字符串（由调用方决定处理方式）。
pub async fn aggregate_summaries(
    llm: &LlmClient,
    level_name: &str,
    items: &[String],
    max_tokens: usize,
) -> Result<String, AppError> {
    if items.is_empty() {
        return Ok(String::new());
    }
    let joined = items.join("\n\n");
    let prompt = format!(
        "请综合以下 {count} 条{level}的摘要，批量生成一份更高级别的{level}摘要，必须保留：\n\
         - 已完成的关键步骤和进展\n\
         - 重要决策及其理由\n\
         - 跨{level}一致的问题或风险\n\
         - 待处理事项\n\
         合并重复信息，去除冗余。控制在 {max_tokens} tokens 以内，使用 Markdown 格式。\n\n\
         ---{level}开始---\n{joined}\n---{level}结束---\n\n聚合摘要：",
        count = items.len(),
        level = level_name,
        max_tokens = max_tokens,
    );

    let response = llm
        .call(
            vec![
                LlmMessage {
                    role: "system".to_string(),
                    content: Some("你是一个高效的对话摘要聚合助手。".to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                },
                LlmMessage {
                    role: "user".to_string(),
                    content: Some(prompt),
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            Vec::new(),
        )
        .await?;

    let text = match response {
        LlmResponse::Text(t) => t.trim().to_string(),
        LlmResponse::ToolCalls(_) => {
            tracing::warn!("摘要聚合 LLM 返回工具调用，返回空摘要");
            String::new()
        }
        LlmResponse::Error(e) => {
            tracing::warn!("摘要聚合 LLM 返回错误: {}", e);
            String::new()
        }
    };
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn phase_number_groups_by_five() {
        assert_eq!(phase_number(1), 1);
        assert_eq!(phase_number(5), 1);
        assert_eq!(phase_number(6), 2);
        assert_eq!(phase_number(10), 2);
        assert_eq!(phase_number(11), 3);
    }

    #[test]
    fn strip_frontmatter_removes_yaml() {
        let content = "---\nid: round-1\ntype: round-summary\n---\n# 摘要正文";
        assert_eq!(strip_frontmatter(content), "# 摘要正文");
    }

    #[test]
    fn strip_frontmatter_returns_as_is_when_no_frontmatter() {
        let content = "# 无 frontmatter";
        assert_eq!(strip_frontmatter(content), "# 无 frontmatter");
    }

    #[test]
    fn store_save_and_load_rounds_sorted() {
        let dir = tempdir().unwrap();
        let store = SummaryStore::new("sess-1", dir.path());
        store.save_round(2, "第二轮摘要").unwrap();
        store.save_round(1, "第一轮摘要").unwrap();

        let rounds = store.load_rounds().unwrap();
        assert_eq!(rounds.len(), 2);
        assert_eq!(rounds[0].round, 1);
        assert_eq!(rounds[1].round, 2);
        assert!(rounds[0].tokens > 0, "round should have token estimate");
    }

    #[test]
    fn store_save_and_load_phases_with_round_range() {
        let dir = tempdir().unwrap();
        let store = SummaryStore::new("sess-2", dir.path());
        store.save_phase(1, 5, "阶段一摘要").unwrap();
        store.save_phase(6, 10, "阶段二摘要").unwrap();

        let phases = store.load_phases().unwrap();
        assert_eq!(phases.len(), 2);
        assert_eq!(phases[0].phase, 1);
        assert_eq!(phases[0].round_start, 1);
        assert_eq!(phases[0].round_end, 5);
        assert_eq!(phases[1].phase, 2);
        assert_eq!(phases[1].round_start, 6);
        assert_eq!(phases[1].round_end, 10);
    }

    #[test]
    fn store_save_and_load_final() {
        let dir = tempdir().unwrap();
        let store = SummaryStore::new("sess-3", dir.path());
        assert!(store.load_final().unwrap().is_none());

        store.save_final("会话总摘要").unwrap();
        let final_summary = store.load_final().unwrap();
        assert_eq!(final_summary.as_deref(), Some("会话总摘要"));
    }

    #[test]
    fn store_load_all_aggregates_layers() {
        let dir = tempdir().unwrap();
        let store = SummaryStore::new("sess-4", dir.path());
        store.save_round(1, "r1").unwrap();
        store.save_phase(1, 5, "p1").unwrap();
        store.save_final("final").unwrap();

        let all = store.load_all().unwrap();
        assert_eq!(all.rounds.len(), 1);
        assert_eq!(all.phases.len(), 1);
        assert_eq!(all.final_summary.as_deref(), Some("final"));
    }

    #[test]
    fn store_with_empty_session_uses_default() {
        let dir = tempdir().unwrap();
        let store = SummaryStore::new("  ", dir.path());
        assert_eq!(store.session_id(), "default");
        assert!(store.root().ends_with("default"));
    }

    #[test]
    fn aggregate_empty_returns_empty() {
        // 不调用 LLM：空列表直接返回空字符串
        let dir = tempdir().unwrap();
        let config = crate::llm::ProviderConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            api_url: "http://localhost:9999/v1".to_string(),
            api_key: Some("test-key".to_string()),
            model: "test-model".to_string(),
            temperature: Some(0.0),
            max_tokens: Some(100),
        };
        let llm = LlmClient::from_configs(vec![config]).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(aggregate_summaries(&llm, "轮次", &[], 800)).unwrap();
        assert!(result.is_empty());
        drop(dir);
    }
}