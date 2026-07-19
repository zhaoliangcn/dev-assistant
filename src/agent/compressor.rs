//! 上下文压缩策略。
//!
//! 当 token 数超过阈值时，只保留最近 N 轮对话，防止上下文无限增长。

use crate::agent::history::ConversationHistory;
use crate::llm::LlmMessage;
use crate::utils::error::AppError;

/// 保留的对话轮数。
const ROUNDS_TO_KEEP: usize = 6;

/// 触发压缩的 token 阈值比例（相对于 max_tokens）。
const MAX_CONVERSATION_TOKENS_RATIO: f64 = 0.9;

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
}

/// 上下文压缩器。无状态，所有方法都是关联函数。
pub struct ContextCompressor;

impl ContextCompressor {
    /// 如果 `history.used_tokens` 超过阈值，保留最近 `ROUNDS_TO_KEEP` 轮对话。
    /// 返回 `CompressionInfo` 描述压缩详情。
    pub fn compress_if_needed(
        history: &mut ConversationHistory,
        max_tokens: usize,
    ) -> Result<CompressionInfo, AppError> {
        let threshold = (max_tokens as f64 * MAX_CONVERSATION_TOKENS_RATIO) as usize;
        let original_messages = history.messages.len();
        let original_tokens = history.used_tokens;

        if history.used_tokens < threshold {
            return Ok(CompressionInfo {
                did_compress: false,
                original_messages,
                after_messages: original_messages,
                kept_rounds: ROUNDS_TO_KEEP,
                original_tokens,
                after_tokens: original_tokens,
            });
        }

        // Keep the last ROUNDS_TO_KEEP rounds of messages
        let mut rounds: Vec<Vec<LlmMessage>> = Vec::new();
        let mut current_round: Vec<LlmMessage> = Vec::new();

        for msg in history.messages.iter().rev() {
            if msg.role == "user" && !current_round.is_empty() {
                rounds.push(current_round);
                current_round = Vec::new();
                if rounds.len() >= ROUNDS_TO_KEEP {
                    break;
                }
            }
            current_round.push(msg.clone());
        }

        if !current_round.is_empty() && rounds.len() < ROUNDS_TO_KEEP {
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
            kept_rounds: ROUNDS_TO_KEEP,
            original_tokens,
            after_tokens,
        })
    }
}
