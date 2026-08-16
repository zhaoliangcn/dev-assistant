use super::{ToolArgs, ToolContext, ToolDefinition, ToolResult};
use crate::hooks::config::HookEvent;
use crate::utils::error::AppError;

/// The project root path, embedded at compile time.
/// restart tool is only allowed when the working directory matches this project.
const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

pub fn run_hook_tool() -> ToolDefinition {
    ToolDefinition {
        name: "run_hook".to_string(),
        description: "Execute configured hooks (from .dev-assistant/hooks.yaml) on demand and return their output. Use to re-run lifecycle scripts (git status, lint, tests, project context) at any point during the session. Optionally filter by event or hook name.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "event": {
                    "type": "string",
                    "description": "Hook event to execute (default: session-start)",
                    "enum": ["session-start", "session-end", "pre-tool", "post-tool", "user-input"]
                },
                "name": {
                    "type": "string",
                    "description": "Only execute the hook with this name (default: all hooks for the event)"
                }
            }
        }),
        skip_security: true,
        handler: Box::new(run_hook_handler),
    }
}

fn run_hook_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    let event_str = args.arguments["event"].as_str().unwrap_or("session-start");
    let event = match HookEvent::parse(event_str) {
        Some(e) => e,
        None => {
            return Ok(ToolResult {
                success: false,
                security_evaluation: None,
                restart_requested: false,
                error_category: None,
                content: format!("[run_hook] ❌ Unknown hook event: '{}'. Valid: session-start, session-end, pre-tool, post-tool, user-input", event_str),
            });
        }
    };
    let name_filter = args.arguments["name"].as_str();

    // 优先复用注入的共享 HookManager（由 App 组装，尊重 --no-hooks 与已加载配置）；
    // 仅当上下文未注入时（如非 Agent 路径的独立调用）才回退到现场加载。
    let owned_manager;
    let hook_manager: &crate::hooks::HookManager = match context.hooks.as_ref() {
        Some(m) => m,
        None => {
            owned_manager = crate::hooks::HookManager::load(&context.working_dir, true);
            &owned_manager
        }
    };
    let output = hook_manager.execute_event(event, name_filter);

    if output.is_empty() {
        Ok(ToolResult {
            success: true,
            security_evaluation: None,
            restart_requested: false,
            error_category: None,
            content: format!(
                "[run_hook] No hooks executed for event '{}'{}.",
                event_str,
                name_filter.map(|n| format!(" (name filter: '{}')", n)).unwrap_or_default()
            ),
        })
    } else {
        Ok(ToolResult {
            success: true,
            security_evaluation: None,
            restart_requested: false,
            error_category: None,
            content: format!("[run_hook] Output of event '{}':\n{}", event_str, output),
        })
    }
}

pub fn finish_tool() -> ToolDefinition {
    ToolDefinition {
        name: "finish".to_string(),
        description: "Finish the task and provide a structured summary. Include: what was accomplished, key findings/decisions, modified files (if any), and unresolved issues (if any). Use this when the task is complete — do not just output text.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Structured completion summary: accomplishments, key findings/decisions, modified files, unresolved issues"
                }
            },
            "required": ["summary"]
        }),
        skip_security: true,
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
                error_category: None,
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
        skip_security: true,
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
                error_category: None,
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
        error_category: None,
        content: format!(
            "[restart] Restart requested. State will be saved to: {}\n\
             The process will restart after cargo build completes.",
            state_file.display()
        ),
    })
}
