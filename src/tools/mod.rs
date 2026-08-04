use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    schema_tokens: AtomicUsize,
}

pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    /// true 表示跳过安全评估直接执行（用于 finish/restart 等元工具）。
    pub skip_security: bool,
    pub(crate) handler: Box<ToolHandler>,
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

/// 错误类别，用于区分可重试和不可重试的错误
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ErrorCategory {
    /// 临时性错误（I/O 错误、网络超时等），可重试
    #[default]
    Transient,
    /// 永久性错误（工具不存在、安全拒绝等），不可重试
    Permanent,
    /// LLM 返回的错误，可能可重试
    Llm,
}

pub struct ToolResult {
    pub success: bool,
    pub content: String,
    pub security_evaluation: Option<SecurityEvaluation>,
    pub restart_requested: bool,
    /// 错误类别，仅在 `success = false` 时有意义。
    /// `None` 表示未分类错误（向后兼容）。
    #[allow(dead_code)] // used by retry logic in execute_tool
    pub error_category: Option<ErrorCategory>,
}

impl ToolResult {
    /// 创建成功结果
    pub fn success(content: String) -> Self {
        Self {
            success: true,
            content,
            security_evaluation: None,
            restart_requested: false,
            error_category: None,
        }
    }

    /// 创建失败结果（带错误类别）
    pub fn failure(content: String, category: ErrorCategory) -> Self {
        Self {
            success: false,
            content,
            security_evaluation: None,
            restart_requested: false,
            error_category: Some(category),
        }
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
            schema_tokens: AtomicUsize::new(0),
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
            schema_tokens: AtomicUsize::new(0),
        };
        registry.register_builtin_tools();
        registry
    }

    fn register_builtin_tools(&mut self) {
        self.register_tools_by_names(&[
            "read_file", "batch_read_files", "write_file", "edit_file",
            "read_symbol",
            "glob", "list_directory", "file_exists", "exec_command",
            "finish", "restart", "spawn_subagent",
            "kb_store", "kb_query",
            "task_status", "pause_task", "resume_task", "cancel_task",
            "analyze_codebase", "record_analysis", "get_analysis_summary", "finish_analysis",
            "schedule_task", "unschedule_task", "list_scheduled_tasks", "get_scheduled_task_logs",
        ]);
    }

    /// 根据工具名列表注册工具。工具名到工厂函数的映射表定义在此处，
    /// 新增工具只需在此表中添加一行，避免在多处重复注册逻辑。
    fn register_tools_by_names(&mut self, names: &[&str]) {
        for name in names {
            if let Some(tool) = self.create_tool_by_name(name) {
                self.register(tool);
            }
        }
    }

    /// 根据工具名创建工具定义。单一映射表，新增工具只需在此处添加。
    fn create_tool_by_name(&self, name: &str) -> Option<ToolDefinition> {
        match name {
            "read_file" => Some(file::read_file_tool()),
            "read_symbol" => Some(file::read_symbol_tool()),
            "batch_read_files" => Some(file::batch_read_files_tool()),
            "write_file" => Some(file::write_file_tool()),
            "edit_file" => Some(file::edit_file_tool()),
            "glob" => Some(file::glob_tool()),
            "list_directory" => Some(file::list_directory_tool()),
            "file_exists" => Some(file::file_exists_tool()),
            "exec_command" => Some(system_tools::exec_command_tool()),
            "finish" => Some(meta_tools::finish_tool()),
            "restart" => Some(meta_tools::restart_tool()),
            "spawn_subagent" => Some(subagent::spawn_subagent_tool()),
            "kb_store" => Some(kb::kb_store_tool()),
            "kb_query" => Some(kb::kb_query_tool()),
            "task_status" => Some(task_tools::task_status_tool()),
            "pause_task" => Some(task_tools::pause_task_tool()),
            "resume_task" => Some(task_tools::resume_task_tool()),
            "cancel_task" => Some(task_tools::cancel_task_tool()),
            "analyze_codebase" => Some(analysis::analyze_codebase_tool()),
            "record_analysis" => Some(analysis::record_analysis_tool()),
            "get_analysis_summary" => Some(analysis::get_analysis_summary_tool()),
            "finish_analysis" => Some(analysis::finish_analysis_tool()),
            "schedule_task" => Some(crate::scheduler::tools_handlers::schedule_task_tool()),
            "unschedule_task" => Some(crate::scheduler::tools_handlers::unschedule_task_tool()),
            "list_scheduled_tasks" => Some(crate::scheduler::tools_handlers::list_scheduled_tasks_tool()),
            "get_scheduled_task_logs" => Some(crate::scheduler::tools_handlers::get_scheduled_task_logs_tool()),
            _ => None,
        }
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

    #[allow(dead_code)] // reserved for future schema token budget tracking
    pub fn schema_token_count(&self) -> usize {
        let current = self.schema_tokens.load(Ordering::Relaxed);
        if current == 0 {
            let schemas = self.get_tool_schemas();
            if let Ok(json) = serde_json::to_string(&schemas) {
                let estimated = crate::agent::token_counter::TokenCounter::estimate(&json);
                self.schema_tokens.store(estimated, Ordering::Relaxed);
                return estimated;
            }
        }
        current
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
                    error_category: None,
                })
            }
            ref level @ (crate::security::DangerLevel::High | crate::security::DangerLevel::Medium) => {
                // 检查是否已有有效审批，使用参数中的具体作用域（如路径）
                let scope_id = crate::security::approval::extract_approval_scope(name, &arguments);
                if self.approval_manager.requires_approval(name, &scope_id, level) {
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
                        error_category: None,
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
    fn execute_approved(&self, name: &str, arguments: Value) -> Result<ToolResult, AppError> {
        debug!(tool = name, "Executing approved tool (security check bypassed)");
        self.execute_tool(name, arguments)
    }

    /// 获取工作目录的引用。
    pub fn working_dir(&self) -> &PathBuf {
        &self.working_dir
    }

    /// 创建子 Agent 的受限工具注册中心。
    ///
    /// 子 Agent 不应拥有以下工具：
    /// - `spawn_subagent`（防止无限递归）
    /// - `restart`（重启整个进程）
    ///
    /// 子 Agent 拥有基本的文件操作工具、KB 工具和 `finish`。
    pub fn new_subagent_registry(&self) -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
            working_dir: self.working_dir.clone(),
            security: self.security.clone(),
            approval_manager: self.approval_manager.clone(),
            retry_manager: self.retry_manager.clone(),
            resources: self.resources.clone(),
            schema_tokens: AtomicUsize::new(0),
        };
        // 子 Agent 拥有文件工具、KB 工具和 finish
        registry.register_tools_by_names(&[
            "read_file", "batch_read_files", "write_file", "edit_file",
            "glob", "list_directory", "file_exists", "exec_command",
            "kb_store", "kb_query", "finish",
        ]);
        registry
    }

    /// 根据 Agent 身份创建受限工具注册中心。
    ///
    /// 不同身份的 Agent 拥有不同的工具集（由 `AgentIdentity::default_tools()` 定义）：
    /// - Architect: 文件工具 + kb_store/kb_query + exec_command + finish
    /// - Implementer: 文件工具 + 编辑工具 + kb_store/kb_query + exec_command + finish
    /// - Reviewer: 读取工具 + exec_command + kb_store/kb_query + finish（不含写/编辑工具）
    /// - Tester: 完整工具集 + kb_store/kb_query + finish
    /// - Debugger: 完整工具集 + kb_store/kb_query + finish
    /// - General: 完整工具集 + kb_store/kb_query + finish
    pub fn new_subagent_registry_with_identity(&self, identity: &AgentIdentity) -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
            working_dir: self.working_dir.clone(),
            security: self.security.clone(),
            approval_manager: self.approval_manager.clone(),
            retry_manager: self.retry_manager.clone(),
            resources: self.resources.clone(),
            schema_tokens: AtomicUsize::new(0),
        };

        let allowed_tools = identity.default_tools();

        // 根据身份过滤工具名，然后批量注册
        let all_tool_names = [
            "read_file", "batch_read_files", "write_file", "edit_file",
            "glob", "list_directory", "file_exists", "exec_command",
            "kb_store", "kb_query", "finish",
        ];
        let filtered: Vec<&str> = all_tool_names.iter()
            .copied()
            .filter(|name| allowed_tools.contains(*name))
            .collect();
        registry.register_tools_by_names(&filtered);

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