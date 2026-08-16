//! Dream 报告与 undo 快照（Dream 阶段 ⑥）。
//!
//! - **undo 快照**：运行前把 `.kb/index.json` 复制到 `.kb/dream-undo/dream-{ts}-index.json`，
//!   供回滚（dream 只归档不删，快照是额外保险）
//! - **报告**：输出 `.kb/reports/dream-YYYYMMDD.md`，记录本轮各阶段动作明细

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::utils::error::AppError;

/// 单轮 Dream 报告数据。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DreamReport {
    /// 运行时间（RFC3339）
    pub run_at: String,
    /// 是否预演模式
    pub dry_run: bool,
    /// 采集的经验候选数
    pub ingested: usize,
    /// 巩固写入的条目数
    pub consolidated: usize,
    /// 去重合并的条目数
    pub deduplicated: usize,
    /// 归档的条目数
    pub archived: usize,
    /// LLM token 消耗（估算）
    pub llm_tokens_used: usize,
    /// 是否发生错误
    pub has_errors: bool,
    /// 动作明细（每行一条，供报告与追溯）
    pub details: Vec<String>,
}

impl DreamReport {
    /// 记录一条动作明细。
    #[allow(dead_code)] // 生产路径直接 push details；保留为公开 builder + 测试用
    pub fn add_detail(&mut self, detail: impl Into<String>) {
        self.details.push(detail.into());
    }
}

/// 运行前对 `.kb/index.json` 做 undo 快照。
///
/// 快照保存到 `.kb/dream-undo/dream-{YYYYMMDD-HHMMSS}-index.json`。
/// `dry_run` 或索引不存在时跳过。返回快照路径（若有）。
pub fn snapshot_index(kb_root: &Path, dry_run: bool) -> Result<Option<PathBuf>, AppError> {
    if dry_run {
        return Ok(None);
    }
    let index_path = kb_root.join("index.json");
    if !index_path.exists() {
        return Ok(None);
    }

    let undo_dir = kb_root.join("dream-undo");
    std::fs::create_dir_all(&undo_dir).map_err(|e| {
        AppError::Io(std::io::Error::other(format!(
            "创建 undo 目录失败 ({}): {}",
            undo_dir.display(),
            e
        )))
    })?;

    let ts = Utc::now().format("%Y%m%d-%H%M%S");
    let snapshot_path = undo_dir.join(format!("dream-{}-index.json", ts));
    std::fs::copy(&index_path, &snapshot_path).map_err(|e| {
        AppError::Io(std::io::Error::other(format!(
            "快照 KB 索引失败 ({} → {}): {}",
            index_path.display(),
            snapshot_path.display(),
            e
        )))
    })?;

    tracing::info!(path = %snapshot_path.display(), "KB 索引 undo 快照已创建");
    Ok(Some(snapshot_path))
}

/// 写入 Dream 报告到 `.kb/reports/dream-YYYYMMDD.md`。
///
/// 返回报告文件路径。报告包含 frontmatter（id/type/title/status）与动作明细，
/// 可被 `kb_query` 检索（需下次索引重建后）。
pub fn write_report(kb_root: &Path, report: &DreamReport) -> Result<PathBuf, AppError> {
    let reports_dir = kb_root.join("reports");
    std::fs::create_dir_all(&reports_dir).map_err(|e| {
        AppError::Io(std::io::Error::other(format!(
            "创建报告目录失败 ({}): {}",
            reports_dir.display(),
            e
        )))
    })?;

    let date = report.run_at.chars().take(10).collect::<String>().replace('-', "");
    let path = reports_dir.join(format!("dream-{}.md", date));

    let mode = if report.dry_run { "dry-run 预演" } else { "正式" };
    let mut content = format!(
        "---\nid: dream-{}\ntype: report\ntitle: Dream 记忆整理报告（{}）\n\
         status: completed\ntags: [dream, memory, consolidation]\n\
         created: {}\n---\n\n# Dream 记忆整理报告\n\n",
        date, mode, report.run_at
    );
    content.push_str(&format!(
        "- 运行模式: **{}**\n- 运行时间: {}\n",
        mode, report.run_at
    ));
    if report.dry_run {
        content.push_str("- ⚠️ 预演模式：未修改任何数据\n");
    }
    content.push_str("\n## 统计\n\n");
    content.push_str(&format!(
        "| 阶段 | 数量 |\n|------|------|\n| ① 采集经验候选 | {} |\n\
         | ② 巩固写入条目 | {} |\n| ③ 去重合并 | {} |\n| ④ 归档条目 | {} |\n\
         | LLM token 消耗（估算） | {} |\n",
        report.ingested,
        report.consolidated,
        report.deduplicated,
        report.archived,
        report.llm_tokens_used
    ));
    if report.has_errors {
        content.push_str("\n> ⚠️ 部分阶段发生错误，详见日志。\n");
    }

    if !report.details.is_empty() {
        content.push_str("\n## 动作明细\n\n");
        for detail in &report.details {
            content.push_str(&format!("- {}\n", detail));
        }
    }

    std::fs::write(&path, content).map_err(|e| {
        AppError::Io(std::io::Error::other(format!(
            "写入 Dream 报告失败 ({}): {}",
            path.display(),
            e
        )))
    })?;

    tracing::info!(path = %path.display(), "Dream 报告已写入");
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn snapshot_creates_undo_copy() {
        let dir = tempdir().unwrap();
        let kb_root = dir.path().join(".kb");
        std::fs::create_dir_all(&kb_root).unwrap();
        std::fs::write(kb_root.join("index.json"), r#"{"version":1}"#).unwrap();

        let snapshot = snapshot_index(&kb_root, false).unwrap().expect("应有快照");
        assert!(snapshot.exists());
        assert!(snapshot.to_string_lossy().contains("dream-undo"));
    }

    #[test]
    fn snapshot_dry_run_skips() {
        let dir = tempdir().unwrap();
        let kb_root = dir.path().join(".kb");
        std::fs::create_dir_all(&kb_root).unwrap();
        std::fs::write(kb_root.join("index.json"), r#"{"version":1}"#).unwrap();

        assert!(snapshot_index(&kb_root, true).unwrap().is_none());
        assert!(!kb_root.join("dream-undo").exists());
    }

    #[test]
    fn snapshot_missing_index_skips() {
        let dir = tempdir().unwrap();
        let kb_root = dir.path().join(".kb");
        std::fs::create_dir_all(&kb_root).unwrap();

        assert!(snapshot_index(&kb_root, false).unwrap().is_none());
    }

    #[test]
    fn write_report_creates_markdown() {
        let dir = tempdir().unwrap();
        let kb_root = dir.path().join(".kb");

        let mut report = DreamReport {
            run_at: Utc::now().to_rfc3339(),
            dry_run: false,
            ingested: 5,
            consolidated: 2,
            deduplicated: 1,
            archived: 3,
            llm_tokens_used: 1200,
            has_errors: false,
            details: vec!["归档条目 OLD-001".to_string()],
        };
        report.add_detail("合并条目 NEW-001 ← OLD-002");

        let path = write_report(&kb_root, &report).unwrap();
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("id: dream-"));
        assert!(content.contains("type: report"));
        assert!(content.contains("① 采集经验候选"));
        assert!(content.contains("归档条目 OLD-001"));
        assert!(content.contains("合并条目 NEW-001 ← OLD-002"));
    }
}
