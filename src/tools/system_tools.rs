use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use super::{ToolArgs, ToolContext, ToolDefinition, ToolResult};
use crate::utils::error::AppError;
use tracing::debug;

pub fn exec_command_tool() -> ToolDefinition {
    ToolDefinition {
        name: "exec_command".to_string(),
        description: "执行命令，支持管道/重定向/&& 等 shell 语法（自动通过 shell 执行）。可直接传完整命令字符串，如 \"ls -la | head\"；或用 args 传参数列表执行非 shell 命令。".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "要执行的命令。未提供 args 时作为完整 shell 命令执行（支持管道/重定向，如 \"ls -la | grep foo\"、\"cargo build 2>&1\"）。"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "可选参数列表。提供 args 时按非 shell 模式直接执行 command + args（如 [\"build\", \"--release\"]）。"
                }
            },
            "required": ["command"]
        }),
        skip_security: false,
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
    debug!(command = %command, args = ?extra_args, cwd = %working_dir.display(), timeout_secs = %timeout, "exec_command");

    // Build display string for the result
    let args_for_display = if extra_args.is_empty() {
        command.to_string()
    } else {
        format!("{} {}", command, extra_args.join(" "))
    };

    // Max output byte limit: 10MB total (stdout + stderr combined) to prevent OOM.
    const MAX_TOTAL_OUTPUT_BYTES: usize = 10 * 1024 * 1024;
    let remaining = Arc::new(AtomicUsize::new(MAX_TOTAL_OUTPUT_BYTES));

    // Spawn with piped output.
    // - 提供 args 时：非 shell 模式，直接执行 command + args
    // - 未提供 args 时：shell 模式，整个 command 作为 shell 命令执行（支持管道/重定向/&& 等），
    //   避免 LLM 因 sh -c 包装多花一轮工具调用。
    let mut cmd = if extra_args.is_empty() {
        #[cfg(unix)]
        {
            let mut c = Command::new("sh");
            c.arg("-c").arg(command);
            c
        }
        #[cfg(not(unix))]
        {
            // 默认 PowerShell（LLM 生成的管道/&&等现代命令兼容性更好）；
            // 设 DA_SHELL=cmd 可回退旧行为。
            if std::env::var("DA_SHELL").as_deref() == Ok("cmd") {
                let mut c = Command::new("cmd");
                c.arg("/C").arg(command);
                c
            } else {
                let mut c = Command::new("powershell");
                c.args(["-NoProfile", "-NonInteractive", "-Command", command]);
                c
            }
        }
    } else {
        let mut c = Command::new(command);
        c.args(&extra_args);
        c
    };
    cmd.current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // SECURITY: On Unix, create a new process group for the child so that
    // we can kill the entire group (including grandchildren) on timeout.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                // Create a new process group. The PGID equals the child PID.
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let mut child = cmd.spawn().map_err(|e| {
        AppError::Llm(format!("Command execution failed for '{}': {}", command, e))
    })?;

    let pid = child.id();

    // SECURITY: Also set the process group from the parent side, as a fallback
    // in case the pre_exec setpgid fails. This is a common pattern to avoid
    // race conditions between fork and exec. We track whether the PGID was
    // successfully created so we know whether killpg() is safe to use.
    #[cfg(unix)]
    let pgid_created = unsafe { libc::setpgid(pid as i32, pid as i32) == 0 };

    // Windows 无进程组概念；kill_process_tree 走 taskkill /T 杀进程树。
    #[cfg(not(unix))]
    let pgid_created = false;

    // Read stdout/stderr with a byte limit to prevent OOM from large output.
    // Uses separate threads to read pipes concurrently, avoiding deadlock.
    use std::io::Read;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let (tx, rx) = mpsc::channel();

    let remaining_clone = Arc::clone(&remaining);
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::with_capacity(4096);
        if let Some(mut reader) = stdout {
            loop {
                let current = remaining_clone.load(Ordering::Acquire);
                if current == 0 {
                    break;
                }
                let take = current.min(64 * 1024); // 每次最多读 64KB
                let mut chunk_buf = vec![0u8; take];
                match std::io::Read::read(&mut reader, &mut chunk_buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        buf.extend_from_slice(&chunk_buf[..n]);
                        remaining_clone.fetch_sub(n, Ordering::Release);
                    }
                    Err(_) => break,
                }
            }
        }
        buf
    });

    let remaining_clone = Arc::clone(&remaining);
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::with_capacity(4096);
        if let Some(mut reader) = stderr {
            loop {
                let current = remaining_clone.load(Ordering::Acquire);
                if current == 0 {
                    break;
                }
                let take = current.min(64 * 1024);
                let mut chunk_buf = vec![0u8; take];
                match std::io::Read::read(&mut reader, &mut chunk_buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk_buf[..n]);
                        remaining_clone.fetch_sub(n, Ordering::Release);
                    }
                    Err(_) => break,
                }
            }
        }
        buf
    });

    // Wait for the process to finish (with timeout via detached thread).
    let _waiter = std::thread::spawn(move || {
        let status = child.wait();
        tx.send(status).ok();
    });

    match rx.recv_timeout(Duration::from_secs(timeout)) {
        Ok(Ok(status)) => {
            // Process completed within timeout — collect output from reader threads
            let stdout_bytes = stdout_reader.join().unwrap_or_default();
            let stderr_bytes = stderr_reader.join().unwrap_or_default();
            let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
            let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
            let stdout_truncated = stdout_bytes.len() >= MAX_TOTAL_OUTPUT_BYTES;
            let stderr_truncated = stderr_bytes.len() >= MAX_TOTAL_OUTPUT_BYTES;

            let mut content = format!(
                "[exec_command] {} (exit code: {})",
                args_for_display,
                status.code().unwrap_or(0)
            );

            if !stdout.is_empty() {
                content.push_str("\n\n--- stdout ---\n");
                content.push_str(&stdout);
                if stdout_truncated {
                    content.push_str("\n... (stdout truncated, exceeded 10MB limit)");
                }
            }
            if !stderr.is_empty() {
                content.push_str("\n\n--- stderr ---\n");
                content.push_str(&stderr);
                if stderr_truncated {
                    content.push_str("\n... (stderr truncated, exceeded 10MB limit)");
                }
            }

            // Also cap the final content string to 50KB for LLM context
            const MAX_CONTENT_LEN: usize = 50000;
            if content.len() > MAX_CONTENT_LEN {
                let truncated = content.len() - MAX_CONTENT_LEN;
                content.truncate(MAX_CONTENT_LEN);
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
                error_category: None,
                    content,
                })
            } else {
                Ok(ToolResult {
                    success: false,
                    security_evaluation: None,
                    restart_requested: false,
                error_category: None,
                    content,
                })
            }
        }
        Ok(Err(e)) => Err(AppError::Llm(format!("Command wait failed: {}", e))),
        Err(_) => {
            // Timeout — kill the entire process group (Unix) or process tree (Windows)
            // to prevent resource leaks from grandchildren.
            debug!(command = %command, pid = %pid, timeout_secs = %timeout, "Command timed out, killing process group");
            let _killed = kill_process_tree(pid, pgid_created);

            Ok(ToolResult {
                success: false,
                security_evaluation: None,
                restart_requested: false,
                error_category: None,
                content: format!(
                    "[exec_command] ❌ Timed out after {} seconds: {}",
                    timeout, args_for_display
                ),
            })
        }
    }
}

