use std::io;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum HookError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Hook timed out after {0}s: {1}")]
    Timeout(u64, String),
    #[error("Hook execution failed: {0}")]
    Execution(String),
    #[error("YAML parse error: {0}")]
    Yaml(String),
}