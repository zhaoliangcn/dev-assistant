//! 读取类工具：`read_file`、`batch_read_files`。

use std::path::Path;

use globset::Glob;
use walkdir::WalkDir;

use super::io::read_file_content;
use super::read_shared::{DEFAULT_READ_LIMIT, SKIP_DIRS};
use crate::tools::{ToolArgs, ToolContext, ToolDefinition, ToolResult};
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
                    "description": "Maximum number of lines to read. Default: 200"
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

    let offset = args.arguments["offset"].as_u64().unwrap_or(1).max(1) as usize;

    let limit = args.arguments["limit"]
        .as_u64()
        .map(|v| v as usize)
        .unwrap_or(DEFAULT_READ_LIMIT);

    let full_path = context.working_dir.join(file_path);
    let content = match read_file_content(&full_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ToolResult {
                success: false,
                security_evaluation: None,
                restart_requested: false,
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

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    let start = (offset - 1).min(total_lines);
    let end = (start + limit).min(total_lines);

    let displayed = lines[start..end].join("\n");
    let displayed_len = displayed.chars().count();

    let mut info = format!(
        "[read_file] {} (lines {}-{} of {}, {} chars, {} KB)",
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
            content: "[batch_read_files] ❌ No files specified".to_string(),
        });
    }

    let max_chars_per_file = args.arguments["max_chars_per_file"]
        .as_u64()
        .map(|v| v as usize)
        .unwrap_or(3000);

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
        match read_file_content(&full_path) {
            Ok(content) => {
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
            Err(e) => {
                fail_count += 1;
                result.push_str(&format!(
                    "\n[batch_read_files] ❌ Failed to read {}: {}\n",
                    file_path, e
                ));
            }
        }
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
        content: format!("{}{}", header, result),
    })
}

fn generate_code_summary(content: &str, file_path: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    let mut functions = Vec::new();
    let mut structs = Vec::new();
    let mut imports = Vec::new();
    let mut comments = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("pub fn ") || line.starts_with("fn ") {
            let func_name = line.split_whitespace().nth(1).unwrap_or("");
            functions.push(format!("  - {} (line {})", func_name, i + 1));
        } else if line.starts_with("pub struct ") || line.starts_with("struct ") {
            let struct_name = line.split_whitespace().nth(1).unwrap_or("");
            structs.push(format!("  - {} (line {})", struct_name, i + 1));
        } else if line.starts_with("use ") {
            imports.push(line.trim().to_string());
        } else if (line.starts_with("//") || line.starts_with("/*")) && i < 10 {
            comments.push(line.trim().to_string());
        }
    }

    let mut summary = format!("\n=== 文件摘要: {} ===\n", file_path);
    summary.push_str(&format!("总行数: {}\n", total_lines));

    if !imports.is_empty() {
        summary.push_str("\n主要导入:\n");
        for imp in imports.iter().take(5) {
            summary.push_str(&format!("  {}\n", imp));
        }
        if imports.len() > 5 {
            summary.push_str(&format!("  ... 还有 {} 个导入\n", imports.len() - 5));
        }
    }

    if !structs.is_empty() {
        summary.push_str("\n结构体:\n");
        for s in &structs {
            summary.push_str(&format!("{}\n", s));
        }
    }

    if !functions.is_empty() {
        summary.push_str("\n函数:\n");
        for f in &functions {
            summary.push_str(&format!("{}\n", f));
        }
    }

    if !comments.is_empty() {
        summary.push_str("\n头部注释:\n");
        for c in &comments {
            summary.push_str(&format!("{}\n", c));
        }
    }

    summary.push_str("=== 摘要结束 ===\n");
    summary
}

fn resolve_glob_patterns(patterns: &[String], working_dir: &Path) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();

    for pattern in patterns {
        if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
            let glob = match Glob::new(pattern) {
                Ok(glob) => glob,
                Err(e) => {
                    files.push(format!("[error] Invalid glob pattern '{}': {}", pattern, e));
                    continue;
                }
            };

            let glob_set = match globset::GlobSetBuilder::new().add(glob).build() {
                Ok(gs) => gs,
                Err(e) => {
                    files.push(format!("[error] Failed to build glob '{}': {}", pattern, e));
                    continue;
                }
            };

            for entry in WalkDir::new(working_dir)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    if let Some(name) = entry_path.file_name().and_then(|n| n.to_str()) {
                        if SKIP_DIRS.contains(&name) || name.starts_with('.') {
                            continue;
                        }
                    }
                }

                if glob_set.is_match(entry_path) {
                    if let Ok(relative) = entry_path.strip_prefix(working_dir) {
                        files.push(relative.to_string_lossy().to_string());
                    }
                }
            }
        } else {
            let full_path = working_dir.join(pattern);
            if full_path.exists() {
                files.push(pattern.clone());
            } else {
                files.push(format!("[not_found] {}", pattern));
            }
        }
    }

    files.sort();
    files.dedup();
    files
}
