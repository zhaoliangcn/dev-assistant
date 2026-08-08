//! 上下文预算管理工具。
//!
//! 提供 `context_budget`、`compress_context`、`save_summary` 三个工具，
//! 让 Agent 能感知和管理自己的上下文使用情况。
//!
//! 注意：这些工具的 handler 是 dummy 实现，实际的逻辑由 Agent 层在
//! `process_tool_calls` 中拦截处理（与 `spawn_subagent` 相同的模式）。

use super::{ToolArgs, ToolContext, ToolDefinition, ToolResult};
use crate::utils::error::AppError;

/// `context_budget` 工具定义。
///
/// 查询当前上下文预算使用情况，返回系统提示、记忆、历史各占用的 token 数，
/// 以及使用率、压力等级和剩余可用空间。
pub fn context_budget_tool() -> ToolDefinition {
    ToolDefinition {
        name: "context_budget".to_string(),
        description:
            "查询当前上下文预算使用情况。返回系统提示、记忆、历史各占用的 token 数，\
             以及使用率、压力等级和剩余可用空间。在每 3-5 轮工具调用后检查一次，\
             帮助规划后续行为。"
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        skip_security: true,
        handler: Box::new(context_budget_handler),
    }
}

/// `compress_context` 工具定义。
///
/// 主动压缩上下文。使用智能策略（根据当前压力等级选择最佳压缩方式）。
/// 调用后旧的对话会被摘要替换，释放上下文空间。
pub fn compress_context_tool() -> ToolDefinition {
    ToolDefinition {
        name: "compress_context".to_string(),
        description:
            "主动压缩上下文以释放空间。使用智能策略根据当前压力等级自动选择最佳压缩方式。\
             调用后旧的对话会被摘要替换，释放上下文空间供后续使用。\
             推荐在 context_budget 显示压力等级为 Critical 时调用。"
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "strategy": {
                    "type": "string",
                    "enum": ["auto", "summarize", "truncate"],
                    "description": "压缩策略：auto=自动选择（推荐，根据压力等级自动选择），summarize=摘要压缩（保留语义），truncate=截断压缩（紧急时使用）",
                    "default": "auto"
                }
            },
            "required": []
        }),
        skip_security: true,
        handler: Box::new(compress_context_handler),
    }
}

/// `save_summary` 工具定义。
///
/// 将当前对话的关键信息保存为摘要到知识库。用于在上下文紧张时，
/// 先保存关键信息再压缩，避免信息丢失。
pub fn save_summary_tool() -> ToolDefinition {
    ToolDefinition {
        name: "save_summary".to_string(),
        description:
            "将当前对话的关键信息保存为摘要到知识库（KB）。\
             用于在上下文紧张时，先保存关键信息再压缩，避免信息丢失。\
             保存的内容包括：已完成的步骤、关键决策、待处理事项、引用的文件等。"
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "需要保存的关键信息摘要（Markdown 格式）。应包含：已完成步骤、关键决策、待处理事项、引用的文件。"
                },
                "tags": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "标签列表，便于后续检索（如：[\"context-summary\", \"session-123\"]）",
                    "default": ["context-summary"]
                }
            },
            "required": ["content"]
        }),
        skip_security: true,
        handler: Box::new(save_summary_handler),
    }
}

// ---------------------------------------------------------------------------
// Dummy handlers（不会被调用，逻辑在 Agent::process_tool_calls 中拦截）
// ---------------------------------------------------------------------------

fn context_budget_handler(_args: &ToolArgs, _context: &ToolContext) -> Result<ToolResult, AppError> {
    Ok(ToolResult {
        success: true,
        content: "[context_budget] 此工具由 Agent 层拦截处理".to_string(),
        security_evaluation: None,
        restart_requested: false,
        error_category: None,
    })
}

fn compress_context_handler(_args: &ToolArgs, _context: &ToolContext) -> Result<ToolResult, AppError> {
    Ok(ToolResult {
        success: true,
        content: "[compress_context] 此工具由 Agent 层拦截处理".to_string(),
        security_evaluation: None,
        restart_requested: false,
        error_category: None,
    })
}

fn save_summary_handler(_args: &ToolArgs, _context: &ToolContext) -> Result<ToolResult, AppError> {
    Ok(ToolResult {
        success: true,
        content: "[save_summary] 此工具由 Agent 层拦截处理".to_string(),
        security_evaluation: None,
        restart_requested: false,
        error_category: None,
    })
}