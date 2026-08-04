use std::io;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum HookError {
    /// 配置错误（预留，供后续配置校验使用）。
    #[allow(dead_code)]
    #[error("Hook config error: {0}")]
    Config(String),
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Hook timed out after {0}s: {1}")]
    Timeout(u64, String),
    #[error("Hook execution failed: {0}")]
    Execution(String),
    #[error("YAML parse error: {0}")]
    Yaml(String),
}