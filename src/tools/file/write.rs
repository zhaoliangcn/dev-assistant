//! 写入类工具：`write_file`、`edit_file`。

use tracing::debug;

use super::io::{read_file_content, write_file_content};
use crate::tools::{ToolArgs, ToolContext, ToolDefinition, ToolKind, ToolMetadata, ToolNamespace, ToolResult};
use crate::utils::error::AppError;

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
        skip_security: false,
        handler: Box::new(write_file_handler),
    }
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
        skip_security: false,
        handler: Box::new(edit_file_handler),
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

// ToolMetadata 实现

pub struct WriteFileToolMetadata;

impl ToolMetadata for WriteFileToolMetadata {
    fn kind(&self) -> ToolKind {
        ToolKind::Write
    }
    
    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::DevAssistant
    }
    
    fn description_template(&self) -> &str {
        "Write content to a file."
    }
}

pub struct EditFileToolMetadata;

impl ToolMetadata for EditFileToolMetadata {
    fn kind(&self) -> ToolKind {
        ToolKind::Write
    }
    
    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::DevAssistant
    }
    
    fn description_template(&self) -> &str {
        "Edit a file by replacing old content with new content."
    }
}
