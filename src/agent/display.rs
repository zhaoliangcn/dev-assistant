//! UI 展示消息缓冲区。
//!
//! 从 [`crate::agent::context::ContextManager`] 剥离的 UI 关注点：
//! - `display_messages`：纯展示消息（label, content），不参与 LLM 上下文
//! - `history_display_start`：当前 turn 在 history 中的起始索引
//!
//! 这些字段不属于对话上下文，只是 split-pane UI 渲染所需的状态。

use crate::utils::message_level::MessageLevel;

/// UI 展示缓冲区。持有 split-pane UI 渲染所需的瞬时状态。
#[derive(Debug, Clone, Default)]
pub struct DisplayBuffer {
    /// 纯展示消息列表 (label, content)，用于 split-pane UI 渲染。
    pub messages: Vec<(String, String)>,
    /// history 中当前 turn 的起始索引。
    pub history_start: usize,
}

impl DisplayBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// 清空所有展示消息，但保留 `history_start`。
    pub fn clear_messages(&mut self) {
        self.messages.clear();
    }

    /// 重置到新的 turn 起点：清空展示消息并把 `history_start` 推到 `new_start`。
    pub fn reset_turn(&mut self, new_start: usize) {
        self.messages.clear();
        self.history_start = new_start;
    }

    /// 添加一条纯展示消息。
    pub fn add(&mut self, level: MessageLevel, msg: &str) {
        self.messages.push((level.label().to_string(), msg.to_string()));
    }
}
