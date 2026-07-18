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
}

impl UIMessageOutput {
    pub fn new(verbose: bool) -> Self {
        Self { verbose, buffer: Vec::new() }
    }

    /// 取出所有暂存的消息并清空缓冲区。
    pub fn drain(&mut self) -> Vec<(MessageLevel, String)> {
        std::mem::take(&mut self.buffer)
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
