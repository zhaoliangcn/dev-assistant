use std::fs;
use std::path::PathBuf;

use globset::{Glob, GlobSetBuilder};
use tracing::debug;
use walkdir::WalkDir;

use super::{ToolArgs, ToolContext, ToolDefinition, ToolResult};
use crate::utils::error::AppError;

/// SECURITY: Safe file open functions that use O_NOFOLLOW on Unix systems
/// to prevent symlink-based TOCTOU attacks. This ensures that if the final
/// path component is a symlink, the open will fail rather than following it.
#[cfg(unix)]
fn open_file_read(path: &PathBuf) -> Result<std::fs::File, std::io::Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_NOFOLLOW);
    options.open(path)
}

#[cfg(unix)]
fn open_file_write(path: &PathBuf) -> Result<std::fs::File, std::io::Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    options.custom_flags(libc::O_NOFOLLOW);
    options.open(path)
}

#[cfg(not(unix))]
fn open_file_read(path: &PathBuf) -> Result<std::fs::File, std::io::Error> {
    std::fs::OpenOptions::new().read(true).open(path)
}

#[cfg(not(unix))]
fn open_file_write(path: &PathBuf) -> Result<std::fs::File, std::io::Error> {
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(path)
}

/// Read file content with O_NOFOLLOW on Unix to prevent symlink-based TOCTOU.
fn read_file_content(path: &PathBuf) -> Result<String, std::io::Error> {
    let mut file = open_file_read(path)?;
    let mut content = String::new();
    use std::io::Read;
    file.read_to_string(&mut content)?;
    Ok(content)
}

/// Write content to file with O_NOFOLLOW on Unix to prevent symlink-based TOCTOU.
fn write_file_content(path: &PathBuf, content: &str) -> Result<(), std::io::Error> {
    let mut file = open_file_write(path)?;
    use std::io::Write;
    file.write_all(content.as_bytes())?;
    Ok(())
}

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
        handler: Box::new(batch_read_files_handler),
    }
}

const DEFAULT_READ_LIMIT: usize = 200;

/// Directories to skip during traversal for performance.
const SKIP_DIRS: &[&str] = &["target", ".git", "node_modules", ".cargo", "dist", "build"];

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
    debug!(file = %file_path, path = %full_path.display(), offset, limit, "read_file");
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

pub fn write_file_tool() -> ToolDefinition {
    ToolDefinition {
        name: "write_file".to_string(),
        description: "Write content to a file".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "File path relative to current directory"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write"
                }
            },
            "required": ["file_path", "content"]
        }),
        handler: Box::new(write_file_handler),
    }
}

fn write_file_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    let file_path = args.arguments["file_path"]
        .as_str()
        .ok_or_else(|| AppError::Llm("file_path is required".to_string()))?;
    let content = args.arguments["content"]
        .as_str()
        .ok_or_else(|| AppError::Llm("content is required".to_string()))?;

    let full_path = context.working_dir.join(file_path);
    debug!(file = %file_path, path = %full_path.display(), content_len = content.len(), "write_file");
    match write_file_content(&full_path, content) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ToolResult {
                success: false,
                security_evaluation: None,
                restart_requested: false,
                content: format!("[write_file] ❌ Parent directory not found: {}", file_path),
            });
        }
        Err(e) => return Err(AppError::Io(e)),
    }

    Ok(ToolResult {
        success: true,
        security_evaluation: None,
        restart_requested: false,
        content: format!("[write_file] ✅ {} ({} chars)", file_path, content.len()),
    })
}

pub fn edit_file_tool() -> ToolDefinition {
    ToolDefinition {
        name: "edit_file".to_string(),
        description: "Edit a file by replacing old content with new content".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "File path relative to current directory"
                },
                "old_content": {
                    "type": "string",
                    "description": "Content to replace"
                },
                "new_content": {
                    "type": "string",
                    "description": "New content"
                }
            },
            "required": ["file_path", "old_content", "new_content"]
        }),
        handler: Box::new(edit_file_handler),
    }
}

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace("\r", "\n")
}

