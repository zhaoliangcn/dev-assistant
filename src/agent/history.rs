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
        let message = LlmMessage {
            role: role_str,
            content: Some(content.clone()),
            tool_calls,
            tool_call_id,
        };

        self.messages.push(message);
        self.used_tokens += TokenCounter::estimate(&content);
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
