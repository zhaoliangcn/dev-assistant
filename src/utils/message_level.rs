// ---------------------------------------------------------------------------
// 消息级别
// ---------------------------------------------------------------------------

/// 消息的严重级别，用于决定在 UI 中的样式和过滤逻辑。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageLevel {
    /// 一般信息
    Info,
    /// 操作成功
    Success,
    /// 错误信息
    Error,
    /// 警告信息
    Warning,
    /// 调试信息（仅在 verbose 模式下显示）
    Debug,
}

impl MessageLevel {
    pub fn label(self) -> &'static str {
        match self {
            MessageLevel::Info => "信息",
            MessageLevel::Success => "成功",
            MessageLevel::Error => "错误",
            MessageLevel::Warning => "警告",
            MessageLevel::Debug => "调试",
        }
    }
}
