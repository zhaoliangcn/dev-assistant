//! 异步读取类工具：`async_read_file`、`async_batch_read_files`。

use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;
use tracing::debug;

use std::sync::Arc;

use super::async_io::read_file_content;
use super::read_shared::{
    generate_code_summary, generate_read_info, resolve_glob_patterns, DEFAULT_READ_LIMIT,
};
use crate::tools::async_tool::{AsyncTool, ToolArgs, ToolContext, ToolResult};
use crate::tools::cache::ReadCache;
use crate::tools::common;
use crate::utils::error::AppError;

/// 将 IO 读取错误映射为工具友好错误信息，成功时返回文件内容。
fn map_read_result(
    result: Result<String, std::io::Error>,
    tool_name: &str,
    file_path: &str,
) -> Result<String, ToolResult> {
    match result {
        Ok(c) => Ok(c),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ToolResult {
            success: false,
            security_evaluation: None,
            restart_requested: false,
            error_category: None,
            content: format!(
                "[{}] ❌ File not found: {}\n\
                 Please check the file path. You may need to use glob to find the correct file name.",
                tool_name, file_path
            ),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Err(ToolResult {
            success: false,
            security_evaluation: None,
            restart_requested: false,
            error_category: None,
            content: format!(
                "[{}] ❌ Permission denied: {}\n\
                 The file exists but you don't have read access.",
                tool_name, file_path
            ),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Err(ToolResult {
            success: false,
            security_evaluation: None,
            restart_requested: false,
            error_category: None,
            content: format!(
                "[{}] ❌ Binary/non-UTF-8 file: {}\n\
                 This file contains binary or non-text data and cannot be displayed.\n\
                 Use exec_command with `file {}` or `xxd {}` to inspect it.",
                tool_name, file_path, file_path, file_path
            ),
        }),
        Err(e) => Err(ToolResult {
            success: false,
            security_evaluation: None,
            restart_requested: false,
            error_category: None,
            content: format!("[{}] ❌ IO error: {}: {}", tool_name, file_path, e),
        }),
    }
}

/// 通过缓存（若有）异步读取文件内容，所有错误映射为 ToolResult。
async fn read_file_with_cache(
    full_path: &Path,
    file_path: &str,
    tool_name: &str,
    cache: Option<&Arc<ReadCache>>,
) -> Result<String, ToolResult> {
    if let Some(cache) = cache {
        if let Some(cached) = cache.read_async(full_path).await {
            debug!("Cache hit for {}", file_path);
            return Ok(cached);
        }
        let content = match read_file_content(full_path).await {
            Ok(c) => c,
            Err(e) => return map_read_result(Err(e), tool_name, file_path),
        };
        cache.write_async(full_path, &content).await;
        Ok(content)
    } else {
        match read_file_content(full_path).await {
            Ok(c) => Ok(c),
            Err(e) => map_read_result(Err(e), tool_name, file_path),
        }
    }
}

/// 异步读取文件工具
pub struct AsyncReadFileTool;

#[async_trait]
impl AsyncTool for AsyncReadFileTool {
    fn name(&self) -> &str {
        "async_read_file"
    }

    fn description(&self) -> &str {
        "Asynchronously read a file from the filesystem. Supports offset/limit for reading large files in chunks."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "File path relative to current directory"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-indexed). Default: 1",
                    "default": 1
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read. Default: 500"
                }
            },
            "required": ["file_path"]
        })
    }

    async fn execute(&self, args: ToolArgs, context: ToolContext) -> Result<ToolResult, AppError> {
        let file_path = args.arguments["file_path"]
            .as_str()
            .ok_or_else(|| AppError::Llm("file_path is required".to_string()))?;

        let offset = common::get_lenient_usize(&args.arguments["offset"], "offset", 1)
            .map_err(AppError::Llm)?
            .max(1);

        let limit = common::get_lenient_usize(&args.arguments["limit"], "limit", DEFAULT_READ_LIMIT)
            .map_err(AppError::Llm)?;

        let full_path = common::resolve_model_path(&context.working_dir, file_path);

        let content = match read_file_with_cache(&full_path, file_path, "async_read_file", context.cache.as_ref())
            .await
        {
            Ok(c) => c,
            Err(tool_result) => return Ok(tool_result),
        };

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start = (offset - 1).min(total_lines);
        let end = (start + limit).min(total_lines);

        let displayed = lines[start..end].join("\n");
        let displayed_len = displayed.chars().count();

        let info = generate_read_info(file_path, start, end, total_lines, displayed_len);

        Ok(ToolResult {
            success: true,
            security_evaluation: None,
            restart_requested: false,
            error_category: None,
            content: format!("{}\n\n{}", info, displayed),
        })
    }
}

