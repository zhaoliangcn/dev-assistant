use std::io;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Glob error: {0}")]
    Glob(#[from] globset::Error),
    #[error("Walkdir error: {0}")]
    Walkdir(#[from] walkdir::Error),
    #[error("Environment variable not found: {0}")]
    Env(String),
    #[error("LLM error: {0}")]
    Llm(String),
    #[error("LLM rate limited: {0}")]
    RateLimited(String),
    #[error("Tool not found: {0}")]
    ToolNotFound(String),
    #[error("Security error: {0}")]
    Security(String),
    #[error("Invalid config: {0}")]
    Config(String),
    #[error("Subagent depth limit exceeded (max depth: {0})")]
    SubagentDepthLimit(usize),
}

impl AppError {
    /// 如果错误是 rate limit（429 / Too Many Requests）返回 true。
    ///
    /// 结构化判断：provider 层返回 [`AppError::RateLimited`] 时直接命中；
    /// [`AppError::Llm`] 中残留的 "status 429" / "Too Many Requests" 字符串
    /// 作为兼容旧 provider 实现的兜底。
    pub fn is_rate_limited(&self) -> bool {
        match self {
            AppError::RateLimited(_) => true,
            AppError::Llm(msg) => {
                msg.contains("status 429") || msg.contains("Too Many Requests")
            }
            _ => false,
        }
    }
    
    /// 判断错误是否可重试
    /// 
    /// 只有瞬时错误才应该重试：
    /// - RateLimited: 需要等待后重试
    /// - Io(Interrupted): 被中断，可以重试
    /// - Http(timeout/connect): 网络超时/连接失败
    /// - Llm: LLM 服务暂时不可用
    /// 
    /// 不可重试的错误：
    /// - NotFound: 文件不存在
    /// - PermissionDenied: 权限拒绝
    /// - ToolNotFound: 工具不存在
    /// - Security: 安全策略阻止
    /// - Config: 配置错误
    pub fn is_retryable(&self) -> bool {
        match self {
            AppError::RateLimited(_) => true,
            AppError::Io(e) => matches!(e.kind(), std::io::ErrorKind::Interrupted),
            AppError::Http(e) => e.is_timeout() || e.is_connect(),
            AppError::Llm(_) => true,
            _ => false,
        }
    }
}