/// Attempt to kill an entire process group (Unix) or process tree (Windows).
/// Returns true if the kill signal was sent successfully.
///
/// # Safety
///
/// 调用 `killpg` 前先使用 `kill(pid, 0)` 检查进程是否仍然存活，
/// 防止在子进程已自然退出、PID 被内核回收后误杀不相关的进程。
///
/// `pgid_created` 表示进程组是否已成功创建（Unix 下），
/// 如果为 false 则回退到 kill 单个进程而非进程组。
#[cfg(unix)]
fn kill_process_tree(pid: u32, pgid_created: bool) -> bool {
    // First check if the process is still alive (kill with signal 0).
    // This prevents killing a process whose PID has been reused after
    // the child exited naturally between the timeout detection and the kill call.
    if unsafe { libc::kill(pid as i32, 0) } != 0 {
        // Process no longer exists (or we don't have permission, which is fine
        // because it means the process isn't ours to kill).
        return false;
    }
    if pgid_created {
        // Negative PID = process group, which kills the process and all its children.
        unsafe { libc::killpg(pid as i32, libc::SIGKILL) == 0 }
    } else {
        // Fallback: kill just the process itself (not the group).
        // This is safe when the process group couldn't be created.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) == 0 }
    }
}

#[cfg(not(unix))]
fn kill_process_tree(pid: u32, _pgid_created: bool) -> bool {
    std::process::Command::new("taskkill")
        .args(&["/F", "/T", "/PID", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}