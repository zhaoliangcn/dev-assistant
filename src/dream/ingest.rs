//! 会话日志解析与经验候选提取（Dream 阶段 ①）。
//!
//! 扫描 `.dev-assistant-store/*.jsonl`（SessionStore 事件流），提取三类经验候选：
//! - 成功流程（`SuccessFlow`）：多步骤工具调用最终成功，可复用的流程
//! - 失败教训（`FailureLesson`）：同一工具连续失败或编译反复出错
//! - 用户纠正（`UserCorrection`）：用户打断/否定后的修正内容
//!
//! 纯规则实现，零 LLM 成本。输出供后续 consolidate / dedup 阶段消费。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::persist::SessionStore;
use crate::utils::error::AppError;

/// 经验候选类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateKind {
    /// 成功的多步骤流程（可复用）
    SuccessFlow,
    /// 失败教训（应避免的模式）
    FailureLesson,
    /// 用户纠正（偏好/方向修正）
    UserCorrection,
}

impl CandidateKind {
    /// 中文标签（用于报告与入库标题）。
    pub fn label(&self) -> &'static str {
        match self {
            CandidateKind::SuccessFlow => "成功流程",
            CandidateKind::FailureLesson => "失败教训",
            CandidateKind::UserCorrection => "用户纠正",
        }
    }
}

/// 一条经验候选。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperienceCandidate {
    /// 候选类型
    pub kind: CandidateKind,
    /// 来源会话 ID
    pub session_id: String,
    /// 概要（入库标题用）
    pub summary: String,
    /// 详情（入库正文用）
    pub details: String,
    /// 事件时间戳（ISO 格式）
    pub timestamp: String,
    /// 相关工具名（失败教训用）
    pub tool_name: Option<String>,
    /// 置信度 0.0~1.0
    pub score: f64,
}

/// 单会话解析结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionScan {
    /// 会话 ID
    pub session_id: String,
    /// 提取的经验候选
    pub candidates: Vec<ExperienceCandidate>,
}

/// 连续失败阈值：同一工具连续失败达到此值记为失败教训。
const FAILURE_RETRY_THRESHOLD: usize = 3;

/// 用户纠正的否定/修正关键词（小写匹配）。
///
/// 按语义分组，各词独立子串匹配。英文词已小写，比较时用户消息也会 to_lowercase。
const CORRECTION_KEYWORDS: &[&str] = &[
    // ── 中文否定 / 打断 ──
    "不对", "不是", "不要", "不要用", "不要那样", "不要这样",
    "不需要", "别这样", "别那样", "别这么做", "别用", "先别",
    "别那么", "不是的", "不是这样", "这样做不对", "不对的",
    // ── 中文错误识别 ──
    "错了", "说错了", "弄错了", "搞错了", "错了重新",
    // ── 中文变更方向 ──
    "重新", "重新来", "换一种", "换个", "换种", "改回", "反悔",
    "纠正", "取消", "撤销", "撤回", "推翻",
    // ── 中文打断 ──
    "停下", "停止",
    // ── 英文通用 ──
    "stop", "wrong", "cancel", "revert", "undo", "nevermind",
    "scratch that", "that's wrong", "not that", "try again",
    "hold on", "rethink", "ignore", "drop that", "forget it",
    "that's not", "actually", "change approach", "different way",
];

/// 判断用户消息是否包含纠正信号词。
fn is_correction_signal(content: &str) -> bool {
    let lower = content.to_lowercase();
    CORRECTION_KEYWORDS
        .iter()
        .any(|k| lower.contains(k))
}

/// 扫描工作目录下 `.dev-assistant-store/` 中的全部会话日志，提取经验候选。
///
/// `working_dir` 为项目根目录（`list_sessions` 内部会拼接 `.dev-assistant-store`，
/// 传入 store 目录本身会导致重复拼接而扫不到任何会话）。
/// 解析失败的会话会被跳过并记录 warning，不影响整体扫描。
pub fn scan_all_sessions(working_dir: &Path) -> Result<Vec<SessionScan>, AppError> {
    let mut scans = Vec::new();
    for session_path in SessionStore::list_sessions(working_dir)? {
        let session_id = session_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        match scan_session(&session_path, &session_id) {
            Ok(scan) => {
                if !scan.candidates.is_empty() {
                    scans.push(scan);
                }
            }
            Err(e) => {
                tracing::warn!(
                    path = %session_path.display(),
                    error = %e,
                    "解析会话日志失败，跳过"
                );
            }
        }
    }
    Ok(scans)
}

