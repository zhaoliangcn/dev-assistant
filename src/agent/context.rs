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
// 上下文预算管理
// ---------------------------------------------------------------------------

/// 上下文压力等级。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum ContextPressure {
    /// 充足 (> 40% 剩余)
    Normal,
    /// 注意 (20% ~ 40% 剩余)
    Warning,
    /// 紧张 (10% ~ 20% 剩余)
    Critical,
    /// 即将溢出 (< 10% 剩余)
    Exhausted,
}

/// 上下文预算报告。
///
/// 告诉 Agent 当前上下文的使用情况，供其主动管理。
#[derive(Debug, Clone, Serialize)]
pub struct ContextBudget {
    /// 系统提示词占用的 tokens
    pub system_prompt_tokens: usize,
    /// 从 KB 注入的记忆占用的 tokens
    pub memory_tokens: usize,
    /// 对话历史占用的 tokens
    pub history_tokens: usize,
    /// 总使用量
    pub total_tokens: usize,
    /// 最大允许 tokens
    pub max_tokens: usize,
    /// 使用率百分比 (0.0 ~ 1.0)
    pub utilization: f64,
    /// 估算剩余可用 tokens
    pub estimated_room: usize,
    /// 上下文压力等级
    pub pressure: ContextPressure,
    /// 工具 schema 占用的 tokens
    #[serde(default)]
    pub tool_schema_tokens: usize,
}

/// 上下文预算管理器。
///
/// 跟踪和管理上下文预算，提供报告生成和预警功能。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBudgetManager {
    /// 最大允许 tokens
    pub max_tokens: usize,
    /// 系统提示词占用的 tokens
    pub system_prompt_tokens: usize,
    /// 从 KB 注入的记忆占用的 tokens
    pub memory_tokens: usize,
    /// 工具 schema 占用的 tokens
    pub tool_schema_tokens: usize,
    /// 预警阈值：当达到此使用率时触发预警
    #[serde(default = "default_warning_threshold")]
    pub warning_threshold: f64,
    /// 关键阈值：当达到此使用率时触发关键预警
    #[serde(default = "default_critical_threshold")]
    pub critical_threshold: f64,
}

fn default_warning_threshold() -> f64 { 0.60 }
fn default_critical_threshold() -> f64 { 0.80 }

impl Default for ContextBudgetManager {
    fn default() -> Self {
        Self {
            max_tokens: 8192,
            system_prompt_tokens: 0,
            memory_tokens: 0,
            tool_schema_tokens: 0,
            warning_threshold: 0.60,
            critical_threshold: 0.80,
        }
    }
}

impl ContextBudgetManager {
    /// 创建一个新的预算管理器。
    pub fn new(max_tokens: usize, system_prompt_tokens: usize) -> Self {
        Self {
            max_tokens,
            system_prompt_tokens,
            memory_tokens: 0,
            tool_schema_tokens: 0,
            warning_threshold: 0.60,
            critical_threshold: 0.80,
        }
    }

    /// 生成当前上下文预算报告。
    pub fn report(&self, history: &ConversationHistory) -> ContextBudget {
        let total = history.used_tokens;
        let history_tokens = total.saturating_sub(
            self.system_prompt_tokens + self.memory_tokens + self.tool_schema_tokens
        );
        let utilization = if self.max_tokens > 0 {
            total as f64 / self.max_tokens as f64
        } else {
            0.0
        };
        let estimated_room = self.max_tokens.saturating_sub(total);

        let pressure = if utilization > 0.90 {
            ContextPressure::Exhausted
        } else if utilization > self.critical_threshold {
            ContextPressure::Critical
        } else if utilization > self.warning_threshold {
            ContextPressure::Warning
        } else {
            ContextPressure::Normal
        };

        ContextBudget {
            system_prompt_tokens: self.system_prompt_tokens,
            memory_tokens: self.memory_tokens,
            history_tokens,
            total_tokens: total,
            max_tokens: self.max_tokens,
            utilization,
            estimated_room,
            pressure,
            tool_schema_tokens: self.tool_schema_tokens,
        }
    }

    /// 检查是否需要压缩（基于压力等级）。
    #[allow(dead_code)] // reserved for automatic compression triggering
    pub fn should_compress(&self, history: &ConversationHistory) -> bool {
        let report = self.report(history);
        matches!(report.pressure,
            ContextPressure::Critical | ContextPressure::Exhausted
        )
    }

    /// 设置记忆 tokens 数。
    #[allow(dead_code)] // reserved for future KB memory quota management
    pub fn set_memory_tokens(&mut self, tokens: usize) {
        self.memory_tokens = tokens;
    }

    /// 设置工具 schema tokens 数。
    pub fn set_tool_schema_tokens(&mut self, tokens: usize) {
        self.tool_schema_tokens = tokens;
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
/// - [`ContextBudgetManager`]：上下文预算管理
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
    /// 上下文预算管理器
    pub budget_manager: ContextBudgetManager,
    /// 会话 ID（用于分层摘要存储定位，如 `.kb/summaries/{session_id}/`）。
    #[serde(default = "default_session_id")]
    pub session_id: String,
}

fn default_session_id() -> String {
    "default".to_string()
}

impl ContextManager {
    pub fn new(system_prompt: String, max_tokens: usize) -> Self {
        let system_prompt_tokens = crate::agent::token_counter::TokenCounter::estimate(&system_prompt);
        Self {
            history: ConversationHistory::new(system_prompt),
            max_tokens,
            consecutive_no_tool_rounds: 0,
            active_model: None,
            display: DisplayBuffer::new(),
            budget_manager: ContextBudgetManager::new(max_tokens, system_prompt_tokens),
            session_id: "default".to_string(),
        }
    }

    #[allow(dead_code)] // reserved for future token budget management
    pub fn estimate_token_usage(&self) -> usize {
        self.history.used_tokens
    }

    /// 获取当前上下文预算报告。
    ///
    /// 用于 `context_budget` 工具，让 Agent 能感知自己的上下文使用情况。
    pub fn get_budget_report(&self) -> ContextBudget {
        self.budget_manager.report(&self.history)
    }

    /// 设置工具 schema 占用的 tokens（用于预算计算）。
    pub fn set_tool_schema_tokens(&mut self, tokens: usize) {
        self.budget_manager.set_tool_schema_tokens(tokens);
    }

    /// 设置记忆（KB 注入）占用的 tokens（用于预算计算）。
    #[allow(dead_code)] // reserved for future KB memory quota management
    pub fn set_memory_tokens(&mut self, tokens: usize) {
        self.budget_manager.set_memory_tokens(tokens);
    }

    /// 将当前上下文预算报告格式化为 JSON 字符串（用于工具返回）。
    pub fn budget_report_json(&self) -> String {
        serde_json::to_string_pretty(&self.get_budget_report())
            .unwrap_or_else(|_| "{}".to_string())
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
