use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tracing::{debug, warn};

use crate::agent::AgentIdentity;
use crate::llm::ToolSchema;
use crate::security::{ApprovalManager, SecurityEvaluation, SecurityPolicy};
use crate::utils::error::AppError;

pub mod file;
pub mod analysis;
pub mod kb;
pub mod meta_tools;
pub mod spec;
pub mod subagent;
pub mod system_tools;
pub mod task_tools;
pub mod common;
pub mod cache;
pub mod async_tool;
pub mod resources;
pub mod retry;


pub type ToolHandler =
    dyn Fn(&ToolArgs, &ToolContext) -> Result<ToolResult, AppError> + Sync + Send + 'static;

/// 工具注册中心。持有所有工具定义和安全策略。
///
/// 安全策略使用 `Arc<SecurityPolicy>` 共享，避免生命周期参数污染类型签名。
pub struct ToolRegistry {
    tools: HashMap<String, ToolDefinition>,
    working_dir: PathBuf,
    pub security: Arc<SecurityPolicy>,
    pub approval_manager: Arc<ApprovalManager>,
    pub retry_manager: retry::RetryManager,
    pub resources: Option<crate::tools::resources::SharedResources>,
    #[allow(dead_code)]
    schema_tokens: Cell<usize>,
}

pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    /// true 表示跳过安全评估直接执行（用于 finish/restart 等元工具）。
    pub skip_security: bool,
    handler: Box<ToolHandler>,
}

/// 工具调用的参数容器。
///
/// 当前只包装了 `arguments: Value`，但保留为新参数类型（如 `timeout`、
/// `redacted_fields` 等）的扩展点，避免未来要在 handler 签名里加参数时
/// 破坏所有 handler。
pub struct ToolArgs {
    pub arguments: Value,
}

/// 工具执行的上下文容器。
///
/// 当前只包装了 `working_dir`，但保留为未来扩展点（如执行超时、环境变量
/// 覆盖、调用方身份等）。把它简化为裸 `PathBuf` 会让这些扩展都要改 handler
/// 签名。
pub struct ToolContext {
    pub working_dir: PathBuf,
    /// 可选的资源容器，用于依赖注入
    pub resources: Option<crate::tools::resources::SharedResources>,
}

pub struct ToolResult {
    pub success: bool,
    pub content: String,
    pub security_evaluation: Option<SecurityEvaluation>,
    pub restart_requested: bool,
}

/// 工具命名空间
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolNamespace {
    DevAssistant,
    MCP,
}

/// 工具类型分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    ListDir,
    Write,
    Move,
    Search,
    Lsp,
    Execute,
    Plan,
    WebSearch,
    WebFetch,
    KnowledgeBase,
    #[serde(other)]
    Other,
}

impl ToolKind {
    pub fn is_read_only(&self) -> bool {
        matches!(self, ToolKind::Read | ToolKind::Search | ToolKind::ListDir | ToolKind::WebSearch | ToolKind::WebFetch | ToolKind::KnowledgeBase)
    }
    
    pub fn requires_approval(&self) -> bool {
        !self.is_read_only()
    }
}

/// 工具元数据 trait
pub trait ToolMetadata: Send + Sync {
    fn kind(&self) -> ToolKind;
    fn tool_namespace(&self) -> ToolNamespace;
    fn description_template(&self) -> &str;
    
    fn is_read_only(&self) -> bool {
        self.kind().is_read_only()
    }
    
    fn requires_approval(&self) -> bool {
        self.kind().requires_approval()
    }
}

