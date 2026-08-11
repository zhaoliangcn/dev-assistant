//! 记忆巩固（Dream 阶段 ②）。
//!
//! 将采集阶段（① ingest）产出的经验候选巩固为可入库的知识条目：
//!
//! - **纯规则模式**（`llm = None`）：每个候选直接写入一条 `experience` 条目
//!   （`.kb/experiences/dream-{ts}-{n}.md`，带 frontmatter）
//! - **LLM 模式**（`llm = Some` 且有预算）：先用 `aggregate_summaries` 把候选
//!   聚合提炼为一条"核心经验"（去冗余、合并重复信息），失败时回退到规则模式
//!
//! 条目写入后不直接改索引——索引重建由 DreamEngine 阶段 ⑤ 批量完成。

use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::agent::summary::aggregate_summaries;
use crate::dream::ingest::{CandidateKind, ExperienceCandidate};
use crate::llm::LlmClient;
use crate::utils::error::AppError;

/// 一条巩固后写入的条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidatedEntry {
    /// 条目 ID
    pub id: String,
    /// 相对 `.kb/` 的路径
    pub path: String,
    /// 标题
    pub title: String,
    /// 来源候选类型
    pub kind: CandidateKind,
}

/// 巩固阶段结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsolidateResult {
    /// 写入的条目
    pub entries: Vec<ConsolidatedEntry>,
}

impl ConsolidateResult {
    /// 本轮写入的条目数。
    pub fn count(&self) -> usize {
        self.entries.len()
    }
}

/// 把经验候选巩固为 KB 条目文件（写入 `.kb/experiences/`）。
///
/// `kb_root` 为 `.kb/` 目录。`llm` 与 `budget_tokens` 控制是否启用 LLM 聚合：
/// - `llm = Some` 且 `budget_tokens > 0` 且候选 ≥ 2 条：尝试 LLM 聚合提炼
/// - 其余情况：逐条规则写入
pub async fn consolidate_candidates(
    candidates: &[ExperienceCandidate],
    kb_root: &Path,
    llm: Option<&LlmClient>,
    budget_tokens: usize,
) -> Result<ConsolidateResult, AppError> {
    if candidates.is_empty() {
        return Ok(ConsolidateResult::default());
    }

    // LLM 模式：候选 ≥ 2 条且有预算时，尝试聚合提炼为一条核心经验
    if let (Some(llm), true) = (llm, budget_tokens > 0 && candidates.len() >= 2) {
        if let Some(entry) = try_llm_consolidate(candidates, kb_root, llm).await? {
            return Ok(ConsolidateResult {
                entries: vec![entry],
            });
        }
        // LLM 聚合失败（返回空摘要或调用出错）→ 回退到规则模式
        tracing::warn!("LLM 聚合失败，回退到规则模式逐条写入");
    }

    // 规则模式：逐条写入
    let mut entries = Vec::new();
    let ts = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    for (i, cand) in candidates.iter().enumerate() {
        let id = format!("dream-{}-{}", ts, i + 1);
        let path = format!("experiences/{}.md", id);
        let kind_label = cand.kind.label();
        let title = format!("[{}] {}", kind_label, cand.summary);
        let tags = vec![
            "dream".to_string(),
            "experience".to_string(),
            kind_label.to_string(),
        ];
        let content = format!(
            "---\nid: {}\ntype: experience\ntitle: {}\ntags: [{}]\nstatus: draft\ncreated: {}\n---\n\n# {}\n\n{}\n",
            id,
            title,
            tags.join(", "),
            Utc::now().to_rfc3339(),
            title,
            cand.details
        );
        let file_path = kb_root.join(&path);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).map_err(AppError::Io)?;
        }
        std::fs::write(&file_path, content).map_err(AppError::Io)?;
        entries.push(ConsolidatedEntry {
            id,
            path,
            title,
            kind: cand.kind,
        });
    }

    Ok(ConsolidateResult { entries })
}

