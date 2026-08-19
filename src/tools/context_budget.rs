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
            "查询上下文预算使用情况：系统提示、记忆、历史各占用的 token 数、使用率、压力等级、剩余空间。每 3-5 轮调用一次。"
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
            "主动压缩上下文：根据压力等级自动选择最优策略（摘要/截断），释放上下文空间。推荐 Critical 等级时调用。"
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "strategy": {
                    "type": "string",
                    "enum": ["auto", "summarize", "truncate"],
                    "description": "auto/summarize/truncate。auto=自动（推荐），summarize=摘要，truncate=截断",
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
            "将关键信息保存为摘要到 KB。上下文紧张时先保存再压缩，避免信息丢失。"
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "关键信息摘要（Markdown）：已完成步骤、关键决策、待处理事项、引用的文件。"
                },
                "tags": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "标签，便于检索（如 [\"context-summary\"]）",
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