use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;
use tracing::{debug, warn};

use crate::llm::ToolSchema;
use crate::security::{SecurityEvaluation, SecurityPolicy};
use crate::utils::error::AppError;

pub mod file_tools;
pub mod meta_tools;
pub mod system_tools;

pub type ToolHandler =
    dyn Fn(&ToolArgs, &ToolContext) -> Result<ToolResult, AppError> + Sync + Send + 'static;

pub struct ToolRegistry<'a> {
    tools: HashMap<String, ToolDefinition>,
    working_dir: PathBuf,
    pub security: &'a SecurityPolicy,
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

impl<'a> ToolRegistry<'a> {
    pub fn new(working_dir: PathBuf, security: &'a SecurityPolicy) -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
            working_dir,
            security,
        };
        registry.register_builtin_tools();
        registry
    }

    fn register_builtin_tools(&mut self) {
        self.register(file_tools::read_file_tool());
        self.register(file_tools::batch_read_files_tool());
        self.register(file_tools::write_file_tool());
        self.register(file_tools::edit_file_tool());
        self.register(file_tools::glob_tool());
        self.register(file_tools::list_directory_tool());
        self.register(file_tools::file_exists_tool());
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

    pub fn execute(&self, name: &str, arguments: Value) -> Result<ToolResult, AppError> {
        debug!(tool = name, "Executing tool");

        // Security check before execution
        let evaluation = self.security.evaluate_tool(name, &arguments);
        match evaluation.danger_level {
            crate::security::DangerLevel::Critical => {
                warn!(tool = name, reason = %evaluation.reason, "Tool blocked by security policy");
                return Ok(ToolResult {
                    success: false,
                    content: format!("🚫 Command blocked (CRITICAL): {}", evaluation.reason),
                    security_evaluation: Some(evaluation),
                    restart_requested: false,
                });
            }
            crate::security::DangerLevel::High | crate::security::DangerLevel::Medium => {
                // Return the evaluation to the Agent; it will prompt the user for approval
                warn!(tool = name, level = ?evaluation.danger_level, reason = %evaluation.reason, "Tool requires approval");
                return Ok(ToolResult {
                    success: false,
                    content: format!(
                        "⚠️  This command requires approval ({}): {}\n\
                         Press Enter to approve, or type 'cancel' to skip.",
                        evaluation.danger_level.as_str(),
                        evaluation.reason
                    ),
                    security_evaluation: Some(evaluation),
                    restart_requested: false,
                });
            }
            crate::security::DangerLevel::Low => {}
        }

        self.execute_tool(name, arguments)
    }

    /// Execute a tool directly without performing security evaluation.
    /// This should only be used after the user has explicitly approved the tool,
    /// or for tools that bypass security checks (e.g., finish, restart).
    /// Note: Critical-level commands are still blocked even after approval.
    pub fn execute_approved(&self, name: &str, arguments: Value) -> Result<ToolResult, AppError> {
        debug!(tool = name, "Executing approved tool (security check bypassed for non-critical)");
        
        // 仍然进行安全评估，但 Critical 级别仍然阻止
        let evaluation = self.security.evaluate_tool(name, &arguments);
        match evaluation.danger_level {
            crate::security::DangerLevel::Critical => {
                warn!(tool = name, reason = %evaluation.reason, "Tool blocked by security policy even after approval");
                Ok(ToolResult {
                    success: false,
                    content: format!("🚫 Command blocked (CRITICAL): {}", evaluation.reason),
                    security_evaluation: Some(evaluation),
                    restart_requested: false,
                })
            }
            _ => {
                // 用户已批准，直接执行（跳过需要交互的检查）
                self.execute_tool(name, arguments)
            }
        }
    }

    /// Internal method that executes the tool handler without security checks.
    fn execute_tool(&self, name: &str, arguments: Value) -> Result<ToolResult, AppError> {
        let tool = self
            .get_tool(name)
            .ok_or_else(|| AppError::Llm(format!("Unknown tool: {}", name)))?;

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
