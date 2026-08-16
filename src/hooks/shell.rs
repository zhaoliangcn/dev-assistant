//! Shell hook 执行器：进程 spawn、超时控制、stdout 捕获。

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use tracing::{debug, warn};

use super::config::{HookConfig, HookEvent};
use super::error::HookError;

/// 最大输出字节数（默认 4096，由配置中的 max_output_bytes 覆盖）。
pub const DEFAULT_MAX_OUTPUT: usize = 4096;

/// 执行一个 shell hook 并捕获其 stdout（事件级上下文）。
///
/// - 使用 `command` 和 `args` 启动进程，不经过 shell 解析
/// - 向子进程注入 `DEV_ASSISTANT_EVENT` / `DEV_ASSISTANT_WORKDIR` / `DEV_ASSISTANT_HOOK_NAME` 环境变量
/// - 向子进程 stdin 写入 JSON payload：`{"event": "...", "cwd": "...", "name": "..."}` 后关闭
/// - 超时后将强制 kill 进程
/// - stderr 仅写入 tracing 日志，stdout 才是注入内容
/// - 输出超长时截断到 `max_output_bytes`
pub fn execute_shell_hook(
    config: &HookConfig,
    event: &HookEvent,
    workdir: &Path,
) -> Result<HookResult, HookError> {
    let payload = serde_json::json!({
        "event": event.as_str(),
        "cwd": workdir,
        "name": config.name,
    });
    run_hook_process(config, event, workdir, &payload, &[])
}

/// 执行一个工具级 hook 并捕获其 stdout（pre-tool / post-tool）。
///
/// 在 [`execute_shell_hook`] 基础上，额外：
/// - 注入 `DEV_ASSISTANT_TOOL_NAME` 环境变量
/// - stdin JSON payload 增加 `tool` 与 `arguments` 字段，hook 可按工具名/参数分支
pub fn execute_tool_hook(
    config: &HookConfig,
    event: &HookEvent,
    workdir: &Path,
    tool_name: &str,
    tool_args: &serde_json::Value,
) -> Result<HookResult, HookError> {
    let payload = serde_json::json!({
        "event": event.as_str(),
        "cwd": workdir,
        "name": config.name,
        "tool": tool_name,
        "arguments": tool_args,
    });
    let extra_env = [("DEV_ASSISTANT_TOOL_NAME", tool_name)];
    run_hook_process(config, event, workdir, &payload, &extra_env)
}

