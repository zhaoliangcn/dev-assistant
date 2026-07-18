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
}

impl AppError {
    /// 如果错误是 429 / Too Many Requests 返回 true。
    pub fn is_rate_limited(&self) -> bool {
        match self {
            AppError::RateLimited(msg) => {
                msg.contains("status 429") || msg.contains("Too Many Requests")
            }
            AppError::Llm(msg) => msg.contains("status 429") || msg.contains("Too Many Requests"),
            _ => false,
        }
    }
}
