//! 文件工具共享常量和函数。

use std::path::Path;

use globset::Glob;
use walkdir::WalkDir;

/// 单次 read_file 默认读取的行数上限。
pub const DEFAULT_READ_LIMIT: usize = 200;

/// 遍历目录时跳过的目录名（构建产物、VCS、依赖等）。
pub const SKIP_DIRS: &[&str] = &[
    "target",
    ".git",
    "node_modules",
    ".cargo",
    "dist",
    "build",
];

/// 生成代码摘要
pub fn generate_code_summary(content: &str, file_path: &str) -> String {
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

/// 解析 glob 模式，返回匹配的文件路径列表
pub fn resolve_glob_patterns(patterns: &[String], working_dir: &Path) -> Vec<String> {
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

/// 生成文件读取信息头
pub fn generate_read_info(file_path: &str, start: usize, end: usize, total_lines: usize, displayed_len: usize) -> String {
    let mut info = format!(
        "[read_file] {} (lines {}-{} of {}, {} chars, {} KB)",
        file_path,
        start + 1,
        end,
        total_lines,
        displayed_len,
        (displayed_len as f64 / 1024.0).round()
    );

    if start > 0 || end < total_lines {
        info.push_str(&format!(
            "\nShowing {}/{} lines. Use offset/limit to read other sections.",
            end - start,
            total_lines
        ));
    }

    info
}
