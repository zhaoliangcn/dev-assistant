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
    /// 服务端错误（5xx），通常是瞬时的，适合自动重试。
    #[error("LLM server error ({0}): {1}")]
    ServerError(u16, String),
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
            AppError::ServerError(code, _) => *code >= 500,
            // 兼容旧 provider 直接返回 Llm 的兜底，保留但不依赖
            AppError::Llm(msg) => msg.contains("status 5"),
            AppError::Http(e) => e.status().is_some_and(|s| s.as_u16() >= 500),
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

    /// 连接阶段失败（DNS 解析失败、连接被拒等），由 `.send()` 抛出，无 HTTP status。
    ///
    /// 此类错误通常意味着目标服务未运行/不可达：重试几次可捕捉瞬时抖动或本地
    /// 模型重启，持续失败则应快速故障转移。注意区分请求超时——超时单次已耗时
    /// 整个 HTTP 超时窗口，重试通常无益，故不归入此类、直接故障转移。
    pub fn is_connect_error(&self) -> bool {
        matches!(self, AppError::Http(e) if e.is_connect())
    }

    /// 判断错误是否可重试
    ///
    /// 只有瞬时错误才应该重试：
    /// - RateLimited: 需要等待后重试
    /// - ServerError: 服务端瞬时故障
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
            AppError::ServerError { .. } => true,
            AppError::Io(e) => matches!(e.kind(), std::io::ErrorKind::Interrupted),
            AppError::Http(e) => e.is_timeout() || e.is_connect(),
            AppError::Llm(_) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn classifies_rate_limited() {
        let e = AppError::RateLimited {
            message: "rl".into(),
            retry_after: Some(Duration::from_secs(5)),
        };
        assert!(e.is_rate_limited());
        assert!(!e.is_server_error());
        assert!(!e.is_connect_error());
        assert_eq!(e.retry_after(), Some(Duration::from_secs(5)));
    }

    #[test]
    fn classifies_server_error_5xx() {
        let e = AppError::ServerError(503, "boom".into());
        assert!(e.is_server_error());
        assert!(!e.is_rate_limited());
        assert!(!e.is_connect_error());
    }

    #[test]
    fn classifies_4xx_llm_as_non_transient() {
        // 4xx（鉴权/参数错误）不应被识别为 429/5xx，重试无益
        let e = AppError::Llm("LLM API returned error (status 400): bad request".into());
        assert!(!e.is_rate_limited());
        assert!(!e.is_server_error());
        assert!(!e.is_connect_error());
    }

    #[test]
    fn classifies_rate_limited_via_llm_substring_fallback() {
        // 旧 provider 残留 "status 429" 字符串的兜底识别
        let e = AppError::Llm("... status 429 Too Many Requests ...".into());
        assert!(e.is_rate_limited());
    }

    #[test]
    fn config_error_is_not_transient_nor_connect() {
        let e = AppError::Config("bad".into());
        assert!(!e.is_rate_limited());
        assert!(!e.is_server_error());
        assert!(!e.is_connect_error());
    }
}
