use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tracing::{debug, warn};

use crate::llm::ToolSchema;
use crate::security::{SecurityEvaluation, SecurityPolicy};
use crate::utils::error::AppError;

pub mod file;
pub mod meta_tools;
pub mod spec;
pub mod system_tools;

pub type ToolHandler =
    dyn Fn(&ToolArgs, &ToolContext) -> Result<ToolResult, AppError> + Sync + Send + 'static;

/// 工具注册中心。持有所有工具定义和安全策略。
///
/// 安全策略使用 `Arc<SecurityPolicy>` 共享，避免生命周期参数污染类型签名。
pub struct ToolRegistry {
    tools: HashMap<String, ToolDefinition>,
    working_dir: PathBuf,
    pub security: Arc<SecurityPolicy>,
}

pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    handler: Box<ToolHandler>,
}

pub struct ToolArgs {
    pub arguments: Value,
}

pub struct ToolContext {
    pub working_dir: PathBuf,
}

pub struct ToolResult {
    pub success: bool,
    pub content: String,
    pub security_evaluation: Option<SecurityEvaluation>,
    pub restart_requested: bool,
}

impl ToolRegistry {
    pub fn new(working_dir: PathBuf, security: Arc<SecurityPolicy>) -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
            working_dir,
            security,
        };
        registry.register_builtin_tools();
        registry
    }

    fn register_builtin_tools(&mut self) {
        self.register(file::read_file_tool());
        self.register(file::batch_read_files_tool());
        self.register(file::write_file_tool());
        self.register(file::edit_file_tool());
        self.register(file::glob_tool());
        self.register(file::list_directory_tool());
        self.register(file::file_exists_tool());
        self.register(system_tools::exec_command_tool());
        self.register(meta_tools::finish_tool());
        self.register(meta_tools::restart_tool());
    }

    fn register(&mut self, tool: ToolDefinition) {
        self.tools.insert(tool.name.clone(), tool);
    }

    pub fn get_tool(&self, name: &str) -> Option<&ToolDefinition> {
        self.tools.get(name)
    }

    pub fn get_tool_schemas(&self) -> Vec<ToolSchema> {
        self.tools
            .values()
            .map(|t| ToolSchema {
                tool_type: "function".to_string(),
                function: crate::llm::ToolFunctionSchema {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                },
            })
            .collect()
    }

    /// 执行工具：先做安全评估，再根据评估结果决定是否执行。
    ///
    /// - `Critical`：阻止执行，返回带评估信息的失败结果
    /// - `High` / `Medium`：返回需要审批的失败结果（调用方负责提示用户）
    /// - `Low`：直接执行
    pub fn execute(&self, name: &str, arguments: Value) -> Result<ToolResult, AppError> {
        debug!(tool = name, "Executing tool");

        let evaluation = self.security.evaluate_tool(name, &arguments);
        match evaluation.danger_level {
            crate::security::DangerLevel::Critical => {
                warn!(tool = name, reason = %evaluation.reason, "Tool blocked by security policy");
                Ok(ToolResult {
                    success: false,
                    content: format!("🚫 Command blocked (CRITICAL): {}", evaluation.reason),
                    security_evaluation: Some(evaluation),
                    restart_requested: false,
                })
            }
            crate::security::DangerLevel::High | crate::security::DangerLevel::Medium => {
                warn!(tool = name, level = ?evaluation.danger_level, reason = %evaluation.reason, "Tool requires approval");
                Ok(ToolResult {
                    success: false,
                    content: format!(
                        "⚠️  This command requires approval ({}): {}\n\
                         Press Enter to approve, or type 'cancel' to skip.",
                        evaluation.danger_level.as_str(),
                        evaluation.reason
                    ),
                    security_evaluation: Some(evaluation),
                    restart_requested: false,
                })
            }
            crate::security::DangerLevel::Low => self.execute_tool(name, arguments),
        }
    }

    /// 直接执行工具，跳过安全评估。
    ///
    /// 仅用于：
    /// - 元工具（finish/restart）——它们不操作外部资源
    /// - 用户已显式批准的工具
    pub fn execute_approved(&self, name: &str, arguments: Value) -> Result<ToolResult, AppError> {
        debug!(tool = name, "Executing approved tool (security check bypassed)");
        self.execute_tool(name, arguments)
    }

    /// Internal method that executes the tool handler without security checks.
    fn execute_tool(&self, name: &str, arguments: Value) -> Result<ToolResult, AppError> {
        let tool = self
            .get_tool(name)
            .ok_or_else(|| AppError::ToolNotFound(name.to_string()))?;

        let args = ToolArgs { arguments };
        let context = ToolContext {
            working_dir: self.working_dir.clone(),
        };

        let result = (tool.handler)(&args, &context);
        result.map(|r| ToolResult {
            security_evaluation: None,
            ..r
        })
    }
}
