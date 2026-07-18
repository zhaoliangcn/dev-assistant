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

/// 上下文压缩器。无状态，所有方法都是关联函数。
pub struct ContextCompressor;

impl ContextCompressor {
    /// 如果 `history.used_tokens` 超过阈值，保留最近 `ROUNDS_TO_KEEP` 轮对话。
    /// 返回 true 表示发生了压缩。
    pub fn compress_if_needed(
        history: &mut ConversationHistory,
        max_tokens: usize,
    ) -> Result<bool, AppError> {
        let threshold = (max_tokens as f64 * MAX_CONVERSATION_TOKENS_RATIO) as usize;
        if history.used_tokens < threshold {
            return Ok(false);
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

        history.messages = new_messages;
        history.recount_tokens();

        Ok(true)
    }
}
