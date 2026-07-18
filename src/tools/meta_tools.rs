use super::{ToolArgs, ToolContext, ToolDefinition, ToolResult};
use crate::utils::error::AppError;

/// The project root path, embedded at compile time.
/// restart tool is only allowed when the working directory matches this project.
const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

pub fn finish_tool() -> ToolDefinition {
    ToolDefinition {
        name: "finish".to_string(),
        description: "Finish the task and provide a summary".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Task completion summary"
                }
            },
            "required": ["summary"]
        }),
        handler: Box::new(finish_handler),
    }
}

fn finish_handler(args: &ToolArgs, _context: &ToolContext) -> Result<ToolResult, AppError> {
    let summary = args.arguments["summary"]
        .as_str()
        .unwrap_or("Task completed");

    Ok(ToolResult {
        success: true,
        security_evaluation: None,
        restart_requested: false,
        content: format!("[finish] {}", summary),
    })
}

pub fn restart_tool() -> ToolDefinition {
    ToolDefinition {
        name: "restart".to_string(),
        description: "Save conversation state, run cargo build, and restart the dev-assistant process with the updated binary. Only available when working on the dev-assistant-rs project itself. Use this after modifying the project's Rust source code to verify changes compile.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "state_file": {
                    "type": "string",
                    "description": "Path to save conversation state (default: .dev-assistant-state.json in working directory)"
                }
            }
        }),
        handler: Box::new(restart_handler),
    }
}

fn restart_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    // Verify that the working directory is the dev-assistant-rs project itself.
    // This tool is only for self-modification, not for arbitrary Rust projects.
    // Canonicalize both paths to handle relative vs absolute path comparisons.
    let project_root = std::path::Path::new(PROJECT_ROOT);
    let cwd = if context.working_dir.is_absolute() {
        context.working_dir.clone()
    } else {
        std::env::current_dir().map(|p| p.join(&context.working_dir)).unwrap_or_else(|_| context.working_dir.clone())
    };
    let cwd_canonical = cwd.canonicalize().unwrap_or(cwd);
    let root_canonical = project_root.canonicalize().unwrap_or_else(|_| project_root.to_path_buf());

    if cwd_canonical != root_canonical {
        return Ok(ToolResult {
            success: false,
            security_evaluation: None,
            restart_requested: false,
            content: format!(
                "[restart] ❌ Restart is only available when working on the dev-assistant-rs project itself.\n\
                 Current project: {}\n\
                 This tool is for self-modification of the assistant, not for general Rust projects.",
                context.working_dir.display()
            ),
        });
    }

    let state_file = args.arguments["state_file"]
        .as_str()
        .map(|s| context.working_dir.join(s))
        .unwrap_or_else(|| context.working_dir.join(".dev-assistant-state.json"));

    // Signal that a restart is requested; the Agent::run method will detect
    // restart_requested = true and handle the actual restart logic.
    Ok(ToolResult {
        success: true,
        security_evaluation: None,
        restart_requested: true,
        content: format!(
            "[restart] Restart requested. State will be saved to: {}\n\
             The process will restart after cargo build completes.",
            state_file.display()
        ),
    })
}
