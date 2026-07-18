use crate::llm::{LlmMessage, ToolCall};
use crate::utils::error::AppError;
use crate::utils::message_level::MessageLevel;
use serde::{Deserialize, Serialize};
use tracing::debug;

#[derive(Debug, Clone)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl From<Role> for String {
    fn from(r: Role) -> String {
        match r {
            Role::System => "system".to_string(),
            Role::User => "user".to_string(),
            Role::Assistant => "assistant".to_string(),
            Role::Tool => "tool".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// 上下文管理器
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContextManager {
    pub history: Vec<LlmMessage>,
    /// 纯展示消息列表 (label, content)，用于 split-pane UI 渲染，不参与 LLM 上下文。
    pub display_messages: Vec<(String, String)>,
    /// history 中当前 turn 的起始索引，get_display_messages 只显示从该位置开始的消息。
    pub history_display_start: usize,
    pub system_prompt: String,
    pub max_tokens: usize,
    pub used_tokens: usize,
    pub consecutive_no_tool_rounds: usize,
    /// 持久化的活跃模型名称，重启后恢复
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_model: Option<String>,
}

const ROUNDS_TO_KEEP: usize = 6;
const MAX_CONVERSATION_TOKENS_RATIO: f64 = 0.9;

impl ContextManager {
    pub fn new(system_prompt: String, max_tokens: usize) -> Self {
        Self {
            history: Vec::new(),
            display_messages: Vec::new(),
            history_display_start: 0,
            system_prompt,
            max_tokens,
            used_tokens: 0,
            consecutive_no_tool_rounds: 0,
            active_model: None,
        }
    }

    #[allow(dead_code)]
    pub fn estimate_token_usage(&self) -> usize {
        self.used_tokens
    }

    /// 添加一条纯展示消息，用于在 UI 中显示。此消息不会发送给 LLM。
    pub fn add_display_message(&mut self, level: MessageLevel, msg: &str) {
        debug!(level = ?level, len = msg.len(), "Adding display message");
        self.display_messages.push((level.label().to_string(), msg.to_string()));
    }

    /// Extract conversation messages from history for the UI.
    /// Returns Vec of (role_label, content) in chronological order.
    /// Skips system messages and messages already shown via display_messages.
    pub fn get_display_messages(&self) -> Vec<(String, String)> {
        // Build a set of content strings already present in display_messages
        let display_contents: std::collections::HashSet<String> = self
            .display_messages
            .iter()
            .map(|(_, content)| content.clone())
            .collect();

        let mut result: Vec<(String, String)> = Vec::new();
        for msg in &self.history[self.history_display_start..] {
            if msg.role == "system" {
                continue;
            }
            let content = msg.content.as_deref().unwrap_or("").to_string();
            if display_contents.contains(&content) {
                continue; // 已通过 display_messages 显示，不再重复
            }
            let role = match msg.role.as_str() {
                "user" => "▸ 你".to_string(),
                "assistant" => "◂ 助手".to_string(),
                "tool" => "⚙ 工具".to_string(),
                other => other.to_string(),
            };
            // 跳过连续重复的相同 (role, content) 消息
            if result.last() == Some(&(role.clone(), content.clone())) {
                continue;
            }
            result.push((role, content));
        }

        result
    }

    /// Save the conversation state to a JSON file.
    pub fn save_state(&self, path: &std::path::Path) -> Result<(), AppError> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| AppError::Config(format!("Failed to serialize state: {}", e)))?;
        std::fs::write(path, json).map_err(AppError::Io)?;
        Ok(())
    }

    /// Load conversation state from a JSON file.
    pub fn load_state(path: &std::path::Path) -> Result<Self, AppError> {
        let json = std::fs::read_to_string(path).map_err(AppError::Io)?;
        let ctx = serde_json::from_str(&json)
            .map_err(|e| AppError::Config(format!("Failed to deserialize state: {}", e)))?;
        Ok(ctx)
    }

    pub fn build_messages(&self) -> Vec<LlmMessage> {
        let mut messages = Vec::new();

        messages.push(LlmMessage {
            role: "system".to_string(),
            content: Some(self.system_prompt.clone()),
            tool_calls: None,
            tool_call_id: None,
        });

        messages.extend(self.history.clone());

        messages
    }

    /// Better-than-before token estimation without extra dependencies.
    /// CJK characters are ~2 tokens each; non-CJK words are ~0.75 tokens per word.
    pub fn estimate_tokens(text: &str) -> usize {
        let mut tokens = 0usize;

        for word in text.split_whitespace() {
            let is_cjk = word.chars().any(|c| {
                matches!(c,
                    '\u{4E00}'..='\u{9FFF}' |
                    '\u{3400}'..='\u{4DBF}' |
                    '\u{3000}'..='\u{303F}' |
                    '\u{3040}'..='\u{309F}' |
                    '\u{30A0}'..='\u{30FF}' |
                    '\u{AC00}'..='\u{D7AF}'
                )
            });

            if is_cjk {
                tokens += word.chars().count() * 2;
            } else {
                tokens += (word.chars().count() as f64 * 0.75).ceil() as usize;
            }
        }

        tokens.max(1)
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

        self.history.push(message);
        self.used_tokens += Self::estimate_tokens(&content);
    }

    pub fn add_tool_result(&mut self, tool_call: &ToolCall, result: &str) {
        self.add_message(
            Role::Tool,
            result.to_string(),
            None,
            Some(tool_call.id.clone()),
        );
    }

    pub fn increment_no_tool_rounds(&mut self) {
        self.consecutive_no_tool_rounds += 1;
    }

    pub fn reset_no_tool_rounds(&mut self) {
        self.consecutive_no_tool_rounds = 0;
    }

    pub fn get_consecutive_no_tool_rounds(&self) -> usize {
        self.consecutive_no_tool_rounds
    }

    /// Keep only the last N rounds of conversation. A round consists of
    /// consecutive non-system messages. This preserves the system prompt
    /// at all times and retains the most recent context.
    pub async fn compress(&mut self) -> Result<(), AppError> {
        if self.used_tokens < (self.max_tokens as f64 * MAX_CONVERSATION_TOKENS_RATIO) as usize {
            return Ok(());
        }

        // Keep the last ROUNDS_TO_KEEP rounds of messages
        let mut rounds: Vec<Vec<LlmMessage>> = Vec::new();
        let mut current_round: Vec<LlmMessage> = Vec::new();

        for msg in self.history.iter().rev() {
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
        let mut new_history: Vec<LlmMessage> = Vec::new();
        for round in rounds.iter().rev() {
            for msg in round.iter().rev() {
                new_history.push(msg.clone());
            }
        }

        self.history = new_history;
        self.used_tokens = self
            .history
            .iter()
            .map(|m| Self::estimate_tokens(m.content.as_deref().unwrap_or("")))
            .sum();

        Ok(())
    }
}
