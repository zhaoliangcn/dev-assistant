//! 异步读取类工具：`async_read_file`、`async_batch_read_files`。

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use super::async_io::read_file_content;
use super::read_shared::{DEFAULT_READ_LIMIT, generate_code_summary, resolve_glob_patterns};
use crate::tools::async_tool::{AsyncTool, ToolArgs, ToolContext, ToolResult};
use crate::tools::common;
use crate::utils::error::AppError;

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
                    "description": "Maximum number of lines to read. Default: 200"
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
        
        // 检查缓存（异步）
        let content = if let Some(cache) = &context.cache {
            if let Some(cached) = cache.read_async(&full_path).await {
                debug!("Cache hit for {}", file_path);
                cached
            } else {
                let content = match read_file_content(&full_path).await {
                    Ok(c) => c,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(ToolResult {
                            success: false,
                            security_evaluation: None,
                            restart_requested: false,
                error_category: None,
                            content: format!(
                                "[async_read_file] ❌ File not found: {}\n\
                                 Please check the file path. You may need to use glob to find the correct file name.",
                                file_path
                            ),
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        return Ok(ToolResult {
                            success: false,
                            security_evaluation: None,
                            restart_requested: false,
                error_category: None,
                            content: format!(
                                "[async_read_file] ❌ Permission denied: {}\n\
                                 The file exists but you don't have read access.",
                                file_path
                            ),
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                        return Ok(ToolResult {
                            success: false,
                            security_evaluation: None,
                            restart_requested: false,
                error_category: None,
                            content: format!(
                                "[async_read_file] ❌ Binary/non-UTF-8 file: {}\n\
                                 This file contains binary or non-text data and cannot be displayed.\n\
                                 Use exec_command with `file {}` or `xxd {}` to inspect it.",
                                file_path, file_path, file_path
                            ),
                        });
                    }
                    Err(e) => return Err(AppError::Io(e)),
                };
                cache.write_async(&full_path, &content).await;
                content
            }
        } else {
            match read_file_content(&full_path).await {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(ToolResult {
                        success: false,
                        security_evaluation: None,
                        restart_requested: false,
                error_category: None,
                        content: format!(
                            "[async_read_file] ❌ File not found: {}\n\
                             Please check the file path. You may need to use glob to find the correct file name.",
                            file_path
                        ),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    return Ok(ToolResult {
                        success: false,
                        security_evaluation: None,
                        restart_requested: false,
                error_category: None,
                        content: format!(
                            "[async_read_file] ❌ Permission denied: {}\n\
                             The file exists but you don't have read access.",
                            file_path
                        ),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                    return Ok(ToolResult {
                        success: false,
                        security_evaluation: None,
                        restart_requested: false,
                error_category: None,
                        content: format!(
                            "[async_read_file] ❌ Binary/non-UTF-8 file: {}\n\
                             This file contains binary or non-text data and cannot be displayed.\n\
                             Use exec_command with `file {}` or `xxd {}` to inspect it.",
                            file_path, file_path, file_path
                        ),
                    });
                }
                Err(e) => return Err(AppError::Io(e)),
            }
        };

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start = (offset - 1).min(total_lines);
        let end = (start + limit).min(total_lines);

        let displayed = lines[start..end].join("\n");
        let displayed_len = displayed.chars().count();

        let mut info = format!(
            "[async_read_file] {} (lines {}-{} of {}, {} chars, {} KB)",
            file_path,
            start + 1,
            end,
            total_lines,
            displayed_len,
            (displayed_len as f64 / 1024.0).round()
        );

        if offset > 1 || end < total_lines {
            info.push_str(&format!(
                "\nShowing {}/{} lines. Use offset/limit to read other sections.",
                end - start,
                total_lines
            ));
        }

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

        let max_chars_per_file = common::get_lenient_usize(&args.arguments["max_chars_per_file"], "max_chars_per_file", 3000)
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
            
            let content = if let Some(cache) = &context.cache {
                if let Some(cached) = cache.read_async(&full_path).await {
                    cached
                } else {
                    let content = match read_file_content(&full_path).await {
                        Ok(c) => c,
                        Err(e) => {
                            fail_count += 1;
                            result.push_str(&format!(
                                "\n[async_batch_read_files] ❌ Failed to read {}: {}\n",
                                file_path, e
                            ));
                            continue;
                        }
                    };
                    cache.write_async(&full_path, &content).await;
                    content
                }
            } else {
                match read_file_content(&full_path).await {
                    Ok(c) => c,
                    Err(e) => {
                        fail_count += 1;
                        result.push_str(&format!(
                            "\n[async_batch_read_files] ❌ Failed to read {}: {}\n",
                            file_path, e
                        ));
                        continue;
                    }
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
        let working_dir = PathBuf::from("/Users/macmima1234/code/dev-assistant-rs");
        let security = Arc::new(SecurityPolicy::new(&working_dir, false));
        let approval_manager = Arc::new(crate::security::approval::ApprovalManager::new());
        let mut registry = AsyncToolRegistry::new(working_dir, security, approval_manager);
        
        registry.register_tool(Arc::new(AsyncReadFileTool));
        
        let result = registry.execute_approved("async_read_file", json!({
            "file_path": "src/tools/file/async_read.rs"
        })).await;
        
        assert!(result.is_ok());
        assert!(result.unwrap().success);
    }
}