/// LLM 聚合提炼：把多条候选合并为一条核心经验条目。
///
/// 复用分层摘要系统的 `aggregate_summaries`（同一 LLM 聚合提示词族）。
/// 成功写入一条 `experience` 条目并返回；LLM 返回空摘要或失败时返回 `None`。
async fn try_llm_consolidate(
    candidates: &[ExperienceCandidate],
    kb_root: &Path,
    llm: &LlmClient,
) -> Result<Option<ConsolidatedEntry>, AppError> {
    let items: Vec<String> = candidates
        .iter()
        .map(|c| format!("【{}】{}", c.kind.label(), c.details))
        .collect();

    let summary = aggregate_summaries(llm, "经验候选", &items, 800).await?;
    if summary.is_empty() {
        return Ok(None);
    }

    let id = format!("dream-llm-{}", Utc::now().format("%Y%m%d-%H%M%S"));
    let path = format!("experiences/{}.md", id);
    let title = format!("核心经验（{} 条候选聚合）", candidates.len());
    let kind = if candidates.iter().any(|c| c.kind == CandidateKind::FailureLesson) {
        CandidateKind::FailureLesson
    } else if candidates.iter().any(|c| c.kind == CandidateKind::UserCorrection) {
        CandidateKind::UserCorrection
    } else {
        CandidateKind::SuccessFlow
    };
    let kind_label = kind.label();
    let tags = vec![
        "dream".to_string(),
        "experience".to_string(),
        "consolidated".to_string(),
        kind_label.to_string(),
    ];
    let content = format!(
        "---\nid: {}\ntype: experience\ntitle: {}\ntags: [{}]\nstatus: draft\ncreated: {}\n---\n\n# {}\n\n{}\n",
        id,
        title,
        tags.join(", "),
        Utc::now().to_rfc3339(),
        title,
        summary
    );
    let file_path = kb_root.join(&path);
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }
    std::fs::write(&file_path, content).map_err(AppError::Io)?;

    Ok(Some(ConsolidatedEntry {
        id,
        path,
        title,
        kind,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn candidate(kind: CandidateKind, summary: &str) -> ExperienceCandidate {
        ExperienceCandidate {
            kind,
            session_id: "s1".to_string(),
            summary: summary.to_string(),
            details: format!("{} 详情", summary),
            timestamp: "2026-08-10T00:00:00Z".to_string(),
            tool_name: None,
            score: 0.8,
        }
    }

    #[tokio::test]
    async fn empty_candidates_return_empty() {
        let dir = tempdir().unwrap();
        let result = consolidate_candidates(&[], dir.path(), None, 0).await.unwrap();
        assert_eq!(result.count(), 0);
    }

    #[tokio::test]
    async fn rules_mode_writes_each_candidate() {
        let dir = tempdir().unwrap();
        let kb_root = dir.path().join(".kb");
        let cands = vec![
            candidate(CandidateKind::FailureLesson, "edit_file 连续失败"),
            candidate(CandidateKind::SuccessFlow, "成功流程"),
        ];
        let result = consolidate_candidates(&cands, &kb_root, None, 0).await.unwrap();
        assert_eq!(result.count(), 2);

        // 验证文件已写入且带 frontmatter
        for entry in &result.entries {
            let file_path = kb_root.join(&entry.path);
            assert!(file_path.exists(), "条目文件应存在: {}", entry.path);
            let content = std::fs::read_to_string(&file_path).unwrap();
            assert!(content.starts_with("---"), "应包含 frontmatter");
            assert!(content.contains("type: experience"));
            assert!(content.contains(&format!("id: {}", entry.id)));
        }
    }

    #[tokio::test]
    async fn rules_mode_creates_experiences_dir() {
        let dir = tempdir().unwrap();
        let kb_root = dir.path().join(".kb");
        let cands = vec![candidate(CandidateKind::UserCorrection, "用户纠正")];
        consolidate_candidates(&cands, &kb_root, None, 0).await.unwrap();
        assert!(kb_root.join("experiences").is_dir());
    }
}