/// Try to find `needle` in `haystack` with fuzzy matching:
/// 1. Exact match after newline normalization
/// 2. Trimmed match (ignore leading/trailing whitespace differences)
/// 3. Dedented match (remove common leading whitespace from all lines)
fn fuzzy_find(haystack: &str, needle: &str) -> Option<usize> {
    let haystack = normalize_newlines(haystack);
    let needle = normalize_newlines(needle);

    // Exact match
    if let Some(pos) = haystack.find(&needle) {
        return Some(pos);
    }

    // Fuzzy match: trim both sides
    let needle_trimmed = needle.trim();
    if needle_trimmed.is_empty() {
        return None;
    }
    if let Some(pos) = haystack.find(needle_trimmed) {
        return Some(pos);
    }

    // Try matching with normalized indentation
    if let Some(first_line) = needle_trimmed.lines().next() {
        if let Some(indent_end) = first_line.find(|c: char| c != ' ' && c != '\t') {
            let indent = &first_line[..indent_end];
            let needle_dedented = needle_trimmed
                .lines()
                .map(|line| line.strip_prefix(indent).unwrap_or(line))
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(pos) = haystack.find(&needle_dedented) {
                return Some(pos);
            }
        }
    }

    None
}

fn edit_file_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    let file_path = args.arguments["file_path"]
        .as_str()
        .ok_or_else(|| AppError::Llm("file_path is required".to_string()))?;
    let old_content = args.arguments["old_content"]
        .as_str()
        .ok_or_else(|| AppError::Llm("old_content is required".to_string()))?;
    let new_content = args.arguments["new_content"]
        .as_str()
        .ok_or_else(|| AppError::Llm("new_content is required".to_string()))?;

    let full_path = context.working_dir.join(file_path);
    debug!(file = %file_path, path = %full_path.display(), "edit_file");
    let content = match read_file_content(&full_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ToolResult {
                success: false,
                security_evaluation: None,
                    restart_requested: false,
                content: format!(
                    "[edit_file] ❌ File not found: {}\n\
                     The file does not exist at this path. You may want to:\n\
                     1. Use glob to find the correct file path\n\
                     2. Use write_file instead to create a new file",
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
                    "[edit_file] ❌ Permission denied: {}\n\
                     The file exists but you don't have write access.",
                    file_path
                ),
            });
        }
        Err(e) => return Err(AppError::Io(e)),
    };

    let match_pos = fuzzy_find(&content, old_content);

    if match_pos.is_none() {
        // Return file content with line numbers so LLM can retry with correct content
        let lines: Vec<&str> = content.lines().collect();
        let numbered: String = lines
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:4} | {}", i + 1, line))
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(ToolResult {
            success: false,
            security_evaluation: None,
                    restart_requested: false,
            content: format!(
                "[edit_file] ❌ Old content not found in file: {}\n\
                 The file content (with line numbers) is:\n\
                 {}\n\
                 Please read the file and use the exact content from it.",
                file_path, numbered
            ),
        });
    }

    let pos = match_pos.unwrap();
    let matched = &content[pos..pos + old_content.len()];
    let before = &content[..pos];
    let after = &content[pos + old_content.len()..];

    let result = format!("{}{}{}", before, new_content, after);
    write_file_content(&full_path, &result)?;

    // Check if the old content appears again later (potential multi-replace)
    let has_duplicate = content[pos + old_content.len()..].contains(old_content)
        || content[..pos].contains(old_content);

    Ok(ToolResult {
        success: true,
        security_evaluation: None,
                    restart_requested: false,
        content: format!(
            "[edit_file] ✅ {} (replaced {} chars{}){}",
            file_path,
            matched.len(),
            if matched == old_content {
                ""
            } else {
                " [fuzzy match]"
            },
            if has_duplicate {
                "\n⚠️  Warning: matched content appears multiple times. Only the first occurrence was replaced."
            } else {
                ""
            }
        ),
    })
}