impl ToolRegistry {
    pub fn new(working_dir: PathBuf, security: Arc<SecurityPolicy>) -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
            working_dir,
            security,
            approval_manager: Arc::new(ApprovalManager::new()),
            retry_manager: retry::RetryManager::default(),
            resources: None,
            schema_tokens: Cell::new(0),
        };
        registry.register_builtin_tools();
        registry
    }
    
    pub fn new_with_resources(
        working_dir: PathBuf, 
        security: Arc<SecurityPolicy>,
        resources: crate::tools::resources::SharedResources,
        approval_manager: Arc<ApprovalManager>,
    ) -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
            working_dir,
            security,
            approval_manager,
            retry_manager: retry::RetryManager::default(),
            resources: Some(resources),
            schema_tokens: Cell::new(0),
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
        self.register(subagent::spawn_subagent_tool());
        self.register(kb::kb_store_tool());
        self.register(kb::kb_query_tool());
        self.register(task_tools::task_status_tool());
        self.register(task_tools::pause_task_tool());
        self.register(task_tools::resume_task_tool());
        self.register(task_tools::cancel_task_tool());
        self.register(analysis::analyze_codebase_tool());
        self.register(analysis::record_analysis_tool());
        self.register(analysis::get_analysis_summary_tool());
        self.register(analysis::finish_analysis_tool());
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

    #[allow(dead_code)]
    pub fn schema_token_count(&self) -> usize {
        if self.schema_tokens.get() == 0 {
            let schemas = self.get_tool_schemas();
            if let Ok(json) = serde_json::to_string(&schemas) {
                self.schema_tokens.set(crate::agent::token_counter::TokenCounter::estimate(&json));
            }
        }
        self.schema_tokens.get()
    }

    /// 执行工具：先做安全评估，再根据评估结果决定是否执行。
    ///
    /// - `Critical`：阻止执行，返回带评估信息的失败结果
    /// - `High` / `Medium`：检查审批管理器，有有效审批则执行，否则返回需要审批的结果
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
            ref level @ (crate::security::DangerLevel::High | crate::security::DangerLevel::Medium) => {
                // 检查是否已有有效审批
                if self.approval_manager.requires_approval(name, name, level) {
                    warn!(tool = name, level = ?level, reason = %evaluation.reason, "Tool requires approval");
                    Ok(ToolResult {
                        success: false,
                        content: format!(
                            "⚠️  This command requires approval ({}): {}\n\
                             Press Enter to approve, or type 'cancel' to skip.",
                            level.as_str(),
                            evaluation.reason
                        ),
                        security_evaluation: Some(evaluation),
                        restart_requested: false,
                    })
                } else {
                    debug!(tool = name, level = ?level, "Tool execution approved by permission store");
                    self.execute_tool(name, arguments)
                }
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

    /// 创建子 Agent 的受限工具注册中心。
    ///
    /// 子 Agent 不应拥有以下工具：
    /// - `spawn_subagent`（防止无限递归）
    /// - `restart`（重启整个进程）
    /// 子 Agent 拥有基本的文件操作工具和 `finish`。
    pub fn new_subagent_registry(&self) -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
            working_dir: self.working_dir.clone(),
            security: self.security.clone(),
            approval_manager: self.approval_manager.clone(),
            retry_manager: self.retry_manager.clone(),
            resources: self.resources.clone(),
            schema_tokens: Cell::new(0),
        };
        // 子 Agent 只能使用文件工具、系统工具和 finish
        registry.register(file::read_file_tool());
        registry.register(file::batch_read_files_tool());
        registry.register(file::write_file_tool());
        registry.register(file::edit_file_tool());
        registry.register(file::glob_tool());
        registry.register(file::list_directory_tool());
        registry.register(file::file_exists_tool());
        registry.register(system_tools::exec_command_tool());
        registry.register(meta_tools::finish_tool());
        registry
    }

    /// 根据 Agent 身份创建受限工具注册中心。
    ///
    /// 不同身份的 Agent 拥有不同的工具集：
    /// - Architect: read_file, write_file, glob, kb_store, kb_query, finish
    /// - Implementer: read_file, write_file, edit_file, exec_command, glob, kb_query, finish
    /// - Reviewer: read_file, batch_read_files, glob, kb_store, kb_query, finish
    /// - Tester: read_file, write_file, edit_file, exec_command, glob, kb_store, kb_query, finish
    /// - Debugger: read_file, write_file, edit_file, exec_command, glob, kb_store, kb_query, finish
    /// - General: 所有基础工具
    pub fn new_subagent_registry_with_identity(&self, identity: &AgentIdentity) -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
            working_dir: self.working_dir.clone(),
            security: self.security.clone(),
            approval_manager: self.approval_manager.clone(),
            retry_manager: self.retry_manager.clone(),
            resources: self.resources.clone(),
            schema_tokens: Cell::new(0),
        };

        let allowed_tools = identity.default_tools();

        if allowed_tools.contains("read_file") {
            registry.register(file::read_file_tool());
        }
        if allowed_tools.contains("batch_read_files") {
            registry.register(file::batch_read_files_tool());
        }
        if allowed_tools.contains("write_file") {
            registry.register(file::write_file_tool());
        }
        if allowed_tools.contains("edit_file") {
            registry.register(file::edit_file_tool());
        }
        if allowed_tools.contains("glob") {
            registry.register(file::glob_tool());
        }
        if allowed_tools.contains("list_directory") {
            registry.register(file::list_directory_tool());
        }
        if allowed_tools.contains("file_exists") {
            registry.register(file::file_exists_tool());
        }
        if allowed_tools.contains("exec_command") {
            registry.register(system_tools::exec_command_tool());
        }
        if allowed_tools.contains("kb_store") {
            registry.register(kb::kb_store_tool());
        }
        if allowed_tools.contains("kb_query") {
            registry.register(kb::kb_query_tool());
        }
        if allowed_tools.contains("finish") {
            registry.register(meta_tools::finish_tool());
        }

        registry
    }

    /// 根据工具的 `skip_security` 标记决定走 `execute`（带安全评估）还是 `execute_approved`（跳过）。
    ///
    /// 这是给 Agent 层的统一入口，避免调用方根据工具名硬编码分流。
    pub fn execute_with_policy(&self, name: &str, arguments: Value) -> Result<ToolResult, AppError> {
        let skip = self
            .get_tool(name)
            .map(|t| t.skip_security)
            .unwrap_or(false);
        if skip {
            self.execute_approved(name, arguments)
        } else {
            self.execute(name, arguments)
        }
    }

    /// Internal method that executes the tool handler without security checks.
    fn execute_tool(&self, name: &str, arguments: Value) -> Result<ToolResult, AppError> {
        let tool = self
            .get_tool(name)
            .ok_or_else(|| AppError::ToolNotFound(name.to_string()))?;

        let args = ToolArgs { arguments };
        let context = ToolContext {
            working_dir: self.working_dir.clone(),
            resources: self.resources.clone(),
        };

        // 使用重试管理器执行工具，只对可重试错误进行重试
        let result = self.retry_manager.execute_with_retry_sync_condition(
            name,
            || (tool.handler)(&args, &context),
            |e| e.is_retryable(),
        );
        result.map(|r| ToolResult {
            security_evaluation: None,
            ..r
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecurityPolicy;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn schema_token_count_returns_non_zero() {
        let policy = Arc::new(SecurityPolicy::new(PathBuf::new().as_path(), true));
        let registry = ToolRegistry::new(PathBuf::new(), policy);
        let tokens = registry.schema_token_count();
        assert!(tokens > 0, "schema token count should be non-zero");
    }
}
