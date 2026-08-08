//! 对话历史管理。
//!
//! 从 [`crate::agent::context::ContextManager`] 剥离的 history 关注点：
//! - 消息存储与追加
//! - 系统提示词前置
//! - token 累计

use crate::agent::context::Role;
use crate::agent::token_counter::TokenCounter;
use crate::llm::{LlmMessage, ToolCall};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// 对话历史。持有消息列表和累计 token 数。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConversationHistory {
    pub messages: Vec<LlmMessage>,
    pub system_prompt: String,
    pub used_tokens: usize,
}

impl ConversationHistory {
    pub fn new(system_prompt: String) -> Self {
        let used_tokens = TokenCounter::estimate(&system_prompt);
        Self {
            messages: Vec::new(),
            system_prompt,
            used_tokens,
        }
    }

    /// 构建发送给 LLM 的完整消息列表：system + history。
    pub fn build_messages(&self) -> Vec<LlmMessage> {
        let mut messages = Vec::new();

        messages.push(LlmMessage {
            role: "system".to_string(),
            content: Some(self.system_prompt.clone()),
            tool_calls: None,
            tool_call_id: None,
        });

        messages.extend(self.messages.clone());
        messages
    }

    pub fn add_message(
        &mut self,
        role: Role,
        content: String,
        tool_calls: Option<Vec<ToolCall>>,
        tool_call_id: Option<String>,
    ) {
        debug!(role = ?role, len = content.len(), "Adding message to context");
        let role_str: String = role.into();

        // 在移动 tool_calls 之前先统计其 JSON 的 token
        let tool_calls_tokens = if let Some(ref calls) = tool_calls {
            serde_json::to_string(calls)
                .map(|json| TokenCounter::estimate(&json))
                .unwrap_or(0)
        } else {
            0
        };

        let message = LlmMessage {
            role: role_str,
            content: Some(content.clone()),
            tool_calls,
            tool_call_id,
        };

        self.messages.push(message);
        self.used_tokens += TokenCounter::estimate(&content) + tool_calls_tokens;
    }

    pub fn add_tool_result(&mut self, tool_call: &ToolCall, result: &str) {
        self.add_message(
            Role::Tool,
            result.to_string(),
            None,
            Some(tool_call.id.clone()),
        );
    }

    /// 重新计算 token 总数（压缩后调用）。
    pub fn recount_tokens(&mut self) {
        self.used_tokens = TokenCounter::estimate_messages(&self.messages);
    }

    /// 当前消息数量。
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// 分离出需要摘要的旧消息（除最近 `keep_rounds` 轮之外的所有消息）。
    ///
    /// 返回 `(old_messages, new_messages)`：
    /// - `old_messages`：应被摘要替换的较旧消息（保持时间顺序）
    /// - `new_messages`：应保留完整的最近 `keep_rounds` 轮消息
    ///
    /// 轮（round）的划分：以 user 消息为界，一个 user + 若干 assistant/tool 消息为一轮。
    pub fn split_old_messages(&self, keep_rounds: usize) -> (Vec<LlmMessage>, Vec<LlmMessage>) {
        let mut rounds: Vec<Vec<LlmMessage>> = Vec::new();
        let mut current_round: Vec<LlmMessage> = Vec::new();

        // 从后往前遍历，收集最近的 keep_rounds 轮
        for msg in self.messages.iter().rev() {
            if msg.role == "user" && !current_round.is_empty() {
                rounds.push(std::mem::take(&mut current_round));
                if rounds.len() >= keep_rounds {
                    break;
                }
            }
            current_round.push(msg.clone());
        }
        if !current_round.is_empty() && rounds.len() < keep_rounds {
            rounds.push(current_round);
        }

        // rounds 是倒序的（最新的在前），翻转成时间顺序
        rounds.reverse();

        // 重建 new_messages（时间顺序）
        let mut new_messages: Vec<LlmMessage> = Vec::new();
        for round in &rounds {
            // 每轮内部也是倒序压入的，需要翻转
            for msg in round.iter().rev() {
                new_messages.push(msg.clone());
            }
        }

        // old = 全部 - new
        let old_count = self.messages.len() - new_messages.len();
        let old_messages: Vec<LlmMessage> = self.messages[..old_count].to_vec();

        (old_messages, new_messages)
    }

    /// 用一条摘要消息替换旧消息，保留最近 `keep_rounds` 轮完整消息。
    ///
    /// 结构：`[摘要消息] + [最近 keep_rounds 轮消息]`
    /// 摘要消息以 `system` 角色插入，标注为「对话摘要」。
    pub fn replace_old_with_summary(&mut self, summary: &str) {
        let keep_rounds = 6usize;
        let (_, mut new_messages) = self.split_old_messages(keep_rounds);

        let mut messages = Vec::new();
        messages.push(LlmMessage {
            role: "system".to_string(),
            content: Some(format!("【对话摘要】\n{}", summary)),
            tool_calls: None,
            tool_call_id: None,
        });
        messages.append(&mut new_messages);

        self.messages = messages;
        self.recount_tokens();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_counts_system_prompt_tokens() {
        let system_prompt = "You are a helpful assistant.".to_string();
        let history = ConversationHistory::new(system_prompt);
        assert!(history.used_tokens > 0, "system prompt should contribute to token count");
    }
}