pub fn glob_tool() -> ToolDefinition {
    ToolDefinition {
        name: "glob".to_string(),
        description: "Find files matching a glob pattern".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern (e.g., **/*.rs)"
                }
            },
            "required": ["pattern"]
        }),
        handler: Box::new(glob_handler),
    }
}

fn glob_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    let pattern = args.arguments["pattern"]
        .as_str()
        .ok_or_else(|| AppError::Llm("pattern is required".to_string()))?;

    debug!(pattern, "glob");

    let glob_set = {
        let mut builder = GlobSetBuilder::new();
        builder.add(
            Glob::new(pattern)
                .map_err(|e| AppError::Llm(format!("Invalid glob pattern: {}", e)))?,
        );
        builder.build()?
    };

    // Directories to skip during traversal for performance

    let mut files: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(&context.working_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        // Skip common large directories and hidden directories
        let entry_path = entry.path();
        if entry_path.is_dir() {
            if let Some(name) = entry_path.file_name().and_then(|n| n.to_str()) {
                if SKIP_DIRS.contains(&name) || name.starts_with('.') {
                    continue;
                }
            }
        }

        if glob_set.is_match(entry.path()) {
            if let Ok(relative) = entry.path().strip_prefix(&context.working_dir) {
                files.push(relative.to_path_buf());
            }
        }
    }

    files.sort();

    let file_list: Vec<String> = files
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    let max_display = 50;
    let truncated = if file_list.len() > max_display {
        let display_list: Vec<String> = file_list.iter().take(max_display).cloned().collect();
        format!(
            "[glob] Found {} files (showing first {}):\n{}",
            file_list.len(),
            max_display,
            display_list.join("\n")
        )
    } else {
        format!(
            "[glob] Found {} files:\n{}",
            file_list.len(),
            file_list.join("\n")
        )
    };

    Ok(ToolResult {
        success: true,
        security_evaluation: None,
                    restart_requested: false,
        content: truncated,
    })
}

pub fn list_directory_tool() -> ToolDefinition {
    ToolDefinition {
        name: "list_directory".to_string(),
        description: "List files and directories in a directory".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "dir_path": {
                    "type": "string",
                    "description": "Directory path relative to current directory, default is current directory"
                }
            }
        }),
        handler: Box::new(list_directory_handler),
    }
}

fn list_directory_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    let dir_path = args.arguments["dir_path"].as_str().unwrap_or(".");

    let full_path = context.working_dir.join(dir_path);
    debug!(dir = %dir_path, path = %full_path.display(), "list_directory");
    let mut entries: Vec<String> = Vec::new();

    let read_dir = match fs::read_dir(&full_path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ToolResult {
                success: false,
                security_evaluation: None,
                    restart_requested: false,
                content: format!("[list_directory] ❌ Directory not found: {}", dir_path),
            });
        }
        Err(e) => return Err(AppError::Io(e)),
    };

    for entry in read_dir {
        let entry = entry?;
        let path = entry.path();
        let file_name = path.file_name().unwrap().to_string_lossy().to_string();

        if path.is_dir() {
            entries.push(format!("📁 {}", file_name));
        } else {
            entries.push(format!("📄 {}", file_name));
        }
    }

    entries.sort();

    Ok(ToolResult {
        success: true,
        security_evaluation: None,
                    restart_requested: false,
        content: format!("[list_directory] {}:\n{}", dir_path, entries.join("\n")),
    })
}

pub fn file_exists_tool() -> ToolDefinition {
    ToolDefinition {
        name: "file_exists".to_string(),
        description: "Check if a file or directory exists".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "File path relative to current directory"
                }
            },
            "required": ["file_path"]
        }),
        handler: Box::new(file_exists_handler),
    }
}

fn file_exists_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    let file_path = args.arguments["file_path"]
        .as_str()
        .ok_or_else(|| AppError::Llm("file_path is required".to_string()))?;

    let full_path = context.working_dir.join(file_path);
    debug!(file = %file_path, path = %full_path.display(), "file_exists");
    let exists = full_path.exists();
    let is_dir = full_path.is_dir();

    Ok(ToolResult {
        success: true,
        security_evaluation: None,
                    restart_requested: false,
        content: format!(
            "[file_exists] {}: {} (is_dir: {})",
            file_path, exists, is_dir
        ),
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

fn resolve_glob_patterns(patterns: &[String], working_dir: &PathBuf) -> Vec<String> {
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

            let glob_set = match GlobSetBuilder::new().add(glob).build() {
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

    let summarize = args.arguments["summarize"]
        .as_bool()
        .unwrap_or(true);

    let resolved_files = resolve_glob_patterns(&file_patterns, &context.working_dir);
    debug!(count = resolved_files.len(), "batch_read_files");

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
