//! 搜索类工具：`glob`、`list_directory`、`file_exists`。

use std::fs;

use globset::{Glob, GlobSetBuilder};
use tracing::debug;
use walkdir::WalkDir;

use super::read_shared::SKIP_DIRS;
use crate::tools::{ToolArgs, ToolContext, ToolDefinition, ToolResult};
use crate::utils::error::AppError;

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

    let mut files: Vec<std::path::PathBuf> = Vec::new();
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
