//! 上下文压缩策略。
//!
//! 当 token 数超过阈值时，只保留最近 N 轮对话，防止上下文无限增长。
//!
//! 支持两种压缩模式：
//! - `Truncate`：直接截断旧消息（当前默认行为）
//! - `Summarize`：用 LLM 将旧消息压缩为语义摘要（保留关键信息）

use crate::agent::history::ConversationHistory;
use crate::llm::{LlmClient, LlmMessage, LlmResponse};
use crate::utils::error::AppError;

/// 保留的对话轮数。
const DEFAULT_ROUNDS_TO_KEEP: usize = 6;

/// 触发压缩的 token 阈值比例（相对于 max_tokens）。
const MAX_CONVERSATION_TOKENS_RATIO: f64 = 0.9;

/// 摘要压缩时保留的完整对话轮数。
const DEFAULT_SUMMARY_KEEP_ROUNDS: usize = 3;

/// 摘要提示词的最大 token 数。
const SUMMARY_MAX_TOKENS: usize = 800;

/// 从环境变量读取保留轮数（`AGENT_ROUNDS_TO_KEEP`），默认 [`DEFAULT_ROUNDS_TO_KEEP`]。
///
/// 允许按任务复杂度调整：复杂长对话可设 8-12 轮，简单对话可设 3-4 轮节省上下文。
fn rounds_to_keep() -> usize {
    std::env::var("AGENT_ROUNDS_TO_KEEP")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(DEFAULT_ROUNDS_TO_KEEP)
}

/// 从环境变量读取摘要保留轮数（`AGENT_SUMMARY_KEEP_ROUNDS`），默认 [`DEFAULT_SUMMARY_KEEP_ROUNDS`]。
fn summary_keep_rounds() -> usize {
    std::env::var("AGENT_SUMMARY_KEEP_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(DEFAULT_SUMMARY_KEEP_ROUNDS)
}

/// 压缩策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionStrategy {
    /// 截断模式：保留最近 N 轮，丢弃更早的（无 LLM 调用，快速）。
    Truncate,
    /// 摘要模式：用 LLM 将旧消息压缩为摘要，保留语义。
    Summarize,
}

/// 压缩操作的结果信息，用于日志和持久化。
#[derive(Debug, Clone)]
pub struct CompressionInfo {
    /// 是否发生了压缩
    pub did_compress: bool,
    /// 压缩前的消息数量
    pub original_messages: usize,
    /// 压缩后的消息数量
    pub after_messages: usize,
    /// 保留的对话轮数
    pub kept_rounds: usize,
    /// 压缩前的 token 数
    pub original_tokens: usize,
    /// 压缩后的 token 数
    pub after_tokens: usize,
    /// 使用的压缩策略
    pub strategy: Option<CompressionStrategy>,
}

/// 上下文压缩器。无状态，所有方法都是关联函数。
pub struct ContextCompressor;

impl ContextCompressor {
    /// 如果 `history.used_tokens` 超过阈值，保留最近 `rounds_to_keep()` 轮对话。
    /// 保留轮数由环境变量 `AGENT_ROUNDS_TO_KEEP` 配置（默认 [`DEFAULT_ROUNDS_TO_KEEP`]）。
    /// 返回 `CompressionInfo` 描述压缩详情。
    pub fn compress_if_needed(
        history: &mut ConversationHistory,
        max_tokens: usize,
    ) -> Result<CompressionInfo, AppError> {
        let threshold = (max_tokens as f64 * MAX_CONVERSATION_TOKENS_RATIO) as usize;

        if history.used_tokens < threshold {
            return Ok(Self::no_op(history));
        }

        Self::truncate(history)
    }

    /// 异步压缩：超过阈值时根据上下文压力等级选择策略。
    ///
    /// - `Warning`：使用 Summarize（LLM 摘要，保留语义；此时仍有空间，可承受一次 LLM 调用）
    /// - `Critical` / `Exhausted`：使用 Truncate（快速截断，释放空间优先；
    ///   调用方应配合 `auto_save_round_summary` 先把关键信息落盘）
    ///
    /// 未超过阈值时不压缩（no-op）。Summarize 失败时内部自动回退到 Truncate。
    pub async fn compress_if_needed_async(
        history: &mut ConversationHistory,
        max_tokens: usize,
        llm: &LlmClient,
        pressure: crate::agent::context::ContextPressure,
    ) -> Result<CompressionInfo, AppError> {
        let threshold = (max_tokens as f64 * MAX_CONVERSATION_TOKENS_RATIO) as usize;

        if history.used_tokens < threshold {
            return Ok(Self::no_op(history));
        }

        match pressure {
            crate::agent::context::ContextPressure::Warning => Self::summarize(history, llm).await,
            _ => Self::truncate(history),
        }
    }

