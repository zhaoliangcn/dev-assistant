use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, warn};

use crate::security::{ApprovalManager, SecurityPolicy};
use crate::utils::error::AppError;


pub use crate::tools::cache::{CacheConfig, ReadCache};

/// 异步工具处理函数类型（使用拥有所有权的参数）
pub type AsyncToolHandler =
    dyn Fn(ToolArgs, ToolContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolResult, AppError>> + Send>> + Sync + Send + 'static;

/// 工具调用的参数容器
pub struct ToolArgs {
    pub arguments: Value,
}

/// 工具执行的上下文容器
pub struct ToolContext {
    pub working_dir: PathBuf,
    pub cache: Option<Arc<ReadCache>>,
    pub resources: Option<crate::tools::resources::SharedResources>,
}

/// 工具结果（重新导出）
pub use crate::tools::ToolResult;

/// 异步工具定义
pub struct AsyncToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    /// true 表示跳过安全评估直接执行
    pub skip_security: bool,
    handler: Box<AsyncToolHandler>,
}

/// 异步工具 Trait
#[async_trait]
pub trait AsyncTool: Sync + Send + 'static {
    /// 获取工具名称
    fn name(&self) -> &str;
    
    /// 获取工具描述
    fn description(&self) -> &str;
    
    /// 获取工具参数 Schema
    fn parameters(&self) -> Value;
    
    /// 是否跳过安全评估
    fn skip_security(&self) -> bool {
        false
    }
    
    /// 异步执行工具
    async fn execute(&self, args: ToolArgs, context: ToolContext) -> Result<ToolResult, AppError>;
}

/// 异步工具注册中心
pub struct AsyncToolRegistry {
    tools: std::collections::HashMap<String, AsyncToolDefinition>,
    working_dir: PathBuf,
    pub security: Arc<SecurityPolicy>,
    pub approval_manager: Arc<ApprovalManager>,
    pub cache: Arc<ReadCache>,
}

impl AsyncToolRegistry {
    pub fn new(
        working_dir: PathBuf,
        security: Arc<SecurityPolicy>,
        approval_manager: Arc<ApprovalManager>,
    ) -> Self {
        Self {
            tools: std::collections::HashMap::new(),
            working_dir,
            security,
            approval_manager,
            cache: Arc::new(ReadCache::default()),
        }
    }

    /// 创建带自定义缓存配置的注册中心
    pub fn new_with_cache_config(
        working_dir: PathBuf,
        security: Arc<SecurityPolicy>,
        approval_manager: Arc<ApprovalManager>,
        cache_config: CacheConfig,
    ) -> Self {
        Self {
            tools: std::collections::HashMap::new(),
            working_dir,
            security,
            approval_manager,
            cache: Arc::new(ReadCache::new(cache_config)),
        }
    }

    fn register(&mut self, tool: AsyncToolDefinition) {
        self.tools.insert(tool.name.clone(), tool);
    }

    pub fn register_tool<T>(&mut self, tool: Arc<T>)
    where
        T: AsyncTool,
    {
        let name = tool.name().to_string();
        let description = tool.description().to_string();
        let parameters = tool.parameters();
        let skip_security = tool.skip_security();
        
        let definition = AsyncToolDefinition {
            name,
            description,
            parameters,
            skip_security,
            handler: Box::new(move |args, context| {
                let tool = tool.clone();
                Box::pin(async move {
                    tool.execute(args, context).await
                })
            }),
        };
        
        self.register(definition);
    }

    /// 直接注册工具定义
    pub fn register_definition(&mut self, definition: AsyncToolDefinition) {
        self.register(definition);
    }

    pub fn get_tool(&self, name: &str) -> Option<&AsyncToolDefinition> {
        self.tools.get(name)
    }

    /// 获取所有工具的 Schema
    pub fn get_tool_schemas(&self) -> Vec<crate::llm::ToolSchema> {
        self.tools
            .values()
            .map(|t| crate::llm::ToolSchema {
                tool_type: "function".to_string(),
                function: crate::llm::ToolFunctionSchema {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                },
            })
            .collect()
    }