/// 解析单个会话日志文件，提取经验候选。
pub fn scan_session(path: &Path, session_id: &str) -> Result<SessionScan, AppError> {
    let events = SessionStore::read_events(path)?;
    Ok(SessionScan {
        session_id: session_id.to_string(),
        candidates: extract_candidates(session_id, &events),
    })
}

/// 从事件流中提取经验候选（纯规则）。
fn extract_candidates(session_id: &str, events: &[crate::persist::SessionEvent]) -> Vec<ExperienceCandidate> {
    let mut candidates = Vec::new();

    // 统计工具失败：tool_name → 连续失败次数
    let mut fail_counts: Vec<(String, usize)> = Vec::new();

    for event in events {
        use crate::persist::SessionEvent::*;
        match event {
            ToolResult { name, success, timestamp, .. } => {
                if !success {
                    // 连续失败计数
                    if let Some(entry) = fail_counts.iter_mut().find(|(n, _)| n == name) {
                        entry.1 += 1;
                    } else {
                        fail_counts.push((name.clone(), 1));
                    }

                    // 达到阈值 → 失败教训候选（先克隆工具名，避免 retain 借用冲突）
                    let tool_clone = name.clone();
                    let count = fail_counts
                        .iter()
                        .find(|(n, _)| n == &tool_clone)
                        .map(|(_, c)| *c)
                        .unwrap_or(0);
                    if count >= FAILURE_RETRY_THRESHOLD {
                        candidates.push(ExperienceCandidate {
                            kind: CandidateKind::FailureLesson,
                            session_id: session_id.to_string(),
                            summary: format!("工具 {} 连续失败 {} 次", tool_clone, count),
                            details: format!(
                                "工具 `{}` 在会话 `{}` 中连续失败 {} 次，可能需要检查参数或改用其他方案。",
                                tool_clone, session_id, count
                            ),
                            // 此时 timestamp 即触发阈值的最后一次失败时间
                            timestamp: timestamp.clone(),
                            tool_name: Some(tool_clone.clone()),
                            score: 0.8,
                        });
                        // 重置该工具的失败计数，避免同工具跨阶段重复计数
                        fail_counts.retain(|(n, _)| n != &tool_clone);
                    }
                } else {
                    // 成功则清零该工具的连续失败
                    fail_counts.retain(|(n, _)| n != name);
                }
            }
            UserMessage { content, timestamp, .. } => {
                if is_correction_signal(content) && content.chars().count() <= 200 {
                    candidates.push(ExperienceCandidate {
                        kind: CandidateKind::UserCorrection,
                        session_id: session_id.to_string(),
                        summary: "用户纠正".to_string(),
                        details: format!(
                            "用户消息包含修正信号：\n> {}",
                            content.trim()
                        ),
                        timestamp: timestamp.clone(),
                        tool_name: None,
                        score: 0.6,
                    });
                }
            }
            _ => {}
        }
    }

    // 成功流程：统计连续成功工具调用序列（>= 3 次成功视为可复用流程）
    let mut success_run = 0usize;
    let mut max_success_run = 0usize;
    let mut run_tools: Vec<String> = Vec::new();
    let mut best_run_tools: Vec<String> = Vec::new();
    for event in events {
        use crate::persist::SessionEvent::*;
        match event {
            ToolResult { name, success, .. } if *success => {
                success_run += 1;
                run_tools.push(name.clone());
                if success_run > max_success_run {
                    max_success_run = success_run;
                    best_run_tools = run_tools.clone();
                }
            }
            _ => {
                success_run = 0;
                run_tools.clear();
            }
        }
    }
    if max_success_run >= 3 {
        candidates.push(ExperienceCandidate {
            kind: CandidateKind::SuccessFlow,
            session_id: session_id.to_string(),
            summary: format!("成功流程（{} 步）", max_success_run),
            details: format!(
                "会话 `{}` 中检测到连续 {} 次成功的工具调用序列：`{}`，可作为可复用流程。",
                session_id,
                max_success_run,
                best_run_tools.join(" → ")
            ),
            timestamp: String::new(),
            tool_name: None,
            score: 0.5,
        });
    }

    candidates
}