    /// 截断压缩：保留最近 `rounds_to_keep()` 轮（环境变量 `AGENT_ROUNDS_TO_KEEP` 可配置），丢弃更早的。
    pub fn truncate(history: &mut ConversationHistory) -> Result<CompressionInfo, AppError> {
        let original_messages = history.messages.len();
        let original_tokens = history.used_tokens;
        let keep = rounds_to_keep();

        // Keep the last `keep` rounds of messages
        let mut rounds: Vec<Vec<LlmMessage>> = Vec::new();
        let mut current_round: Vec<LlmMessage> = Vec::new();

        for msg in history.messages.iter().rev() {
            if msg.role == "user" && !current_round.is_empty() {
                rounds.push(current_round);
                current_round = Vec::new();
                if rounds.len() >= keep {
                    break;
                }
            }
            current_round.push(msg.clone());
        }

        if !current_round.is_empty() && rounds.len() < keep {
            rounds.push(current_round);
        }

        // Rebuild history from kept rounds
        let mut new_messages: Vec<LlmMessage> = Vec::new();
        for round in rounds.iter().rev() {
            for msg in round.iter().rev() {
                new_messages.push(msg.clone());
            }
        }

        let after_messages = new_messages.len();
        history.messages = new_messages;
        history.recount_tokens();
        let after_tokens = history.used_tokens;

        Ok(CompressionInfo {
            did_compress: true,
            original_messages,
            after_messages,
            kept_rounds: keep,
            original_tokens,
            after_tokens,
            strategy: Some(CompressionStrategy::Truncate),
        })
    }

    /// 摘要压缩：用 LLM 将旧消息压缩为摘要，保留最近 `summary_keep_rounds()` 轮完整对话（环境变量 `AGENT_SUMMARY_KEEP_ROUNDS` 可配置）。
    ///
    /// `llm` 用于生成摘要。若 LLM 调用失败，则回退到截断压缩。
    ///
    /// **前置处理**：构建 old_text 之前，先把 role=="tool" 的消息内容降级为一行
    /// 结构化摘要（[tool:{name}] 首200字... ({N}B, {M}L)），避免 summarizer LLM
    /// 看到完整工具输出、浪费 token。降级发生在 join 之前，否则无意义。
    pub async fn summarize(
        history: &mut ConversationHistory,
        llm: &LlmClient,
    ) -> Result<CompressionInfo, AppError> {
        let original_messages = history.messages.len();
        let original_tokens = history.used_tokens;
        let keep = summary_keep_rounds();

        // 分离旧消息和最近保留的完整轮消息
        let (old_messages, _) = history.split_old_messages(keep);

        if old_messages.is_empty() {
            return Ok(Self::no_op(history));
        }

        // 前置：把 tool 消息内容降级为一行摘要
        let tool_name_map = build_tool_name_map(&old_messages);
        let old_messages: Vec<LlmMessage> = old_messages
            .into_iter()
            .map(|mut m| {
                if m.role == "tool" {
                    let name = m
                        .tool_call_id
                        .as_deref()
                        .and_then(|id| tool_name_map.get(id).map(|s| s.as_str()))
                        .unwrap_or("unknown");
                    let original = m.content.as_deref().unwrap_or("");
                    let degraded = summarize_tool_result(original, name);
                    m.content = Some(degraded);
                }
                m
            })
            .collect();

        // 构建摘要 prompt（使用纯文本 chat 调用，需要 LlmResponse）
        let old_text: Vec<String> = old_messages
            .iter()
            .map(|m| {
                let role_label = match m.role.as_str() {
                    "user" => "用户",
                    "assistant" => "助手",
                    "tool" => "工具",
                    _ => &m.role,
                };
                let content = m.content.as_deref().unwrap_or("");
                format!("【{}】{}", role_label, content)
            })
            .collect();
        let old_text = old_text.join("\n\n");

        let summarize_prompt = format!(
            "生成简洁中文摘要，保留：\n\
             - 已完成的关键步骤\n\
             - 重要决策及其理由\n\
             - 发现的问题或风险\n\
             - 待处理事项\n\
             摘要控制在 {} tokens 以内。\n\n\
             ---对话开始---\n{}\n---对话结束---\n\n摘要：",
            SUMMARY_MAX_TOKENS, old_text
        );

        // 调用 LLM 生成摘要（不带工具，纯文本）
        let summary = match llm
            .call(
                vec![
                    LlmMessage {
                        role: "system".to_string(),
                        content: Some("你是对话摘要助手。".to_string()),
                        tool_calls: None,
                        tool_call_id: None,
                    },
                    LlmMessage {
                        role: "user".to_string(),
                        content: Some(summarize_prompt.clone()),
                        tool_calls: None,
                        tool_call_id: None,
                    },
                ],
                Vec::new(),
            )
            .await
        {
            Ok(LlmResponse::Text(text)) => text.trim().to_string(),
            Ok(LlmResponse::ToolCalls(_)) => {
                // LLM 返回工具调用而非文本，回退到截断
                tracing::warn!("摘要 LLM 返回工具调用，回退到截断压缩");
                return Self::truncate(history);
            }
            Ok(LlmResponse::Error(e)) => {
                tracing::warn!("摘要 LLM 返回错误: {}，回退到截断压缩", e);
                return Self::truncate(history);
            }
            Err(e) => {
                tracing::warn!("摘要 LLM 调用失败: {}，回退到截断压缩", e);
                return Self::truncate(history);
            }
        };

        if summary.is_empty() {
            tracing::warn!("摘要为空，回退到截断压缩");
            return Self::truncate(history);
        }

        // 用摘要替换旧消息，保留最近 SUMMARY_KEEP_ROUNDS 轮完整消息
        history.replace_old_with_summary(&summary, keep);

        let after_tokens = history.used_tokens;
        Ok(CompressionInfo {
            did_compress: true,
            original_messages,
            after_messages: history.messages.len(),
            kept_rounds: keep,
            original_tokens,
            after_tokens,
            strategy: Some(CompressionStrategy::Summarize),
        })
    }

