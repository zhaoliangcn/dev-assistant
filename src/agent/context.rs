use crate::agent::compressor::{CompressionInfo, ContextCompressor};
use crate::agent::display::DisplayBuffer;
use crate::agent::history::ConversationHistory;
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
// 上下文管理器（薄协调层）
// ---------------------------------------------------------------------------

/// 上下文管理器。
///
/// 这是一个薄协调层，实际职责分散到：
/// - [`ConversationHistory`]：消息存储与 token 累计
/// - [`TokenCounter`]：token 估算
/// - [`ContextCompressor`]：上下文压缩
/// - [`DisplayBuffer`]：UI 展示缓冲区
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContextManager {
    #[serde(flatten)]
    pub history: ConversationHistory,
    pub max_tokens: usize,
    pub consecutive_no_tool_rounds: usize,
    /// 持久化的活跃模型名称，重启后恢复
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_model: Option<String>,
    /// UI 展示缓冲区（不参与 LLM 上下文，也不序列化）
    #[serde(skip)]
    pub display: DisplayBuffer,
}

impl ContextManager {
    pub fn new(system_prompt: String, max_tokens: usize) -> Self {
        Self {
            history: ConversationHistory::new(system_prompt),
            max_tokens,
            consecutive_no_tool_rounds: 0,
            active_model: None,
            display: DisplayBuffer::new(),
        }
    }

    #[allow(dead_code)] // reserved for future token budget management
    pub fn estimate_token_usage(&self) -> usize {
        self.history.used_tokens
    }

    /// 添加一条纯展示消息，用于在 UI 中显示。此消息不会发送给 LLM。
    pub fn add_display_message(&mut self, level: MessageLevel, msg: &str) {
        debug!(level = ?level, len = msg.len(), "Adding display message");
        self.display.add(level, msg);
    }

    /// Extract conversation messages from history for the UI.
    /// Returns Vec of (role_label, content) in chronological order.
    /// Skips system messages and messages already shown via display buffer.
    pub fn get_display_messages(&self) -> Vec<(String, String)> {
        // Build a set of content strings already present in display buffer
        let display_contents: std::collections::HashSet<String> = self
            .display
            .messages
            .iter()
            .map(|(_, content): &(_, String)| content.clone())
            .collect();

        let mut result: Vec<(String, String)> = Vec::new();
        // 确保 history_start 在有效范围内，防止越界访问
        let history_start = self.display.history_start.min(self.history.messages.len());
        for msg in &self.history.messages[history_start..] {
            if msg.role == "system" {
                continue;
            }
            let content = msg.content.as_deref().unwrap_or("").to_string();
            if display_contents.contains(&content) {
                continue; // 已通过 display buffer 显示，不再重复
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

    // ----- 委托给 ConversationHistory -----

    pub fn build_messages(&self) -> Vec<LlmMessage> {
        self.history.build_messages()
    }

    pub fn add_message(
        &mut self,
        role: Role,
        content: String,
        tool_calls: Option<Vec<ToolCall>>,
        tool_call_id: Option<String>,
    ) {
        self.history.add_message(role, content, tool_calls, tool_call_id);
    }

    pub fn add_tool_result(&mut self, tool_call: &ToolCall, result: &str) {
        self.history.add_tool_result(tool_call, result);
    }

    // ----- 便捷访问器 -----

    pub fn increment_no_tool_rounds(&mut self) {
        self.consecutive_no_tool_rounds += 1;
    }

    pub fn reset_no_tool_rounds(&mut self) {
        self.consecutive_no_tool_rounds = 0;
    }

    pub fn get_consecutive_no_tool_rounds(&self) -> usize {
        self.consecutive_no_tool_rounds
    }

    /// 压缩上下文：委托给 [`ContextCompressor`]。
    /// 返回 [`CompressionInfo`] 描述压缩详情。
    pub fn compress(&mut self) -> Result<CompressionInfo, AppError> {
        ContextCompressor::compress_if_needed(&mut self.history, self.max_tokens)
    }
}
