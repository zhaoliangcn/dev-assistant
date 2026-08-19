//! `spawn_subagent` 工具定义。
//!
//! 该工具允许 Agent 创建子 Agent 执行独立任务。
//! 工具 handler 只做参数验证，实际的子 Agent 创建由 Agent 层在
//! `process_tool_calls` 中拦截并处理（因此 handler 是 dummy 实现，
//! 永远不会被 `ToolRegistry::execute_tool` 调用）。

use super::{ToolArgs, ToolContext, ToolDefinition, ToolResult};
use crate::utils::error::AppError;

/// `spawn_subagent` 工具定义。
///
/// LLM 调用此工具来创建一个子 Agent 执行独立任务。
/// 子 Agent 拥有独立的上下文、工具集（受限）和迭代循环。
///
/// 注意：此工具在 `process_tool_calls` 中被拦截处理，
/// handler 是 dummy 实现（不会被调用）。
pub fn spawn_subagent_tool() -> ToolDefinition {
    ToolDefinition {
        name: "spawn_subagent".to_string(),
        description: "Create a sub-agent for independent subtasks. Has its own context and tools, reports back with results.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Task description for the sub-agent. Be specific."
                },
                "context": {
                    "type": "string",
                    "description": "Context for the sub-agent: file paths, interface definitions, background knowledge."
                },
                "agent_type": {
                    "type": "string",
                    "description": "Sub-agent type: architect/implementer/reviewer/tester/debugger/general. Default: general.",
                    "enum": ["architect", "implementer", "reviewer", "tester", "debugger", "general"],
                    "default": "general"
                },
                "max_iterations": {
                    "type": "integer",
                    "description": "Max iterations (default: 30). Lower for simple tasks, higher for complex.",
                    "default": 30
                },
                "max_tokens": {
                    "type": "integer",
                    "description": "Context budget for sub-agent (default: 262144). Set higher for context-heavy tasks.",
                    "default": 262144
                }
            },
            "required": ["task"]
        }),
        skip_security: true,
        handler: Box::new(spawn_subagent_handler),
    }
}

/// Dummy handler：此函数不会被 `ToolRegistry::execute_tool` 调用，
/// 因为 `spawn_subagent` 在 `Agent::process_tool_calls` 中被拦截。
/// 仅作为 `ToolDefinition` 的必需字段存在。
fn spawn_subagent_handler(args: &ToolArgs, _context: &ToolContext) -> Result<ToolResult, AppError> {
    let task = args.arguments["task"]
        .as_str()
        .ok_or_else(|| AppError::Llm("spawn_subagent: 'task' is required".to_string()))?;

    let context = args.arguments["context"]
        .as_str()
        .unwrap_or("");

    let agent_type = args.arguments["agent_type"]
        .as_str()
        .unwrap_or("general");

    Ok(ToolResult {
        success: true,
        security_evaluation: None,
        restart_requested: false,
                error_category: None,
        content: format!(
            "[spawn_subagent] Task: {}\nContext: {}\nAgent Type: {}",
            task, context, agent_type
        ),
    })
}