/// 核心执行：spawn 进程、注入环境变量、写 stdin payload、超时等待、截断输出。
fn run_hook_process(
    config: &HookConfig,
    event: &HookEvent,
    workdir: &Path,
    payload: &serde_json::Value,
    extra_env: &[(&str, &str)],
) -> Result<HookResult, HookError> {
    let timeout = config.timeout.unwrap_or(5);
    let max_output = config.max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT);

    debug!(name = %config.name, command = %config.command, timeout = %timeout, event = event.as_str(), "Executing shell hook");

    let mut cmd = Command::new(&config.command);
    if let Some(ref args) = config.args {
        cmd.args(args);
    }
    // 注入事件上下文，hook 可按事件分支、定位工作目录
    cmd.env("DEV_ASSISTANT_EVENT", event.as_str());
    cmd.env("DEV_ASSISTANT_WORKDIR", workdir);
    cmd.env("DEV_ASSISTANT_HOOK_NAME", &config.name);
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        HookError::Execution(format!("Failed to spawn '{}': {}", config.command, e))
    })?;

    // 写入 stdin JSON payload 后立即关闭，让 hook 进程读到 EOF。
    // payload 很小（远小于管道缓冲），不会与 stdout 读取互相死锁。
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = serde_json::to_writer(&mut stdin, payload) {
            warn!(name = %config.name, error = %e, "Failed to write hook stdin payload");
        }
        drop(stdin);
    }

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
        // 按 UTF-8 字符边界截断，避免切片落在多字节字符中间导致 panic
        let mut end = max_bytes.min(s.len());
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        let mut truncated = s[..end].to_string();
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
            event: HookEvent::SessionStart,
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
        let result = execute_shell_hook(&config, &HookEvent::SessionStart, std::path::Path::new(".")).unwrap();
        assert!(result.success);
        assert_eq!(result.name, "test");
        assert!(result.output.contains("hello"));
    }

    #[test]
    fn hook_timeout_produces_error() {
        let config = make_config("sleep", vec!["10"], 1);
        let result = execute_shell_hook(&config, &HookEvent::SessionStart, std::path::Path::new("."));
        assert!(result.is_err());
        match result {
            Err(HookError::Timeout(_, _)) => {} // expected
            _ => panic!("Expected Timeout error"),
        }
    }

    #[test]
    fn non_existent_command_errors() {
        let config = make_config("/nonexistent/command", vec![], 5);
        let result = execute_shell_hook(&config, &HookEvent::SessionStart, std::path::Path::new("."));
        assert!(result.is_err());
    }

    #[test]
    fn hook_receives_context_env_vars() {
        // 通过 sh 读取注入的环境变量，验证事件/工作目录/hook 名透传
        let config = HookConfig {
            name: "env-check".to_string(),
            event: HookEvent::SessionStart,
            type_: "shell".to_string(),
            command: "sh".to_string(),
            args: Some(vec![
                "-c".to_string(),
                "printf '%s|%s|%s' \"$DEV_ASSISTANT_EVENT\" \"$DEV_ASSISTANT_WORKDIR\" \"$DEV_ASSISTANT_HOOK_NAME\"".to_string(),
            ]),
            timeout: Some(5),
            priority: None,
            wrap_tag: None,
            max_output_bytes: None,
        };
        let workdir = std::path::Path::new("/tmp/hook-test");
        let result = execute_shell_hook(&config, &HookEvent::SessionStart, workdir).unwrap();
        assert_eq!(result.output, "session-start|/tmp/hook-test|env-check");
    }

    #[test]
    fn hook_receives_stdin_json_payload() {
        // cat 会把 stdin 原样输出，验证 JSON payload 已写入 stdin
        let config = HookConfig {
            name: "stdin-check".to_string(),
            event: HookEvent::SessionStart,
            type_: "shell".to_string(),
            command: "cat".to_string(),
            args: None,
            timeout: Some(5),
            priority: None,
            wrap_tag: None,
            max_output_bytes: None,
        };
        let workdir = std::path::Path::new("/tmp/stdin-test");
        let result = execute_shell_hook(&config, &HookEvent::SessionStart, workdir).unwrap();
        // payload 是 JSON：含 event / cwd / name 三个字段
        assert!(result.output.contains("\"event\":\"session-start\""));
        assert!(result.output.contains("\"cwd\":\"/tmp/stdin-test\""));
        assert!(result.output.contains("\"name\":\"stdin-check\""));
    }

    #[test]
    fn truncation_works() {
        let config = HookConfig {
            name: "truncate".to_string(),
            event: HookEvent::SessionStart,
            type_: "shell".to_string(),
            command: "echo".to_string(),
            args: Some(vec!["hello world".to_string()]),
            timeout: Some(5),
            priority: None,
            wrap_tag: None,
            max_output_bytes: Some(6),
        };
        let result = execute_shell_hook(&config, &HookEvent::SessionStart, std::path::Path::new(".")).unwrap();
        assert!(result.output.contains("... (truncated)"));
        assert!(result.output.len() <= 30);
    }

    #[test]
    fn truncation_respects_utf8_boundary() {
        // "你好世界" 为 12 字节，max_output_bytes=7 落在第 2 个字符中间
        let config = HookConfig {
            name: "truncate-utf8".to_string(),
            event: HookEvent::SessionStart,
            type_: "shell".to_string(),
            command: "echo".to_string(),
            args: Some(vec!["你好世界".to_string()]),
            timeout: Some(5),
            priority: None,
            wrap_tag: None,
            max_output_bytes: Some(7),
        };
        let result = execute_shell_hook(&config, &HookEvent::SessionStart, std::path::Path::new(".")).unwrap();
        assert!(result.output.contains("... (truncated)"));
        // 截断后的内容必须是合法 UTF-8，且以完整字符开头
        assert!(result.output.starts_with("你"));
        assert!(result.output.is_char_boundary(3));
    }
}