/// 汇总所有扫描结果的候选列表（供下游阶段使用）。
pub fn flatten(scans: &[SessionScan]) -> Vec<ExperienceCandidate> {
    scans
        .iter()
        .flat_map(|s| s.candidates.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// 构造一个会话日志文件并返回其路径。
    fn write_session_file(dir: &Path, events_json: &str) -> std::path::PathBuf {
        let path = dir.join("session_test.jsonl");
        std::fs::write(&path, events_json).unwrap();
        path
    }

    #[test]
    fn extract_failure_lesson_after_three_failures() {
        let events = vec![
            crate::persist::SessionEvent::ToolCallRequest {
                timestamp: "2026-08-10T00:00:00Z".into(),
                session_id: "s1".into(),
                tool_call_id: "c1".into(),
                name: "edit_file".into(),
                arguments: serde_json::json!({}),
            },
            crate::persist::SessionEvent::ToolResult {
                timestamp: "2026-08-10T00:00:01Z".into(),
                session_id: "s1".into(),
                tool_call_id: "c1".into(),
                name: "edit_file".into(),
                success: false,
                content: "fail 1".into(),
            },
            crate::persist::SessionEvent::ToolResult {
                timestamp: "2026-08-10T00:00:02Z".into(),
                session_id: "s1".into(),
                tool_call_id: "c2".into(),
                name: "edit_file".into(),
                success: false,
                content: "fail 2".into(),
            },
            crate::persist::SessionEvent::ToolResult {
                timestamp: "2026-08-10T00:00:03Z".into(),
                session_id: "s1".into(),
                tool_call_id: "c3".into(),
                name: "edit_file".into(),
                success: false,
                content: "fail 3".into(),
            },
        ];
        let cands = extract_candidates("s1", &events);
        let lessons: Vec<_> = cands
            .iter()
            .filter(|c| c.kind == CandidateKind::FailureLesson)
            .collect();
        assert_eq!(lessons.len(), 1, "连续失败 3 次应产生 1 条失败教训");
        assert_eq!(lessons[0].tool_name.as_deref(), Some("edit_file"));
        assert!(lessons[0].summary.contains("3 次"));
    }

    #[test]
    fn extract_user_correction_keyword() {
        let events = vec![
            crate::persist::SessionEvent::UserMessage {
                timestamp: "2026-08-10T00:00:00Z".into(),
                session_id: "s1".into(),
                content: "不要用这种方式，重新来".into(),
            },
        ];
        let cands = extract_candidates("s1", &events);
        assert!(
            cands.iter().any(|c| c.kind == CandidateKind::UserCorrection),
            "含修正关键词的用户消息应产生纠正候选"
        );
    }

    #[test]
    fn chinese_correction_keywords_are_detected() {
        let chinese_signals = vec![
            "撤销刚才的操作",
            "取消这个请求",
            "纠正一下，应该用另一种方式",
            "反悔了，重新来",
            "改回原来的方案",
            "弄错了，不是这个",
            "搞错了，换一个",
            "不是这样，重新做",
            "别那样，这样做不对",
            "先别，停下",
            "推翻之前的决策",
            "撤回上一条消息",
            "错了重新开始",
            "不要那样做",
            "这样做不对，换一种",
        ];
        for signal in chinese_signals {
            let events = vec![crate::persist::SessionEvent::UserMessage {
                timestamp: "2026-08-10T00:00:00Z".into(),
                session_id: "s1".into(),
                content: signal.to_string(),
            }];
            let cands = extract_candidates("s1", &events);
            assert!(
                cands.iter().any(|c| c.kind == CandidateKind::UserCorrection),
                "中文纠正信号词应被识别: '{}'",
                signal
            );
        }
    }

    #[test]
    fn english_correction_keywords_are_detected() {
        let english_signals = vec![
            "cancel this request",
            "revert that change",
            "undo what you did",
            "nevermind, let's try something else",
            "scratch that, different approach",
            "that's wrong, do it again",
            "not that, use the other one",
            "try again with a different tool",
            "hold on, let me think",
            "rethink the approach",
            "ignore that, do this instead",
            "drop that idea",
            "forget it, start over",
            "that's not what I meant",
            "actually, use a different approach",
            "change approach, this isn't working",
            "different way please",
        ];
        for signal in english_signals {
            let events = vec![crate::persist::SessionEvent::UserMessage {
                timestamp: "2026-08-10T00:00:00Z".into(),
                session_id: "s1".into(),
                content: signal.to_string(),
            }];
            let cands = extract_candidates("s1", &events);
            assert!(
                cands.iter().any(|c| c.kind == CandidateKind::UserCorrection),
                "英文纠正信号词应被识别: '{}'",
                signal
            );
        }
    }

    #[test]
    fn non_correction_message_is_not_flagged() {
        let normal_messages = vec![
            "你好，请帮我实现这个功能",
            "请帮我写一段 Rust 代码",
            "解释一下这个算法的原理",
            "帮我分析一下这个项目的架构",
            "请优化一下这段代码的性能",
            "hello, please help me with this task",
            "can you explain how this works",
            "please implement this feature",
            "write a function that does X",
            "how does this algorithm work",
        ];
        for msg in &normal_messages {
            let events = vec![crate::persist::SessionEvent::UserMessage {
                timestamp: "2026-08-10T00:00:00Z".into(),
                session_id: "s1".into(),
                content: msg.to_string(),
            }];
            let cands = extract_candidates("s1", &events);
            assert!(
                cands.iter().all(|c| c.kind != CandidateKind::UserCorrection),
                "正常对话不应被识别为纠正: '{}'",
                msg
            );
        }
    }

    #[test]
    fn extract_success_flow_after_three_successes() {
        let events = vec![
            crate::persist::SessionEvent::ToolResult {
                timestamp: "2026-08-10T00:00:00Z".into(),
                session_id: "s1".into(),
                tool_call_id: "c1".into(),
                name: "read_file".into(),
                success: true,
                content: "ok".into(),
            },
            crate::persist::SessionEvent::ToolResult {
                timestamp: "2026-08-10T00:00:01Z".into(),
                session_id: "s1".into(),
                tool_call_id: "c2".into(),
                name: "edit_file".into(),
                success: true,
                content: "ok".into(),
            },
            crate::persist::SessionEvent::ToolResult {
                timestamp: "2026-08-10T00:00:02Z".into(),
                session_id: "s1".into(),
                tool_call_id: "c3".into(),
                name: "cargo_check".into(),
                success: true,
                content: "ok".into(),
            },
        ];
        let cands = extract_candidates("s1", &events);
        assert!(
            cands.iter().any(|c| c.kind == CandidateKind::SuccessFlow),
            "连续 3 次成功工具调用应产生成功流程候选"
        );
    }

    #[test]
    fn scan_session_reads_file() {
        let dir = tempdir().unwrap();
        let path = write_session_file(
            dir.path(),
            "{\"type\":\"user_message\",\"timestamp\":\"2026-08-10T00:00:00Z\",\"session_id\":\"s1\",\"content\":\"你好\"}\n",
        );
        let scan = scan_session(&path, "s1").unwrap();
        assert_eq!(scan.session_id, "s1");
    }

    #[test]
    fn scan_all_sessions_finds_store_directory() {
        // 回归测试：scan_all_sessions 接收 working_dir，内部拼接 .dev-assistant-store，
        // 之前误传 store_dir 本身导致重复拼接、扫不到任何会话（采集恒为 0）。
        let dir = tempdir().unwrap();
        let store_dir = dir.path().join(".dev-assistant-store");
        std::fs::create_dir_all(&store_dir).unwrap();

        // 写入一个含连续 3 次失败的会话文件（应产生 1 条失败教训候选）
        let lines = [
            "{\"type\":\"tool_result\",\"timestamp\":\"2026-08-10T00:00:00Z\",\"session_id\":\"s1\",\"tool_call_id\":\"c1\",\"name\":\"edit_file\",\"success\":false,\"content\":\"fail 1\"}\n",
            "{\"type\":\"tool_result\",\"timestamp\":\"2026-08-10T00:00:01Z\",\"session_id\":\"s1\",\"tool_call_id\":\"c2\",\"name\":\"edit_file\",\"success\":false,\"content\":\"fail 2\"}\n",
            "{\"type\":\"tool_result\",\"timestamp\":\"2026-08-10T00:00:02Z\",\"session_id\":\"s1\",\"tool_call_id\":\"c3\",\"name\":\"edit_file\",\"success\":false,\"content\":\"fail 3\"}\n",
        ];
        std::fs::write(store_dir.join("session_scan-test.jsonl"), lines.concat()).unwrap();

        // 传 working_dir（而非 store_dir）
        let scans = scan_all_sessions(dir.path()).unwrap();
        assert_eq!(scans.len(), 1, "应扫到 1 个含候选的会话");
        assert_eq!(scans[0].session_id, "session_scan-test");
        assert!(
            scans[0]
                .candidates
                .iter()
                .any(|c| c.kind == CandidateKind::FailureLesson),
            "应提取到失败教训候选"
        );
    }

    #[test]
    fn scan_all_sessions_empty_without_store() {
        // 无 .dev-assistant-store 目录时应返回空，而不是报错
        let dir = tempdir().unwrap();
        let scans = scan_all_sessions(dir.path()).unwrap();
        assert!(scans.is_empty());
    }
}
