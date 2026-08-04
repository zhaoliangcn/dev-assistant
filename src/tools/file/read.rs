//! 读取类工具：`read_file`、`batch_read_files`。

use super::io::read_file_content;
use super::read_shared::{DEFAULT_READ_LIMIT, generate_code_summary, generate_read_info, resolve_glob_patterns};
use crate::tools::{common, ToolArgs, ToolContext, ToolDefinition, ToolResult};
use crate::utils::error::AppError;

pub fn read_file_tool() -> ToolDefinition {
    ToolDefinition {
        name: "read_file".to_string(),
        description: "Read a file from the filesystem. Supports offset/limit for reading large files in chunks.".to_string(),
        parameters: serde_json::json!({
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
        }),
        skip_security: false,
        handler: Box::new(read_file_handler),
    }
}

pub fn batch_read_files_tool() -> ToolDefinition {
    ToolDefinition {
        name: "batch_read_files".to_string(),
        description: "Read multiple files at once. This is much more efficient for tasks like code review where you need to read many files. Supports glob patterns and automatic content truncation.".to_string(),
        parameters: serde_json::json!({
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
        }),
        skip_security: false,
        handler: Box::new(batch_read_files_handler),
    }
}

fn read_file_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    let file_path = args.arguments["file_path"]
        .as_str()
        .ok_or_else(|| AppError::Llm("file_path is required".to_string()))?;

    let offset = common::get_lenient_usize(&args.arguments["offset"], "offset", 1)
        .map_err(AppError::Llm)?
        .max(1);

    let limit = common::get_lenient_usize(&args.arguments["limit"], "limit", DEFAULT_READ_LIMIT)
        .map_err(AppError::Llm)?;

    let full_path = common::resolve_model_path(&context.working_dir, file_path);
    
    // 检查 gitignore
    if let Some(reason) = common::check_gitignore(&full_path, &context.resources) {
        return Ok(ToolResult {
            success: false,
            security_evaluation: None,
            restart_requested: false,
            error_category: None,
            content: format!("[read_file] ❌ Gitignore ignored: {}", reason),
        });
    }
    
    // 检查缓存，避免重复读取同一文件
    let content = if let Some(cached) = context.cache.as_ref().and_then(|c| c.read(&full_path)) {
        cached
    } else {
        let content = match read_file_content(&full_path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ToolResult {
                    success: false,
                    security_evaluation: None,
                    restart_requested: false,
                    error_category: None,
                    content: format!(
                        "[read_file] ❌ File not found: {}\n\
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
                        "[read_file] ❌ Permission denied: {}\n\
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
                        "[read_file] ❌ Binary/non-UTF-8 file: {}\n\
                         This file contains binary or non-text data and cannot be displayed.\n\
                         Use exec_command with `file {}` or `xxd {}` to inspect it.",
                        file_path, file_path, file_path
                    ),
                });
            }
            Err(e) => return Err(AppError::Io(e)),
        };
        // 写入缓存，供后续读取使用
        if let Some(cache) = &context.cache {
            cache.write(&full_path, &content);
        }
        content
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

fn batch_read_files_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
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
            content: "[batch_read_files] ❌ No files specified".to_string(),
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
        
        // 检查 gitignore
        if let Some(reason) = common::check_gitignore(&full_path, &context.resources) {
            fail_count += 1;
            result.push_str(&format!(
                "\n[batch_read_files] ❌ Gitignore ignored: {}: {}\n",
                file_path, reason
            ));
            continue;
        }
        
        // 检查缓存，避免重复读取同一文件
        let content = if let Some(cached) = context.cache.as_ref().and_then(|c| c.read(&full_path)) {
            cached
        } else {
            match read_file_content(&full_path) {
                Ok(c) => {
                    // 写入缓存，供后续读取使用
                    if let Some(cache) = &context.cache {
                        cache.write(&full_path, &c);
                    }
                    c
                }
                Err(e) => {
                    fail_count += 1;
                    result.push_str(&format!(
                        "\n[batch_read_files] ❌ Failed to read {}: {}\n",
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
                "\n\n[truncated] 文件共 {} 字符，显示前 {} 字符。使用 read_file 读取完整内容。\n",
                content_len, max_chars_per_file
            ));
        }
        result.push('\n');
    }

    let header = format!(
        "[batch_read_files] ✅ 读取完成: {}/{} 文件成功\n",
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