/// 异步批量读取文件工具
pub struct AsyncBatchReadFilesTool;

#[async_trait]
impl AsyncTool for AsyncBatchReadFilesTool {
    fn name(&self) -> &str {
        "async_batch_read_files"
    }

    fn description(&self) -> &str {
        "Asynchronously read multiple files at once. This is much more efficient for tasks like code review where you need to read many files. Supports glob patterns and automatic content truncation."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    },
                    "description": "List of file paths or glob patterns to read (e.g., ['src/main.rs', 'src/utils/*.rs'])"
                },
                "max_chars_per_file": {
                    "type": "integer",
                    "description": "Maximum number of characters to read from each file. Default: 3000",
                    "default": 3000
                },
                "summarize": {
                    "type": "boolean",
                    "description": "Generate a summary for each file (functions, structs, imports). Default: true",
                    "default": true
                }
            },
            "required": ["files"]
        })
    }

    async fn execute(&self, args: ToolArgs, context: ToolContext) -> Result<ToolResult, AppError> {
        let files_json = args.arguments["files"]
            .as_array()
            .ok_or_else(|| AppError::Llm("files must be an array".to_string()))?;

        let file_patterns: Vec<String> = files_json
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();

        if file_patterns.is_empty() {
            return Ok(ToolResult {
                success: false,
                security_evaluation: None,
                restart_requested: false,
                error_category: None,
                content: "[async_batch_read_files] ❌ No files specified".to_string(),
            });
        }

        let max_chars_per_file =
            common::get_lenient_usize(&args.arguments["max_chars_per_file"], "max_chars_per_file", 3000)
                .map_err(AppError::Llm)?;

        let summarize = args.arguments["summarize"].as_bool().unwrap_or(true);

        let resolved_files = resolve_glob_patterns(&file_patterns, &context.working_dir);

        let mut result = String::new();
        let mut success_count = 0;
        let mut fail_count = 0;

        for file_path in resolved_files {
            if file_path.starts_with("[error]") || file_path.starts_with("[not_found]") {
                result.push_str(&format!("\n{}\n", file_path));
                fail_count += 1;
                continue;
            }

            let full_path = context.working_dir.join(&file_path);

            let content = match read_file_with_cache(
                &full_path,
                &file_path,
                "async_batch_read_files",
                context.cache.as_ref(),
            )
            .await
            {
                Ok(c) => c,
                Err(tool_result) => {
                    fail_count += 1;
                    result.push_str(&format!("\n{}\n", tool_result.content));
                    continue;
                }
            };

            success_count += 1;

            let content_len = content.chars().count();
            let truncated = if content_len > max_chars_per_file {
                let chars: Vec<char> = content.chars().take(max_chars_per_file).collect();
                chars.into_iter().collect()
            } else {
                content.clone()
            };
            let was_truncated = content_len > max_chars_per_file;

            if summarize {
                let summary = generate_code_summary(&content, &file_path);
                result.push_str(&summary);
            }

            result.push_str(&format!("\n=== 文件内容: {} ===\n", file_path));
            result.push_str(&truncated);
            if was_truncated {
                result.push_str(&format!(
                    "\n\n[truncated] 文件共 {} 字符，显示前 {} 字符。使用 async_read_file 读取完整内容。\n",
                    content_len, max_chars_per_file
                ));
            }
            result.push('\n');
        }

        let header = format!(
            "[async_batch_read_files] ✅ 读取完成: {}/{} 文件成功\n",
            success_count,
            success_count + fail_count
        );

        Ok(ToolResult {
            success: success_count > 0,
            security_evaluation: None,
            restart_requested: false,
            error_category: None,
            content: format!("{}{}", header, result),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecurityPolicy;
    use crate::tools::async_tool::AsyncToolRegistry;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[tokio::test]
    async fn async_read_file_execution() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let working_dir = manifest_dir.clone();
        let security = Arc::new(SecurityPolicy::new(&working_dir, false));
        let approval_manager = Arc::new(crate::security::approval::ApprovalManager::new());
        let mut registry = AsyncToolRegistry::new(working_dir, security, approval_manager);

        registry.register_tool(Arc::new(AsyncReadFileTool));

        let result = registry
            .execute_approved(
                "async_read_file",
                json!({
                    "file_path": "src/tools/file/async_read.rs"
                }),
            )
            .await;

        assert!(result.is_ok());
        assert!(result.unwrap().success);
    }
}