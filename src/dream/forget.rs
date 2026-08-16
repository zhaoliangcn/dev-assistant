//! 记忆遗忘（Dream 阶段 ④）。
//!
//! 健康分 = 时效衰减 × 类型权重 + 状态加成。
//! 低分条目 → `archived=true`（只归档不删，`kb_query` 可搜索、可恢复）。
//!
//! 归档操作：
//! 1. 在 `.kb/index.json` 中置 `archived: true`
//! 2. 在条目 Markdown 文件的 frontmatter 中写入 `archived: true`（保持一致性）
//! 3. 返回归档动作列表，供报告阶段记录与 undo 快照

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::tools::kb::KbIndexEntry;
use crate::utils::error::AppError;

/// 归档阈值：健康分低于此值的条目将被归档。
pub const ARCHIVE_THRESHOLD: f64 = 0.25;

/// 时效半衰期（天）：updated 距今每经过一个半衰期，时效分减半。
const RECENCY_HALF_LIFE_DAYS: f64 = 90.0;

/// 条目类型权重：决策类最值得保留，工具/临时类权重低。
fn type_weight(entry_type: &str) -> f64 {
    match entry_type {
        "decision" => 1.0,
        "summary" => 0.9,
        "interface" => 0.8,
        "analysis" | "report" => 0.7,
        _ => 0.5,
    }
}

/// 状态加成：已接受的决策/完成项加分，废弃/草稿减分。
fn status_bonus(status: &str) -> f64 {
    match status {
        "accepted" | "completed" => 0.2,
        "draft" | "proposed" => -0.1,
        "deprecated" | "superseded" => -0.3,
        _ => 0.0,
    }
}

/// 单条归档动作。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveAction {
    /// 条目 ID
    pub id: String,
    /// 条目路径（相对 `.kb/`）
    pub path: String,
    /// 计算的健康分
    pub health: f64,
    /// 归档原因说明
    pub reason: String,
}

/// 遗忘阶段结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ForgetResult {
    /// 归档动作列表
    pub archived: Vec<ArchiveAction>,
}

impl ForgetResult {
    /// 本轮归档的条目数。
    pub fn count(&self) -> usize {
        self.archived.len()
    }
}

/// 对 KB 索引执行遗忘：计算健康分，低分条目归档（只归档不删）。
///
/// `kb_root` 为 `.kb/` 目录。`dry_run` 为 true 时只计算并返回动作，
/// 不修改任何文件（预演模式）。
pub fn run_forget(kb_root: &Path, dry_run: bool) -> Result<ForgetResult, AppError> {
    let index_path = kb_root.join("index.json");
    if !index_path.exists() {
        return Ok(ForgetResult::default());
    }

    let content = std::fs::read_to_string(&index_path).map_err(|e| {
        AppError::Io(std::io::Error::other(format!("读取 KB 索引失败: {}", e)))
    })?;
    let mut index: crate::tools::kb::KbIndex = serde_json::from_str(&content)
        .map_err(|e| AppError::Config(format!("解析 KB 索引失败: {}", e)))?;

    let now = Utc::now();
    let mut actions = Vec::new();

    // 查询统计从 sidecar 注水：index.json 不再维护 query_count/last_query_at，
    // 遗忘阶段需从 query-stats.json 取回真实命中数据后再计算健康分。
    let query_stats = crate::tools::kb::load_query_stats(kb_root, &index);

    for (id, entry) in index.entries.iter() {
        if entry.archived {
            continue; // 已归档的跳过
        }
        // 注水后计算健康分（不改动写回 index.json 的 entry，避免双真相源）
        let health = {
            let mut e = entry.clone();
            if let Some(s) = query_stats.get(id) {
                e.query_count = s.query_count;
                e.last_query_at = s.last_query_at.clone();
            }
            compute_health(&e, now)
        };
        if health < ARCHIVE_THRESHOLD {
            actions.push(ArchiveAction {
                id: id.clone(),
                path: entry.path.clone(),
                health,
                reason: format!("健康分 {:.2} 低于阈值 {}", health, ARCHIVE_THRESHOLD),
            });
        }
    }

    if dry_run {
        return Ok(ForgetResult { archived: actions });
    }

    // 正式归档：置 archived 标记并写回索引
    for action in &actions {
        if let Some(entry) = index.entries.get_mut(&action.id) {
            entry.archived = true;
        }
        // 同步更新条目文件的 frontmatter（尽力而为，失败不阻断归档）
        let file_path = kb_root.join(&action.path);
        if file_path.exists() {
            if let Ok(fm_text) = std::fs::read_to_string(&file_path) {
                if let Some(updated) = mark_archived_in_frontmatter(&fm_text) {
                    let _ = std::fs::write(&file_path, updated);
                }
            }
        }
    }

    index.updated = now.to_rfc3339();
    let index_json = serde_json::to_string_pretty(&index).map_err(AppError::Json)?;
    std::fs::write(&index_path, index_json).map_err(|e| {
        AppError::Io(std::io::Error::other(format!("写入 KB 索引失败: {}", e)))
    })?;

    Ok(ForgetResult { archived: actions })
}

