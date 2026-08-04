//! Shell hook 执行器：进程 spawn、超时控制、stdout 捕获。

use std::process::{Command, Output, Stdio};
use std::time::Duration;

use tracing::{debug, warn};

use super::error::HookError;
use super::config::HookConfig;

/// 最大输出字节数（默认 4096，由配置中的 max_output_bytes 覆盖）。
const DEFAULT_MAX_OUTPUT: usize = 4096;

/// 执行一个 shell hook 并捕获其 stdout。
///
/// - 使用 `command` 和 `args` 启动进程，不经过 shell 解析
/// - 超时后将强制 kill 进程
/// - stderr 仅写入 tracing 日志，stdout 才是注入内容
/// - 输出超长时截断到 `max_output_bytes`
pub fn execute_shell_hook(config: &HookConfig) -> Result<HookResult, HookError> {
    let timeout = config.timeout.unwrap_or(5);
    let max_output = config.max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT);

    debug!(name = %config.name, command = %config.command, timeout = %timeout, "Executing shell hook");

    let mut cmd = Command::new(&config.command);
    if let Some(ref args) = config.args {
        cmd.args(args);
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        HookError::Execution(format!("Failed to spawn '{}': {}", config.command, e))
    })?;

    // 超时等待
    let start = std::time::Instant::now();
    let output: Output = loop {
        if start.elapsed() >= Duration::from_secs(timeout) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(HookError::Timeout(timeout, config.name.clone()));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                // 进程已退出，收集输出
                let stdout = child.stdout.take()
                    .map(|mut s| {
                        let mut buf = Vec::new();
                        std::io::Read::read_to_end(&mut s, &mut buf).ok();
                        buf
                    })
                    .unwrap_or_default();
                let stderr = child.stderr.take()
                    .map(|mut s| {
                        let mut buf = Vec::new();
                        std::io::Read::read_to_end(&mut s, &mut buf).ok();
                        buf
                    })
                    .unwrap_or_default();

                let output = Output {
                    status,
                    stdout,
                    stderr,
                };
                break output;
            }
            Ok(None) => {
                // 仍在运行，短暂休眠后重试
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HookError::Execution(format!(
                    "Failed to wait for '{}': {}",
                    config.command, e
                )));
            }
        }
    };

    // 记录 stderr（仅日志）
    let stderr_str = String::from_utf8_lossy(&output.stderr);
    if !stderr_str.trim().is_empty() {
        warn!(name = %config.name, stderr = %stderr_str.trim(), "Hook stderr");
    }

    // 检查退出码
    if !output.status.success() {
        let msg = String::from_utf8_lossy(&output.stdout).to_string();
        return Err(HookError::Execution(format!(
            "Hook '{}' exited with code {}: {}",
            config.name,
            output.status.code().unwrap_or(-1),
            truncate_output(&msg, max_output)
        )));
    }

    let stdout_str = String::from_utf8_lossy(&output.stdout).to_string();

    debug!(name = %config.name, bytes = stdout_str.len(), "Shell hook completed");

    Ok(HookResult {
        name: config.name.clone(),
        output: truncate_output(&stdout_str, max_output),
        success: true,
    })
}

/// Hook 执行结果。
#[derive(Debug, Clone)]
pub struct HookResult {
    pub name: String,
    pub output: String,
    #[allow(dead_code)]
    pub success: bool,
}

fn truncate_output(s: &str, max_bytes: usize) -> String {
    if s.len() > max_bytes {
        let mut truncated = s[..max_bytes].to_string();
        truncated.push_str("\n... (truncated)");
        truncated
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(command: &str, args: Vec<&str>, timeout: u64) -> HookConfig {
        HookConfig {
            name: "test".to_string(),
            event: "session-start".to_string(),
            type_: "shell".to_string(),
            command: command.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            timeout: Some(timeout),
            priority: None,
            wrap_tag: None,
            max_output_bytes: None,
        }
    }

    #[test]
    fn echo_hook_succeeds() {
        let config = make_config("echo", vec!["hello"], 5);
        let result = execute_shell_hook(&config).unwrap();
        assert!(result.success);
        assert_eq!(result.name, "test");
        assert!(result.output.contains("hello"));
    }

    #[test]
    fn hook_timeout_produces_error() {
        let config = make_config("sleep", vec!["10"], 1);
        let result = execute_shell_hook(&config);
        assert!(result.is_err());
        match result {
            Err(HookError::Timeout(_, _)) => {} // expected
            _ => panic!("Expected Timeout error"),
        }
    }

    #[test]
    fn non_existent_command_errors() {
        let config = make_config("/nonexistent/command", vec![], 5);
        let result = execute_shell_hook(&config);
        assert!(result.is_err());
    }

    #[test]
    fn truncation_works() {
        let config = HookConfig {
            name: "truncate".to_string(),
            event: "session-start".to_string(),
            type_: "shell".to_string(),
            command: "echo".to_string(),
            args: Some(vec!["hello world".to_string()]),
            timeout: Some(5),
            priority: None,
            wrap_tag: None,
            max_output_bytes: Some(6),
        };
        let result = execute_shell_hook(&config).unwrap();
        assert!(result.output.contains("... (truncated)"));
        assert!(result.output.len() <= 30);
    }
}