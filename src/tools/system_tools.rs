use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use super::{ToolArgs, ToolContext, ToolDefinition, ToolResult};
use crate::utils::error::AppError;
use tracing::debug;

pub fn exec_command_tool() -> ToolDefinition {
    ToolDefinition {
        name: "exec_command".to_string(),
        description: "Execute a command directly in the project directory. For security reasons, this does NOT invoke a shell, so shell syntax like pipes (|), redirects (>, >>), and logical operators (&&, ||) are NOT supported. If you need shell features, use command=\"sh\" with args=[\"-c\", \"your_pipeline_here\"] instead.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The executable to run (e.g., \"ls\", \"cargo\", \"git\"). Do NOT include shell syntax here."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional arguments passed to the command (e.g., [\"build\", \"--release\"]). For shell pipelines, use command=\"sh\" with args=[\"-c\", \"cmd1 | cmd2\"]."
                }
            },
            "required": ["command"]
        }),
        handler: Box::new(exec_command_handler),
    }
}

/// 默认命令超时时间（秒），可通过环境变量 `EXEC_COMMAND_TIMEOUT` 覆盖。
fn command_timeout_secs() -> u64 {
    std::env::var("EXEC_COMMAND_TIMEOUT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300)
}

fn exec_command_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    let command = args.arguments["command"]
        .as_str()
        .ok_or_else(|| AppError::Llm("command is required".to_string()))?;

    let extra_args: Vec<String> = args.arguments["args"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let working_dir = context.working_dir.clone();
    let timeout = command_timeout_secs();
    debug!(command = %command, args = ?extra_args, cwd = %working_dir.display(), timeout = %timeout, "exec_command");

    // Build display string for the result
    let args_for_display = if extra_args.is_empty() {
        command.to_string()
    } else {
        format!("{} {}", command, extra_args.join(" "))
    };

    // Spawn with piped output
    let mut cmd = Command::new(command);
    cmd.args(&extra_args)
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = cmd.spawn().map_err(|e| {
        AppError::Llm(format!(
            "Command execution failed for '{}': {}. \
             Note: exec_command no longer supports shell syntax (pipes, redirects, etc.). \
             Use command=\"sh\" with args=[\"-c\", \"your_command_here\"] if shell features are required.",
            command, e
        ))
    })?;

    let pid = child.id();

    // Use wait_with_output() in a separate thread for timeout support.
    // wait_with_output() internally reads stdout/stderr pipes to completion,
    // avoiding the deadlock that would occur if we only waited on the child.
    let (tx, rx) = mpsc::channel();

    let _waiter = std::thread::spawn(move || {
        let output = child.wait_with_output();
        tx.send(output).ok();
    });

    match rx.recv_timeout(Duration::from_secs(timeout)) {
        Ok(Ok(output)) => {
            // Process completed within timeout — we have all output
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let status = output.status;

            let mut content = format!(
                "[exec_command] {} (exit code: {})",
                args_for_display,
                status.code().unwrap_or(0)
            );

            if !stdout.is_empty() {
                content.push_str("\n\n--- stdout ---\n");
                content.push_str(&stdout);
            }
            if !stderr.is_empty() {
                content.push_str("\n\n--- stderr ---\n");
                content.push_str(&stderr);
            }

            // If stdout was large, add a summary
            const MAX_OUTPUT_LEN: usize = 50000;
            if content.len() > MAX_OUTPUT_LEN {
                let truncated = content.len() - MAX_OUTPUT_LEN;
                content.truncate(MAX_OUTPUT_LEN);
                content.push_str(&format!(
                    "\n\n... (output truncated, {} more bytes)",
                    truncated
                ));
            }

            if status.success() {
                Ok(ToolResult {
                    success: true,
                    security_evaluation: None,
                    restart_requested: false,
                    content,
                })
            } else {
                Ok(ToolResult {
                    success: false,
                    security_evaluation: None,
                    restart_requested: false,
                    content,
                })
            }
        }
        Ok(Err(e)) => Err(AppError::Llm(format!("Command wait failed: {}", e))),
        Err(_) => {
            // Timeout — kill the child process by PID to prevent resource leak.
            // We use PID-based kill because the Child object is consumed by
            // wait_with_output() in the waiter thread.
            debug!(command = %command, pid = %pid, timeout_secs = %timeout, "Command timed out, killing process");
            let _killed = kill_process(pid);

            Ok(ToolResult {
                success: false,
                security_evaluation: None,
                restart_requested: false,
                content: format!(
                    "[exec_command] ❌ Timed out after {} seconds: {}",
                    timeout, args_for_display
                ),
            })
        }
    }
}

/// Attempt to kill a process by PID. Returns true if the kill signal was sent.
#[cfg(unix)]
fn kill_process(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, libc::SIGKILL) == 0 }
}

#[cfg(not(unix))]
fn kill_process(pid: u32) -> bool {
    std::process::Command::new("taskkill")
        .args(&["/F", "/PID", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}