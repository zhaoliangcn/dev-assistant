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
        description: "Create a sub-agent to execute a task independently. Use this when a task can be decomposed into independent subtasks that can be worked on separately. The sub-agent will have its own context and tool set, and will report back with results.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Task description for the sub-agent. Be specific about what the sub-agent should accomplish."
                },
                "context": {
                    "type": "string",
                    "description": "Context information to pass to the sub-agent, such as relevant file paths, interface definitions, or background knowledge."
                },
                "agent_type": {
                    "type": "string",
                    "description": "Type of sub-agent to create. Available types: architect, implementer, reviewer, tester, debugger, general. Each type has specialized skills and tools. Default: general.",
                    "enum": ["architect", "implementer", "reviewer", "tester", "debugger", "general"],
                    "default": "general"
                },
                "max_iterations": {
                    "type": "integer",
                    "description": "Maximum iterations for the sub-agent (default: 15). Set lower for simple tasks, higher for complex ones.",
                    "default": 15
                },
                "max_tokens": {
                    "type": "integer",
                    "description": "Token budget for the sub-agent's context (default: 8192). Set higher for tasks requiring large context.",
                    "default": 8192
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