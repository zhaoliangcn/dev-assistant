use crate::utils::message_output::MessageOutput;
use crate::utils::message_level::MessageLevel;

// ---------------------------------------------------------------------------
// 交互模式：消息暂存到缓冲区，run() 返回后再写入 ContextManager
// ---------------------------------------------------------------------------

/// 交互 REPL 模式下的消息输出。消息先暂存到内部缓冲区，
/// 待 `agent.run()` 返回后由调用方统一写入 `ContextManager`。
/// 这样避免了在 `agent.run()` 执行期间同时持有 `&mut agent` 和
/// `&mut agent.context` 的双重可变借用问题。
pub struct UIMessageOutput {
    verbose: bool,
    buffer: Vec<(MessageLevel, String)>,
    /// 上一次流式渲染的助手内容，用于跳过重复渲染
    last_streamed_content: String,
}

impl UIMessageOutput {
    pub fn new(verbose: bool) -> Self {
        Self {
            verbose,
            buffer: Vec::new(),
            last_streamed_content: String::new(),
        }
    }

    /// 取出所有暂存的消息并清空缓冲区。
    pub fn drain(&mut self) -> Vec<(MessageLevel, String)> {
        std::mem::take(&mut self.buffer)
    }

    /// 获取最后一条消息的内容（不消费缓冲区）。
    /// 用于在下一轮 step 前生成上下文状态提示。
    pub fn last_message(&self) -> Option<&str> {
        self.buffer.last().map(|(_, msg)| msg.as_str())
    }

    /// 获取最后一条消息的级别。
    #[allow(dead_code)]
    pub fn last_level(&self) -> Option<MessageLevel> {
        self.buffer.last().map(|(level, _)| *level)
    }
}

impl MessageOutput for UIMessageOutput {
    fn emit(&mut self, level: MessageLevel, msg: &str) {
        // 非 verbose 模式下跳过 Debug 和 Info 级别消息
        if !self.verbose && matches!(level, MessageLevel::Debug | MessageLevel::Info) {
            return;
        }
        // 去重：避免相同 (level, msg) 连续出现
        let entry = (level, msg.to_string());
        if self.buffer.last() != Some(&entry) {
            self.buffer.push(entry);
        }
    }

    /// 流式输出助手消息，直接渲染到终端。
    ///
    /// 每次调用时，使用 ANSI 控制序列覆盖当前行，显示累积的助手内容。
    /// 完成后（is_final=true），输出完整内容并移除闪烁光标。
    fn streaming_assistant(&mut self, content: &str, is_final: bool) {
        use std::io::Write;
        use unicode_width::UnicodeWidthStr;

        let mut stdout = std::io::stdout();
        if is_final {
            // 清除可能因终端自动换行产生的残留行，再输出最终内容
            let term_width = crate::ui::get_terminal_width().unwrap_or(80);
            let prefix = "🤖 助手: ";
            // 计算总行数：自动换行行数 + 内容中的显式换行数
            let visual_len = prefix.width() + content.width();
            let wrap_lines = visual_len / term_width;
            let explicit_lines = content.matches('\n').count();
            let total_lines = wrap_lines + explicit_lines;
            for _ in 0..total_lines {
                let _ = write!(stdout, "\r\x1b[2K\x1b[A");
            }
            let _ = writeln!(stdout, "\r\x1b[2K{}{}", prefix, content);
            self.last_streamed_content.clear();
        } else {
            // 内容未变时跳过重复渲染，避免内容包含换行时产生多行残留
            if content == self.last_streamed_content {
                return;
            }
            self.last_streamed_content = content.to_string();
            // 将换行替换为可见表示，防止 \r 无法清除多行残留
            let display = format!("{} \x1b[5m▊\x1b[0m", content.replace('\n', "\\n"));
            let _ = write!(stdout, "\r\x1b[2K🤖 助手: {}", display);
            let _ = stdout.flush();
        }
    }
}

// ---------------------------------------------------------------------------
// 非交互模式：消息直接输出到 stdout
// ---------------------------------------------------------------------------

/// 非交互模式（`--message` 参数一次性执行）下的消息输出。
/// 消息直接写入 stdout，带有级别标签。
pub struct CliMessageOutput {
    verbose: bool,
}

impl CliMessageOutput {
    pub fn new(verbose: bool) -> Self {
        Self { verbose }
    }
}

impl MessageOutput for CliMessageOutput {
    fn emit(&mut self, level: MessageLevel, msg: &str) {
        // 非 verbose 模式下跳过 Debug 和 Info 级别消息（与 UIMessageOutput 保持一致）
        if !self.verbose && matches!(level, MessageLevel::Debug | MessageLevel::Info) {
            return;
        }

        let prefix = level.label();
        println!("{} {}", prefix, msg);
    }
}

// ---------------------------------------------------------------------------
// 静默模式（测试等）
// ---------------------------------------------------------------------------

/// 静默模式，不输出任何消息。用于测试场景。
#[allow(dead_code)]
pub struct SilentMessageOutput;

impl MessageOutput for SilentMessageOutput {
    fn emit(&mut self, _level: MessageLevel, _msg: &str) {}
}