    /// 返回未压缩的 `CompressionInfo`。
    pub fn no_op(history: &ConversationHistory) -> CompressionInfo {
        CompressionInfo {
            did_compress: false,
            original_messages: history.messages.len(),
            after_messages: history.messages.len(),
            kept_rounds: rounds_to_keep(),
            original_tokens: history.used_tokens,
            after_tokens: history.used_tokens,
            strategy: None,
        }
    }
}

/// 从一串消息里构建 `tool_call_id → tool_name` 映射。
///
/// 遍历 assistant 消息的 tool_calls，每个 call 的 `id` → `function.name`。
/// 后续 tool 消息可以通过自己的 `tool_call_id` 查出对应的工具名。
fn build_tool_name_map(messages: &[LlmMessage]) -> std::collections::HashMap<String, String> {
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for m in messages {
        if m.role == "assistant" {
            if let Some(calls) = &m.tool_calls {
                for call in calls {
                    map.insert(call.id.clone(), call.function.name.clone());
                }
            }
        }
    }
    map
}

/// 把工具结果内容压缩为一行结构化摘要。
///
/// 格式：`[tool:{name}] {首200字}... ({N}B, {M}L)`
/// - 超过 200 字符时截断并加 `...`
/// - 空内容返回 `[tool:{name}] (empty)`
/// - 统计字节数和行数方便后续追查原始大小
fn summarize_tool_result(content: &str, tool_name: &str) -> String {
    if content.is_empty() {
        return format!("[tool:{}] (empty)", tool_name);
    }
    let total_bytes = content.len();
    let total_lines = content.lines().count();
    let prefix = if content.chars().count() > 200 {
        // 安全边界：按字符截断避免切半个 UTF-8 码点
        let cut = content
            .char_indices()
            .nth(200)
            .map(|(i, _)| i)
            .unwrap_or(content.len());
        format!("{}...", &content[..cut])
    } else {
        content.to_string()
    };
    format!("[tool:{}] {} ({}B, {}L)", tool_name, prefix, total_bytes, total_lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 环境变量操作全局互斥锁，防止并行测试间的环境变量竞态。
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn rounds_to_keep_defaults_to_six() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("AGENT_ROUNDS_TO_KEEP");
        assert_eq!(rounds_to_keep(), DEFAULT_ROUNDS_TO_KEEP);
    }

    #[test]
    fn rounds_to_keep_reads_env_var() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("AGENT_ROUNDS_TO_KEEP", "12");
        assert_eq!(rounds_to_keep(), 12);
        std::env::remove_var("AGENT_ROUNDS_TO_KEEP");
    }

    #[test]
    fn rounds_to_keep_clamps_invalid_values() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("AGENT_ROUNDS_TO_KEEP", "0");
        assert_eq!(rounds_to_keep(), DEFAULT_ROUNDS_TO_KEEP);
        std::env::set_var("AGENT_ROUNDS_TO_KEEP", "-1");
        assert_eq!(rounds_to_keep(), DEFAULT_ROUNDS_TO_KEEP);
        std::env::set_var("AGENT_ROUNDS_TO_KEEP", "abc");
        assert_eq!(rounds_to_keep(), DEFAULT_ROUNDS_TO_KEEP);
        std::env::remove_var("AGENT_ROUNDS_TO_KEEP");
    }

    #[test]
    fn summary_keep_rounds_defaults_to_three() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("AGENT_SUMMARY_KEEP_ROUNDS");
        assert_eq!(summary_keep_rounds(), DEFAULT_SUMMARY_KEEP_ROUNDS);
    }

    #[test]
    fn summary_keep_rounds_reads_env_var() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("AGENT_SUMMARY_KEEP_ROUNDS", "5");
        assert_eq!(summary_keep_rounds(), 5);
        std::env::remove_var("AGENT_SUMMARY_KEEP_ROUNDS");
    }

    // ── summarize_tool_result / build_tool_name_map 测试 ──────────────────

    #[test]
    fn summarize_tool_result_empty() {
        let out = summarize_tool_result("", "read_file");
        assert_eq!(out, "[tool:read_file] (empty)");
    }

    #[test]
    fn summarize_tool_result_short_content_kept_intact() {
        let short = "hello world";
        let out = summarize_tool_result(short, "exec_command");
        assert!(out.starts_with("[tool:exec_command] hello world"));
        // 短内容不应有省略号
        assert!(!out.contains("..."));
        // 结尾含字节/行数统计
        assert!(out.contains("B, 1L)"));
    }

    #[test]
    fn summarize_tool_result_long_content_truncates_to_200_chars() {
        let long = "x".repeat(500);
        let out = summarize_tool_result(&long, "exec_command");
        assert!(out.starts_with("[tool:exec_command] "));
        assert!(out.contains("...")); // 应被截断
        // 首段（不含 tool:name 前缀和结尾统计）最多 200 char + "..."
        // 完整输出应为格式："[tool:name] {200 chars}... ({500}B, 1L)"
        assert!(out.contains("500B"));
    }

    #[test]
    fn build_tool_name_map_collects_all_tool_call_ids() {
        use crate::llm::{ToolCall, ToolCallFunction};

        let calls = vec![
            ToolCall {
                id: "call_1".into(),
                function: ToolCallFunction {
                    name: "read_file".into(),
                    arguments: serde_json::json!({}),
                },
            },
            ToolCall {
                id: "call_2".into(),
                function: ToolCallFunction {
                    name: "exec_command".into(),
                    arguments: serde_json::json!({}),
                },
            },
        ];

        let messages = vec![LlmMessage {
            role: "assistant".into(),
            content: Some("calling tools".into()),
            tool_calls: Some(calls),
            tool_call_id: None,
        }];

        let map = build_tool_name_map(&messages);
        assert_eq!(map.get("call_1").map(|s| s.as_str()), Some("read_file"));
        assert_eq!(map.get("call_2").map(|s| s.as_str()), Some("exec_command"));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn build_tool_name_map_ignores_non_assistant_messages() {
        let messages = vec![
            LlmMessage {
                role: "user".into(),
                content: Some("do something".into()),
                tool_calls: None,
                tool_call_id: None,
            },
            LlmMessage {
                role: "tool".into(),
                content: Some("result".into()),
                tool_calls: None,
                tool_call_id: Some("call_1".into()),
            },
        ];
        let map = build_tool_name_map(&messages);
        assert!(map.is_empty());
    }
}
