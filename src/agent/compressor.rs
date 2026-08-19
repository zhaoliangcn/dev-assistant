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
            crate::agent::context::ContextPressure::Warning => {
                Self::summarize(history, llm).await
            }
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
            "请对以下对话生成一个简洁的中文摘要，必须保留：\n\
             - 已完成的关键步骤\n\
             - 重要决策及其理由\n\
             - 发现的问题或风险\n\
             - 待处理事项\n\
             不要包含无关细节。摘要控制在 {} tokens 以内。\n\n\
             ---对话开始---\n{}\n---对话结束---\n\n摘要：",
            SUMMARY_MAX_TOKENS, old_text
        );

        // 调用 LLM 生成摘要（不带工具，纯文本）
        let summary = match llm
            .call(
                vec![LlmMessage {
                    role: "system".to_string(),
                    content: Some("你是一个高效的对话摘要助手。".to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                },
                LlmMessage {
                    role: "user".to_string(),
                    content: Some(summarize_prompt.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                }],
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
        history.replace_old_with_summary(&summary);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_to_keep_defaults_to_six() {
        // 无环境变量时返回默认值 6
        std::env::remove_var("AGENT_ROUNDS_TO_KEEP");
        assert_eq!(rounds_to_keep(), DEFAULT_ROUNDS_TO_KEEP);
    }

    #[test]
    fn rounds_to_keep_reads_env_var() {
        std::env::set_var("AGENT_ROUNDS_TO_KEEP", "12");
        assert_eq!(rounds_to_keep(), 12);
        std::env::remove_var("AGENT_ROUNDS_TO_KEEP");
    }

    #[test]
    fn rounds_to_keep_clamps_invalid_values() {
        // 0 和负数无效，回退到默认值
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
        std::env::remove_var("AGENT_SUMMARY_KEEP_ROUNDS");
        assert_eq!(summary_keep_rounds(), DEFAULT_SUMMARY_KEEP_ROUNDS);
    }

    #[test]
    fn summary_keep_rounds_reads_env_var() {
        std::env::set_var("AGENT_SUMMARY_KEEP_ROUNDS", "5");
        assert_eq!(summary_keep_rounds(), 5);
        std::env::remove_var("AGENT_SUMMARY_KEEP_ROUNDS");
    }
}