/// 计算条目的健康分。
///
/// 公式：`健康分 = 时效衰减 × 类型权重 + 状态加成 + 查询激励`，结果夹在 `[0, 1.5]`。
/// 时效衰减按 updated 距今天数指数衰减（半衰期 `RECENCY_HALF_LIFE_DAYS`）。
/// 查询激励按历史命中次数对数增长 + 最近 30 天内被查询过的额外加成，
/// 让高频被使用的条目即使久远也不易被遗忘。
pub fn compute_health(entry: &KbIndexEntry, now: DateTime<Utc>) -> f64 {
    // 时效衰减：updated 距今天数，指数衰减
    let days = entry
        .updated
        .as_deref()
        .and_then(|u| DateTime::parse_from_rfc3339(u).ok())
        .map(|dt| {
            let dt = dt.with_timezone(&Utc);
            (now - dt).num_days().max(0) as f64
        })
        .unwrap_or(0.0);
    let recency = 0.5f64.powf(days / RECENCY_HALF_LIFE_DAYS);

    let health = recency * type_weight(&entry.entry_type)
        + status_bonus(&entry.status)
        + query_bonus(entry, now);
    health.clamp(0.0, 1.5)
}

/// 查询激励：按历史命中次数对数增长，最近 30 天内被查询过额外加成。
///
/// 效应：0 次查询 → 0；1 次 → 0.058；3 次 → 0.149；10 次 → 0.20（上限）；
/// 最近 30 天内查询过额外 +0.08。总激励上限 0.30。
fn query_bonus(entry: &KbIndexEntry, now: DateTime<Utc>) -> f64 {
    let base = (1.0f64 + entry.query_count as f64).ln() * 0.05;
    let recency = entry
        .last_query_at
        .as_deref()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|dt| {
            let days = (now - dt.with_timezone(&Utc)).num_days().max(0);
            if days <= 30 {
                0.08
            } else {
                0.0
            }
        })
        .unwrap_or(0.0);
    (base + recency).min(0.30)
}

