//! 异步写入类工具：`async_write_file`、`async_edit_file`。

use async_trait::async_trait;
use serde_json::Value;

use super::async_io::{file_exists, write_file_content};
use crate::tools::async_tool::{AsyncTool, ToolArgs, ToolContext, ToolResult};
use crate::tools::common;
use crate::utils::error::AppError;

/// 异步写入文件工具
pub struct AsyncWriteFileTool;

#[async_trait]
impl AsyncTool for AsyncWriteFileTool {
    fn name(&self) -> &str {
        "async_write_file"
    }

    fn description(&self) -> &str {
        "Asynchronously write content to a file. Creates the file if it doesn't exist, overwrites if it does."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "File path relative to current directory"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                },
                "append": {
                    "type": "boolean",
                    "description": "If true, append content to the end of the file. Default: false",
                    "default": false
                }
            },
            "required": ["file_path", "content"]
        })
    }

    async fn execute(&self, args: ToolArgs, context: ToolContext) -> Result<ToolResult, AppError> {
        let file_path = args.arguments["file_path"]
            .as_str()
            .ok_or_else(|| AppError::Llm("file_path is required".to_string()))?;

        let content = args.arguments["content"]
            .as_str()
            .ok_or_else(|| AppError::Llm("content is required".to_string()))?;

        let append = args.arguments["append"].as_bool().unwrap_or(false);

        let full_path = common::resolve_model_path(&context.working_dir, file_path);
        
        // 失效缓存
        if let Some(cache) = &context.cache {
            cache.invalidate(&full_path);
        }

        if append {
            let existing = if file_exists(&full_path).await {
                match super::async_io::read_file_content(&full_path).await {
                    Ok(c) => c,
                    Err(e) => {
                        return Ok(ToolResult {
                            success: false,
                            security_evaluation: None,
                            restart_requested: false,
                error_category: None,
                            content: format!(
                                "[async_write_file] ❌ Failed to read existing file for append: {}",
                                e
                            ),
                        });
                    }
                }
            } else {
                String::new()
            };
            let new_content = format!("{}{}", existing, content);
            match write_file_content(&full_path, &new_content).await {
                Ok(_) => {}
                Err(e) => return Err(AppError::Io(e)),
            }
        } else {
            match write_file_content(&full_path, content).await {
                Ok(_) => {}
                Err(e) => return Err(AppError::Io(e)),
            }
        }

        let info = format!(
            "[async_write_file] ✅ {} '{}'",
            if append { "Appended to" } else { "Wrote" },
            file_path
        );

        Ok(ToolResult {
            success: true,
            security_evaluation: None,
            restart_requested: false,
                error_category: None,
            content: info,
        })
    }
}

/// 异步编辑文件工具
pub struct AsyncEditFileTool;

#[async_trait]
impl AsyncTool for AsyncEditFileTool {
    fn name(&self) -> &str {
        "async_edit_file"
    }

    fn description(&self) -> &str {
        "Asynchronously edit a file by replacing all occurrences of a pattern with new content."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "File path relative to current directory"
                },
                "old_str": {
                    "type": "string",
                    "description": "The string to search for and replace"
                },
                "new_str": {
                    "type": "string",
                    "description": "The replacement string"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of replacements to make. 0 means unlimited. Default: 0",
                    "default": 0
                }
            },
            "required": ["file_path", "old_str", "new_str"]
        })
    }

    async fn execute(&self, args: ToolArgs, context: ToolContext) -> Result<ToolResult, AppError> {
        let file_path = args.arguments["file_path"]
            .as_str()
            .ok_or_else(|| AppError::Llm("file_path is required".to_string()))?;

        let old_str = args.arguments["old_str"]
            .as_str()
            .ok_or_else(|| AppError::Llm("old_str is required".to_string()))?;

        let new_str = args.arguments["new_str"]
            .as_str()
            .ok_or_else(|| AppError::Llm("new_str is required".to_string()))?;

        let limit = common::get_lenient_usize(&args.arguments["limit"], "limit", 0)
            .map_err(AppError::Llm)?;

        let full_path = common::resolve_model_path(&context.working_dir, file_path);

        // 读取文件内容
        let content = match super::async_io::read_file_content(&full_path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ToolResult {
                    success: false,
                    security_evaluation: None,
                    restart_requested: false,
                error_category: None,
                    content: format!(
                        "[async_edit_file] ❌ File not found: {}",
                        file_path
                    ),
                });
            }
            Err(e) => return Err(AppError::Io(e)),
        };

        // 执行替换
        let (new_content, replacements) = if limit == 0 {
            let count = content.matches(old_str).count();
            (content.replace(old_str, new_str), count)
        } else {
            let mut replaced = 0;
            let result: String = content.split(old_str).enumerate().map(|(i, part)| {
                if i > 0 && replaced < limit {
                    replaced += 1;
                    format!("{}{}", new_str, part)
                } else if i > 0 {
                    format!("{}{}", old_str, part)
                } else {
                    part.to_string()
                }
            }).collect();
            (result, replaced)
        };

        if replacements == 0 {
            return Ok(ToolResult {
                success: false,
                security_evaluation: None,
                restart_requested: false,
                error_category: None,
                content: format!(
                    "[async_edit_file] ❌ Pattern '{}' not found in {}",
                    old_str, file_path
                ),
            });
        }

        // 写入文件
        match write_file_content(&full_path, &new_content).await {
            Ok(_) => {}
            Err(e) => return Err(AppError::Io(e)),
        }

        // 失效缓存
        if let Some(cache) = &context.cache {
            cache.invalidate(&full_path);
        }

        let info = format!(
            "[async_edit_file] ✅ Replaced {} occurrence(s) of '{}' in {}",
            replacements, old_str, file_path
        );

        Ok(ToolResult {
            success: true,
            security_evaluation: None,
            restart_requested: false,
                error_category: None,
            content: info,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecurityPolicy;
    use crate::tools::async_tool::AsyncToolRegistry;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[tokio::test]
    async fn async_write_file_execution() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        let working_dir = dir.path().to_path_buf();
        
        let security = Arc::new(SecurityPolicy::new(&working_dir, false));
        let approval_manager = Arc::new(crate::security::approval::ApprovalManager::new());
        let mut registry = AsyncToolRegistry::new(working_dir, security, approval_manager);
        
        registry.register_tool(Arc::new(AsyncWriteFileTool));
        
        let result = registry.execute_approved("async_write_file", json!({
            "file_path": "test.txt",
            "content": "Hello World!"
        })).await;
        
        assert!(result.is_ok());
        assert!(result.unwrap().success);
        
        // 验证文件内容
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Hello World!");
    }

    #[tokio::test]
    async fn async_edit_file_execution() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "Hello World!").unwrap();
        
        let working_dir = dir.path().to_path_buf();
        
        let security = Arc::new(SecurityPolicy::new(&working_dir, false));
        let approval_manager = Arc::new(crate::security::approval::ApprovalManager::new());
        let mut registry = AsyncToolRegistry::new(working_dir, security, approval_manager);
        
        registry.register_tool(Arc::new(AsyncEditFileTool));
        
        let result = registry.execute_approved("async_edit_file", json!({
            "file_path": "test.txt",
            "old_str": "World",
            "new_str": "Async"
        })).await;
        
        assert!(result.is_ok());
        assert!(result.unwrap().success);
        
        // 验证文件内容
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Hello Async!");
    }
}
