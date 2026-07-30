use std::io;
use std::time::Duration;

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
    /// LLM rate limited。`retry_after` 是服务端建议的等待时间（若有）。
    #[error("LLM rate limited: {message}")]
    RateLimited {
        message: String,
        retry_after: Option<Duration>,
    },
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
            AppError::RateLimited { .. } => true,
            AppError::Llm(msg) => {
                msg.contains("status 429") || msg.contains("Too Many Requests")
            }
            _ => false,
        }
    }

    /// 如果错误是服务端错误（5xx / server error）返回 true。
    ///
    /// 此类错误通常是瞬时的，适合自动重试。
    pub fn is_server_error(&self) -> bool {
        match self {
            AppError::Llm(msg) => {
                msg.contains("status 502") || msg.contains("Bad Gateway")
                    || msg.contains("status 503") || msg.contains("Service Unavailable")
                    || msg.contains("status 504") || msg.contains("Gateway Timeout")
                    || msg.contains("status 5")
            }
            AppError::Http(e) => {
                e.status().is_some_and(|s| s.as_u16() >= 500)
            }
            _ => false,
        }
    }

    /// 如果错误是 rate limit，返回服务端建议的等待时间（Retry-After 头）。
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            AppError::RateLimited { retry_after, .. } => *retry_after,
            _ => None,
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
            AppError::RateLimited { .. } => true,
            AppError::Io(e) => matches!(e.kind(), std::io::ErrorKind::Interrupted),
            AppError::Http(e) => e.is_timeout() || e.is_connect(),
            AppError::Llm(_) => true,
            _ => false,
        }
    }
}
