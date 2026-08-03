//! 实时输出：子代理的工具调用和响应信息实时渲染到终端。
//!
//! 与 `UIMessageOutput` 不同，此实现会在消息产生时立即渲染到终端，
//! 而不是等到子代理完成后再批量处理。这样用户可以在主代理等待子代理
//! 完成时，实时看到子代理的工具调用和响应。

use crate::utils::message_level::MessageLevel;
use crate::utils::message_output::MessageOutput;
use crate::ui::{self, MarkdownRenderer, MessageBlock};

/// 实时输出：子代理运行期间的消息实时渲染到终端。
///
/// # 功能
///
/// - 消息产生时立即渲染到终端（实时可见）
/// - 按子代理深度缩进，层级关系一目了然
/// - 标记子代理类型，区分不同角色的输出
/// - 同时缓冲消息，供子代理完成后统一处理
pub struct RealtimeOutput {
    verbose: bool,
    buffer: Vec<(MessageLevel, String)>,
    markdown_renderer: MarkdownRenderer,
    /// 子代理深度（用于缩进）
    pub depth: usize,
    /// 子代理类型名称
    pub agent_type: String,
}

impl RealtimeOutput {
    pub fn new(verbose: bool, depth: usize, agent_type: &str) -> Self {
        Self {
            verbose,
            buffer: Vec::new(),
            markdown_renderer: MarkdownRenderer::new(),
            depth,
            agent_type: agent_type.to_string(),
        }
    }

    /// 取出所有暂存的消息并清空缓冲区。
    pub fn drain(&mut self) -> Vec<(MessageLevel, String)> {
        std::mem::take(&mut self.buffer)
    }

    /// 将消息实时渲染到终端，带子代理标识和深度缩进。
    fn render_realtime(&self, level: MessageLevel, msg: &str) {
        // 深度缩进：每层 2 空格
        let indent = "  ".repeat(self.depth);
        let tag = format!("[{}]", self.agent_type);

        let block: MessageBlock = match level {
            MessageLevel::Info => {
                if msg.starts_with("🔧") {
                    // 工具调用：突出显示
                    MessageBlock::System {
                        content: format!("{}🔧 {} {}", indent, tag, msg),
                    }
                } else if msg.starts_with("↻") {
                    // 轮次信息
                    MessageBlock::System {
                        content: format!("{}🔄 {} {}", indent, tag, msg),
                    }
                } else if msg.starts_with("✅") {
                    MessageBlock::ToolResult {
                        tool_name: format!("子代理 {}", self.agent_type),
                        success: true,
                        content: format!("{}{}", indent, msg),
                    }
                } else if msg.starts_with("❌") || msg.starts_with("🔥") {
                    MessageBlock::Error {
                        content: format!("{}{}", indent, msg),
                    }
                } else if msg.starts_with("💭") {
                    MessageBlock::Thinking {
                        content: format!("{}💭 {} {}", indent, tag, msg),
                    }
                } else {
                    MessageBlock::System {
                        content: format!("{}ℹ️ {} {}", indent, tag, msg),
                    }
                }
            }
            MessageLevel::Success => MessageBlock::ToolResult {
                tool_name: format!("子代理 {}", self.agent_type),
                success: true,
                content: format!("{}{}", indent, msg),
            },
            MessageLevel::Error => MessageBlock::Error {
                content: format!("{}{}", indent, msg),
            },
            MessageLevel::Warning => MessageBlock::System {
                content: format!("{}⚠️ {} {}", indent, tag, msg),
            },
            MessageLevel::Debug => {
                if self.verbose {
                    MessageBlock::System {
                        content: format!("{}🐛 {} {}", indent, tag, msg),
                    }
                } else {
                    return;
                }
            }
        };

        let _ = ui::render_block(&block, &self.markdown_renderer);
    }
}

impl MessageOutput for RealtimeOutput {
    fn emit(&mut self, level: MessageLevel, msg: &str) {
        // 非 verbose 模式下跳过 Debug 级别消息
        if !self.verbose && matches!(level, MessageLevel::Debug) {
            return;
        }

        // 实时渲染到终端
        self.render_realtime(level, msg);

        // 同时缓冲消息，供子代理完成后统一处理
        let entry = (level, msg.to_string());
        if self.buffer.last() != Some(&entry) {
            self.buffer.push(entry);
        }
    }

    /// 子代理的流式助手内容：最终结果实时渲染。
    fn streaming_assistant(&mut self, content: &str, is_final: bool) {
        if is_final && !content.is_empty() {
            let indent = "  ".repeat(self.depth);
            let tag = format!("[{}]", self.agent_type);
            let block = MessageBlock::Assistant {
                content: format!("{}📋 {} 结果:\n{}", indent, tag, content),
                
            };
            let _ = ui::render_block(&block, &self.markdown_renderer);
        }
    }
}