/// 在 Markdown 的 frontmatter 中写入 `archived: true`（保留原字段）。
fn mark_archived_in_frontmatter(content: &str) -> Option<String> {
    let fm = crate::utils::frontmatter::parse_frontmatter(content).ok()?;
    let body = crate::utils::frontmatter::extract_body(content);
    let mut fields = fm;
    fields.insert("archived".to_string(), "true".to_string());
    Some(crate::utils::frontmatter::build_document(&fields, &body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// 构造一个测试条目。
    fn entry(entry_type: &str, status: &str, updated_days_ago: i64) -> KbIndexEntry {
        let updated = (Utc::now() - chrono::Duration::days(updated_days_ago)).to_rfc3339();
        KbIndexEntry {
            path: format!("decisions/test-{}.md", entry_type),
            entry_type: entry_type.to_string(),
            title: "Test".to_string(),
            tags: vec!["test".to_string()],
            status: status.to_string(),
            archived: false,
            relates_to: None,
            depends_on: None,
            supersedes: None,
            author: None,
            created: Some(updated.clone()),
            updated: Some(updated),
            query_count: 0,
            last_query_at: None,
        }
    }

    #[test]
    fn recent_accepted_decision_has_high_health() {
        let e = entry("decision", "accepted", 1);
        let health = compute_health(&e, Utc::now());
        assert!(health >= 1.0, "近期已接受的决策健康分应很高，got {}", health);
    }

    #[test]
    fn old_draft_entry_has_low_health() {
        let e = entry("issue", "draft", 400);
        let health = compute_health(&e, Utc::now());
        assert!(health < ARCHIVE_THRESHOLD, "400 天前的草稿应低于归档阈值，got {}", health);
    }

    #[test]
    fn deprecated_entry_decays_fast() {
        let e = entry("decision", "deprecated", 200);
        let health = compute_health(&e, Utc::now());
        assert!(
            health < 0.3,
            "废弃条目即使类型权重高也应低分，got {}",
            health
        );
    }

    #[test]
    fn frequently_queried_entry_has_boosted_health() {
        // 高频查询（query_count=50）的旧条目健康分应显著高于无查询的同龄条目
        let mut queried = entry("decision", "draft", 200);
        queried.query_count = 50;
        queried.last_query_at = Some(Utc::now().to_rfc3339());

        let unqueried = entry("decision", "draft", 200);

        let h_queried = compute_health(&queried, Utc::now());
        let h_unqueried = compute_health(&unqueried, Utc::now());
        assert!(
            h_queried > h_unqueried + 0.15,
            "高频查询条目的健康分应显著高于无查询条目：queried={}, unqueried={}",
            h_queried,
            h_unqueried
        );
    }

    #[test]
    fn recently_queried_old_entry_escapes_archive() {
        // 很久未更新但最近被查询过的条目，健康分应高于归档阈值；
        // 而同龄从未查询的条目应仍被归档（证明是查询激励挽救了它）。
        let mut queried = entry("decision", "draft", 200);
        queried.query_count = 3;
        queried.last_query_at = Some((Utc::now() - chrono::Duration::days(1)).to_rfc3339());

        let unqueried = entry("decision", "draft", 200);

        let h_queried = compute_health(&queried, Utc::now());
        let h_unqueried = compute_health(&unqueried, Utc::now());

        assert!(
            h_unqueried < ARCHIVE_THRESHOLD,
            "同龄未查询条目应被归档：health={}",
            h_unqueried
        );
        assert!(
            h_queried >= ARCHIVE_THRESHOLD,
            "最近被查询的旧条目健康分应高于归档阈值：health={}",
            h_queried
        );
    }

    #[test]
    fn never_queried_entry_gets_no_bonus() {
        // 从未被查询的条目，健康分等于原公式（零查询不产生额外激励）
        let e = entry("issue", "draft", 400);
        let health = compute_health(&e, Utc::now());
        // 无查询激励，应仍低于归档阈值（与 old_draft_entry_has_low_health 一致）
        assert!(
            health < ARCHIVE_THRESHOLD,
            "从未被查询的旧条目应低于归档阈值：health={}",
            health
        );
    }

    #[test]
    fn query_bonus_scales_logarithmically() {
        // 验证查询激励按对数增长、单调递增、不超过上限
        let now = Utc::now();
        let mut base = entry("decision", "accepted", 1);

        let bonuses: Vec<f64> = [0u64, 1, 3, 10, 30, 100, 500]
            .iter()
            .map(|&q| {
                base.query_count = q;
                base.last_query_at = None;
                query_bonus(&base, now)
            })
            .collect();

        // 0 次查询 → 0
        assert_eq!(bonuses[0], 0.0, "0 次查询的激励应为 0");

        // 单调递增
        for i in 1..bonuses.len() {
            assert!(
                bonuses[i] > bonuses[i - 1],
                "查询激励应随查询次数递增：{} 次 {} ≤ {} 次 {}",
                [0, 1, 3, 10, 30, 100, 500][i],
                bonuses[i],
                [0, 1, 3, 10, 30, 100, 500][i - 1],
                bonuses[i - 1]
            );
        }

        // 不超过上限 0.30
        for (i, &b) in bonuses.iter().enumerate() {
            assert!(
                b <= 0.30 + 1e-10,
                "查询激励不应超过 0.30：{} 次查询 → {}",
                [0, 1, 3, 10, 30, 100, 500][i],
                b
            );
        }
    }

    #[test]
    fn run_forget_archives_old_entries() {
        let dir = tempdir().unwrap();
        let kb_root = dir.path().join(".kb");
        std::fs::create_dir_all(&kb_root).unwrap();

        let now = Utc::now();
        let old = entry("issue", "draft", 500);
        let fresh = entry("decision", "accepted", 2);
        let mut index = crate::tools::kb::KbIndex::default();
        index.entries.insert("OLD-001".to_string(), old.clone());
        index.entries.insert("FRESH-001".to_string(), fresh.clone());
        std::fs::write(
            kb_root.join("index.json"),
            serde_json::to_string_pretty(&index).unwrap(),
        )
        .unwrap();
        let _ = now;

        let result = run_forget(&kb_root, false).unwrap();
        assert_eq!(result.count(), 1, "应只归档旧条目");
        assert_eq!(result.archived[0].id, "OLD-001");

        // 验证索引已更新
        let updated: crate::tools::kb::KbIndex =
            serde_json::from_str(&std::fs::read_to_string(kb_root.join("index.json")).unwrap())
                .unwrap();
        assert!(updated.entries["OLD-001"].archived);
        assert!(!updated.entries["FRESH-001"].archived);
    }

    #[test]
    fn run_forget_dry_run_does_not_modify() {
        let dir = tempdir().unwrap();
        let kb_root = dir.path().join(".kb");
        std::fs::create_dir_all(&kb_root).unwrap();

        let old = entry("issue", "draft", 500);
        let mut index = crate::tools::kb::KbIndex::default();
        index.entries.insert("OLD-001".to_string(), old);
        let original = serde_json::to_string_pretty(&index).unwrap();
        std::fs::write(kb_root.join("index.json"), &original).unwrap();

        let result = run_forget(&kb_root, true).unwrap();
        assert_eq!(result.count(), 1, "dry-run 应仍能识别可归档条目");

        let after = std::fs::read_to_string(kb_root.join("index.json")).unwrap();
        assert_eq!(original, after, "dry-run 不应修改索引文件");
    }

    #[test]
    fn mark_archived_adds_frontmatter_flag() {
        let content = "---\nid: ADR-001\ntype: decision\ntitle: Test\n---\n# Body";
        let updated = mark_archived_in_frontmatter(content).unwrap();
        assert!(updated.contains("archived: true"));
        assert!(updated.contains("# Body"));
    }
}