    /// 异步执行工具
    pub async fn execute(&self, name: &str, arguments: Value) -> Result<ToolResult, AppError> {
        debug!(tool = name, "Executing async tool");

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
                    })
                } else {
                    debug!(tool = name, level = ?level, "Tool execution approved by permission store");
                    self.execute_tool(name, arguments).await
                }
            }
            crate::security::DangerLevel::Low => self.execute_tool(name, arguments).await,
        }
    }

    /// 直接执行工具，跳过安全评估
    pub async fn execute_approved(&self, name: &str, arguments: Value) -> Result<ToolResult, AppError> {
        debug!(tool = name, "Executing approved async tool");
        self.execute_tool(name, arguments).await
    }

    /// 根据工具的 skip_security 标记决定执行方式
    pub async fn execute_with_policy(&self, name: &str, arguments: Value) -> Result<ToolResult, AppError> {
        let skip = self
            .get_tool(name)
            .map(|t| t.skip_security)
            .unwrap_or(false);
        if skip {
            self.execute_approved(name, arguments).await
        } else {
            self.execute(name, arguments).await
        }
    }

    async fn execute_tool(&self, name: &str, arguments: Value) -> Result<ToolResult, AppError> {
        let tool = self
            .get_tool(name)
            .ok_or_else(|| AppError::ToolNotFound(name.to_string()))?;

        let args = ToolArgs { arguments };
        let context = ToolContext {
            working_dir: self.working_dir.clone(),
            cache: Some(self.cache.clone()),
            resources: None,
        };

        let result = (tool.handler)(args, context).await;
        result.map(|r| ToolResult {
            security_evaluation: None,
            ..r
        })
    }

    /// 获取缓存统计信息
    pub fn cache_stats(&self) -> crate::tools::cache::CacheStats {
        self.cache.stats()
    }

    /// 使指定路径的缓存失效
    pub fn invalidate_cache(&self, path: &std::path::Path) {
        self.cache.invalidate(path);
    }

    /// 清除所有缓存
    pub fn clear_cache(&self) {
        self.cache.clear();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecurityPolicy;
    use serde_json::json;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::tempdir;

    struct TestTool;

    #[async_trait]
    impl AsyncTool for TestTool {
        fn name(&self) -> &str {
            "test_tool"
        }

        fn description(&self) -> &str {
            "Test tool"
        }

        fn parameters(&self) -> Value {
            json!({
                "type": "object",
                "properties": {},
                "required": []
            })
        }

        async fn execute(&self, _args: ToolArgs, _context: ToolContext) -> Result<ToolResult, AppError> {
            Ok(ToolResult {
                success: true,
                content: "test result".to_string(),
                security_evaluation: None,
                restart_requested: false,
            })
        }
    }

    fn test_registry(dir: &Path) -> AsyncToolRegistry {
        let policy = Arc::new(SecurityPolicy::new(dir, true));
        let approval_manager = Arc::new(crate::security::approval::ApprovalManager::new());
        AsyncToolRegistry::new(dir.to_path_buf(), policy, approval_manager)
    }

    #[tokio::test]
    async fn async_tool_execution() {
        let dir = tempdir().unwrap();
        let mut registry = test_registry(dir.path());

        registry.register_tool(Arc::new(TestTool));

        let result = registry.execute("test_tool", json!({})).await.unwrap();
        assert!(result.success);
        assert_eq!(result.content, "test result");
    }

    #[tokio::test]
    async fn async_tool_not_found() {
        let dir = tempdir().unwrap();
        let registry = test_registry(dir.path());

        let result = registry.execute("nonexistent", json!({})).await;
        assert!(matches!(result, Err(AppError::ToolNotFound(_))));
    }

    #[tokio::test]
    async fn async_tool_schema_generation() {
        let dir = tempdir().unwrap();
        let mut registry = test_registry(dir.path());

        registry.register_tool(Arc::new(TestTool));

        let schemas = registry.get_tool_schemas();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].function.name, "test_tool");
    }

    #[tokio::test]
    async fn async_tool_cache_access() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "content").unwrap();

        let registry = test_registry(dir.path());

        // 验证缓存可访问
        assert!(registry.cache.read(&file_path).is_none());
        registry.cache.write(&file_path, "content");
        assert_eq!(registry.cache.read(&file_path), Some("content".to_string()));
    }
}
