use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::Arc;
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

const COMMAND_TIMEOUT_SECS: u64 = 30;

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
    debug!(command = %command, args = ?extra_args, cwd = %working_dir.display(), "exec_command");

    // SECURITY: Execute the command directly without shell interpretation.
    // This prevents shell injection attacks (e.g., command="ls"; args=["; rm -rf /"]).
    // If shell features (pipes, redirects) are needed, the LLM should explicitly
    // invoke a shell: command="sh", args=["-c", "ls | grep foo"]
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

    // Use Arc<Mutex<Option<Child>>> to share the child between the waiter thread
    // and the main thread (for timeout killing)
    let child_arc = Arc::new(std::sync::Mutex::new(Some(child)));
    let child_arc_for_timeout = child_arc.clone();

    // Use mpsc channel to get the exit status with timeout
    let (tx, rx) = mpsc::channel();

    let _waiter = std::thread::spawn(move || {
        let mut child_opt = child_arc.lock().unwrap();
        if let Some(mut child) = child_opt.take() {
            let status = child.wait();
            tx.send(status).ok();
        }
    });

    // Build display string for the result
    let args_for_display = if extra_args.is_empty() {
        command.to_string()
    } else {
        format!("{} {}", command, extra_args.join(" "))
    };

    match rx.recv_timeout(Duration::from_secs(COMMAND_TIMEOUT_SECS)) {
        Ok(Ok(status)) => {
            // Process completed within timeout — collect output
            let mut child_opt = child_arc_for_timeout.lock().unwrap();
            let (stdout, stderr) = if let Some(mut child) = child_opt.take() {
                let stdout = child.stdout.take()
                    .map(|mut s| {
                        let mut buf = String::new();
                        use std::io::Read;
                        let _ = s.read_to_string(&mut buf);
                        buf
                    })
                    .unwrap_or_default();
                let stderr = child.stderr.take()
                    .map(|mut s| {
                        let mut buf = String::new();
                        use std::io::Read;
                        let _ = s.read_to_string(&mut buf);
                        buf
                    })
                    .unwrap_or_default();
                (stdout, stderr)
            } else {
                (String::new(), String::new())
            };

            let mut content = format!("[exec_command] {} (exit code: {})",
                args_for_display,
                status.code().unwrap_or(0));

            if !stdout.is_empty() {
                content.push_str("\n\n--- stdout ---\n");
                content.push_str(&stdout);
            }
            if !stderr.is_empty() {
                content.push_str("\n\n--- stderr ---\n");
                content.push_str(&stderr);
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
            // Timeout — kill the child process to prevent resource leak
            let mut child_opt = child_arc_for_timeout.lock().unwrap();
            if let Some(mut child) = child_opt.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            Ok(ToolResult {
                success: false,
                security_evaluation: None,
                    restart_requested: false,
                content: format!(
                    "[exec_command] ❌ Timed out after {} seconds: {}",
                    COMMAND_TIMEOUT_SECS,
                    args_for_display
                ),
            })
        }
    }
}
