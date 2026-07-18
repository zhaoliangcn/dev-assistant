use crate::utils::message_level::MessageLevel;

/// 所有面向用户的消息都应通过此接口输出，而不是直接使用 `tracing` 或
/// `println!`。`tracing` 保留给内部调试日志使用。
///
/// 实现者根据运行模式决定消息的落点：
/// - 交互模式 → 写入 `ContextManager`，由 split-pane UI 渲染
/// - 非交互模式（`--message`）→ 直接打印到 stdout
/// - 测试模式 → 静默丢弃
#[allow(dead_code)]
pub trait MessageOutput: Send + Sync {
    fn emit(&mut self, level: MessageLevel, msg: &str);

    fn info(&mut self, msg: &str) {
        self.emit(MessageLevel::Info, msg);
    }
    fn success(&mut self, msg: &str) {
        self.emit(MessageLevel::Success, msg);
    }
    fn error(&mut self, msg: &str) {
        self.emit(MessageLevel::Error, msg);
    }
    fn warning(&mut self, msg: &str) {
        self.emit(MessageLevel::Warning, msg);
    }
    fn debug(&mut self, msg: &str) {
        self.emit(MessageLevel::Debug, msg);
    }
}
