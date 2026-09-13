//! 符号读取工具：`read_symbol`。
//!
//! 从源文件中按符号名定位并提取定义（函数、结构体、枚举、trait、impl 块、常量、类型别名、宏、模块）。
//!
//! 优先使用 tree-sitter 解析（准确识别 impl 块内的方法、struct 字段、enum 变体等），
//! 失败时降级到手写扫描（仅顶层定义）。

use crate::tools::{common, ErrorCategory, ToolArgs, ToolContext, ToolDefinition, ToolResult};
use crate::utils::error::AppError;

// ---------------------------------------------------------------------------
// 工具定义
// ---------------------------------------------------------------------------

pub fn read_symbol_tool() -> ToolDefinition {
    ToolDefinition {
        name: "read_symbol".to_string(),
        description: "Read a specific symbol definition (function, struct, enum, trait, impl method, field, variant, etc.) from a Rust source file. Uses tree-sitter for accurate parsing, including methods inside impl blocks (query as 'TypeName::method_name' or just 'method_name'). When the symbol is not found, lists all available symbols in the file.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Source file path relative to current working directory"
                },
                "symbol": {
                    "type": "string",
                    "description": "Symbol name to look up. For impl methods, use 'TypeName::method_name' for exact match or just 'method_name' for fuzzy match across all impls."
                },
                "kind": {
                    "type": "string",
                    "enum": ["function", "struct", "enum", "trait", "impl", "method", "field", "variant", "const", "type", "macro", "module", "any"],
                    "description": "Symbol type filter (optional, default: any). 'method' matches impl methods, 'field' matches struct fields, 'variant' matches enum variants.",
                    "default": "any"
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Extra context lines before and after the symbol definition (default: 0)",
                    "default": 0
                },
                "include_body": {
                    "type": "boolean",
                    "description": "Whether to include the symbol's full body (function body, struct fields, etc.), default: true",
                    "default": true
                }
            },
            "required": ["file_path", "symbol"]
        }),
        skip_security: false,
        handler: Box::new(read_symbol_handler),
    }
}

// ---------------------------------------------------------------------------
// 符号类型
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Method,
    Field,
    Variant,
    Const,
    Type,
    Macro,
    Module,
}

impl SymbolKind {
    fn all() -> &'static [SymbolKind] {
        &[
            SymbolKind::Function,
            SymbolKind::Method,
            SymbolKind::Struct,
            SymbolKind::Field,
            SymbolKind::Enum,
            SymbolKind::Variant,
            SymbolKind::Trait,
            SymbolKind::Impl,
            SymbolKind::Const,
            SymbolKind::Type,
            SymbolKind::Macro,
            SymbolKind::Module,
        ]
    }

    fn name(&self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Struct => "struct",
            SymbolKind::Field => "field",
            SymbolKind::Enum => "enum",
            SymbolKind::Variant => "variant",
            SymbolKind::Trait => "trait",
            SymbolKind::Impl => "impl",
            SymbolKind::Const => "const",
            SymbolKind::Type => "type",
            SymbolKind::Macro => "macro",
            SymbolKind::Module => "module",
        }
    }
}

/// 一个已识别符号的摘要信息。
#[derive(Debug, Clone)]
struct SymbolInfo {
    kind: SymbolKind,
    name: String,
    /// 完整限定名（如 `ContextManager::inject_historical_summaries`），用于精确匹配。
    qualified_name: String,
    start_line: usize, // 1-indexed
    end_line: usize,   // 1-indexed, inclusive
    line: usize,       // 定义行（1-indexed）
    attrs: Vec<String>,
    doc_comments: Vec<String>,
}

// ---------------------------------------------------------------------------
// tree-sitter 解析器缓存
// ---------------------------------------------------------------------------

/// 创建并初始化 tree-sitter Parser。
/// Parser 不实现 Clone，每次调用创建新实例（开销极小，约微秒级）。
fn create_parser() -> Result<tree_sitter::Parser, AppError> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|e| AppError::Llm(format!("tree-sitter-rust language init failed: {}", e)))?;
    Ok(parser)
}

// ---------------------------------------------------------------------------
// tree-sitter 扫描实现
// ---------------------------------------------------------------------------

/// 用 tree-sitter 扫描文件中的所有符号。
fn scan_symbols_ts(source: &str) -> Result<Vec<SymbolInfo>, AppError> {
    let mut parser = create_parser()?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AppError::Llm("tree-sitter parse failed".to_string()))?;

    let mut symbols = Vec::new();
    walk_node(&tree.root_node(), source, &mut symbols, None);
    Ok(symbols)
}

/// 递归遍历 AST 节点，收集所有符号。
fn walk_node(
    node: &tree_sitter::Node,
    source: &str,
    symbols: &mut Vec<SymbolInfo>,
    impl_target: Option<String>,
) {
    let kind = node.kind();
    let start_line = node.start_position().row + 1; // 1-indexed
    let end_line = node.end_position().row + 1;

    // 收集属性与文档注释（向上查找紧邻的 attribute_item / line_comment 兄弟）
    let (attrs, docs) = collect_attrs_and_docs(node, source);

    match kind {
        "function_item" | "function_signature_item" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = source[name_node.byte_range()].to_string();
                // tree-sitter-rust 中自由函数与 impl/trait 内方法同为 function_item /
                // function_signature_item，需依据所属上下文区分：有 owner 即方法。
                let (kind, qualified) = match &impl_target {
                    Some(t) => (SymbolKind::Method, format!("{}::{}", t, name)),
                    None => (SymbolKind::Function, name.clone()),
                };
                symbols.push(SymbolInfo {
                    kind,
                    name: name.clone(),
                    qualified_name: qualified,
                    start_line,
                    end_line,
                    line: start_line,
                    attrs,
                    doc_comments: docs,
                });
            }
        }
        "struct_item" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = source[name_node.byte_range()].to_string();
                symbols.push(SymbolInfo {
                    kind: SymbolKind::Struct,
                    name: name.clone(),
                    qualified_name: name,
                    start_line,
                    end_line,
                    line: start_line,
                    attrs,
                    doc_comments: docs,
                });
            }
        }
        "field_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = source[name_node.byte_range()].to_string();
                symbols.push(SymbolInfo {
                    kind: SymbolKind::Field,
                    name: name.clone(),
                    qualified_name: name,
                    start_line,
                    end_line,
                    line: start_line,
                    attrs,
                    doc_comments: docs,
                });
            }
        }
        "enum_item" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = source[name_node.byte_range()].to_string();
                symbols.push(SymbolInfo {
                    kind: SymbolKind::Enum,
                    name: name.clone(),
                    qualified_name: name,
                    start_line,
                    end_line,
                    line: start_line,
                    attrs,
                    doc_comments: docs,
                });
            }
        }
        "enum_variant" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = source[name_node.byte_range()].to_string();
                symbols.push(SymbolInfo {
                    kind: SymbolKind::Variant,
                    name: name.clone(),
                    qualified_name: name,
                    start_line,
                    end_line,
                    line: start_line,
                    attrs,
                    doc_comments: docs,
                });
            }
        }
        "trait_item" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = source[name_node.byte_range()].to_string();
                symbols.push(SymbolInfo {
                    kind: SymbolKind::Trait,
                    name: name.clone(),
                    qualified_name: name.clone(),
                    start_line,
                    end_line,
                    line: start_line,
                    attrs,
                    doc_comments: docs,
                });
                // trait 体内成员同样是 function_item，需传递归属以便判定为方法
                if let Some(body) = node.child_by_field_name("body") {
                    for i in 0..body.child_count() {
                        if let Some(child) = body.child(i) {
                            walk_node(&child, source, symbols, Some(name.clone()));
                        }
                    }
                }
                return;
            }
        }
        "impl_item" => {
            // 提取 impl 目标类型名
            let target = extract_impl_target(node, source);
            // 记录 impl 块本身
            symbols.push(SymbolInfo {
                kind: SymbolKind::Impl,
                name: target.clone().unwrap_or_else(|| "impl".to_string()),
                qualified_name: target.clone().unwrap_or_else(|| "impl".to_string()),
                start_line,
                end_line,
                line: start_line,
                attrs,
                doc_comments: docs,
            });
            // 递归子节点，传递 impl 目标
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    walk_node(&child, source, symbols, target.clone());
                }
            }
            return; // 已手动遍历子节点
        }
        "const_item" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = source[name_node.byte_range()].to_string();
                symbols.push(SymbolInfo {
                    kind: SymbolKind::Const,
                    name: name.clone(),
                    qualified_name: name,
                    start_line,
                    end_line,
                    line: start_line,
                    attrs,
                    doc_comments: docs,
                });
            }
        }
        "type_item" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = source[name_node.byte_range()].to_string();
                symbols.push(SymbolInfo {
                    kind: SymbolKind::Type,
                    name: name.clone(),
                    qualified_name: name,
                    start_line,
                    end_line,
                    line: start_line,
                    attrs,
                    doc_comments: docs,
                });
            }
        }
        "macro_definition" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = source[name_node.byte_range()].to_string();
                symbols.push(SymbolInfo {
                    kind: SymbolKind::Macro,
                    name: name.clone(),
                    qualified_name: name,
                    start_line,
                    end_line,
                    line: start_line,
                    attrs,
                    doc_comments: docs,
                });
            }
        }
        "mod_item" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = source[name_node.byte_range()].to_string();
                symbols.push(SymbolInfo {
                    kind: SymbolKind::Module,
                    name: name.clone(),
                    qualified_name: name,
                    start_line,
                    end_line,
                    line: start_line,
                    attrs,
                    doc_comments: docs,
                });
            }
        }
        _ => {}
    }

    // 默认递归遍历子节点（impl_item 已手动处理并 return）
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            walk_node(&child, source, symbols, impl_target.clone());
        }
    }
}

/// 从 impl_item 节点提取目标类型名。
fn extract_impl_target(node: &tree_sitter::Node, source: &str) -> Option<String> {
    // 官方语法中 impl 目标类型字段名为 type（self_type 不存在）
    let self_type = node.child_by_field_name("type")?;
    let text = &source[self_type.byte_range()];
    // 取第一个 token（去掉泛型、trait 限定等）
    let first = text.split_whitespace().next()?.trim();
    // 去掉泛型 <...>
    let first = first.split('<').next()?.trim();
    // 去掉路径限定（如 std::collections::HashMap → HashMap）
    let first = first.rsplit("::").next()?.trim();
    if first.is_empty() {
        None
    } else {
        Some(first.to_string())
    }
}

/// 收集节点上方的属性（attribute_item）和文档注释（line_comment 以 /// 开头）。
fn collect_attrs_and_docs(node: &tree_sitter::Node, source: &str) -> (Vec<String>, Vec<String>) {
    let mut attrs = Vec::new();
    let mut docs = Vec::new();

    let mut sibling = node.prev_named_sibling();
    while let Some(s) = sibling {
        match s.kind() {
            "attribute_item" => {
                attrs.push(source[s.byte_range()].trim().to_string());
                sibling = s.prev_named_sibling();
            }
            "line_comment" => {
                let text = source[s.byte_range()].to_string();
                if text.starts_with("///") {
                    docs.push(text.trim_start_matches("///").trim().to_string());
                    sibling = s.prev_named_sibling();
                } else {
                    break;
                }
            }
            _ => break,
        }
    }

    attrs.reverse();
    docs.reverse();
    (attrs, docs)
}

// ---------------------------------------------------------------------------
// 手写扫描实现（fallback）
// ---------------------------------------------------------------------------

/// 按行扫描文件内容，收集所有顶层符号的位置信息（fallback，不含 impl 内方法）。
fn scan_symbols(lines: &[&str]) -> Vec<SymbolInfo> {
    let mut symbols = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("/*") {
            i += 1;
            continue;
        }

        let (attrs, doc_comments) = collect_attributes(lines, i);

        if let Some((kind, name, body_end)) = try_match_symbol(lines, i) {
            let end_line = body_end.unwrap_or(i);
            symbols.push(SymbolInfo {
                kind,
                name: name.clone(),
                qualified_name: name,
                start_line: attrs_start_line(lines, i) + 1,
                end_line: end_line + 1,
                line: i + 1,
                attrs,
                doc_comments,
            });
            if let Some(e) = body_end {
                i = e + 1;
                continue;
            }
        }

        i += 1;
    }

    symbols
}

fn collect_attributes(lines: &[&str], line_idx: usize) -> (Vec<String>, Vec<String>) {
    let mut attrs = Vec::new();
    let mut docs = Vec::new();
    let mut i = line_idx as isize - 1;
    let mut pending_attr: Option<String> = None;

    while i >= 0 {
        let trimmed = lines[i as usize].trim();

        if trimmed.starts_with("///") {
            docs.push(trimmed.trim_start_matches("///").trim().to_string());
        } else if trimmed.starts_with("//!") {
            break;
        } else if trimmed.starts_with("#[") && !trimmed.starts_with("#![") {
            if pending_attr.is_some() {
                break;
            }
            pending_attr = Some(trimmed.to_string());
            if trimmed.ends_with(']') {
                attrs.push(pending_attr.take().unwrap());
            }
        } else if trimmed.is_empty() {
            break;
        } else {
            if let Some(ref mut attr) = pending_attr {
                attr.push(' ');
                attr.push_str(trimmed);
                if trimmed.ends_with(']') {
                    attrs.push(pending_attr.take().unwrap());
                }
            } else {
                break;
            }
        }

        i -= 1;
    }

    if let Some(attr) = pending_attr {
        if !attr.is_empty() {
            attrs.push(attr);
        }
    }

    attrs.reverse();
    docs.reverse();
    (attrs, docs)
}

fn attrs_start_line(lines: &[&str], line_idx: usize) -> usize {
    let mut i = line_idx as isize - 1;
    while i >= 0 {
        let trimmed = lines[i as usize].trim();
        if trimmed.starts_with("#[") || trimmed.starts_with("///") || trimmed.is_empty() {
            if trimmed.is_empty() {
                break;
            }
            i -= 1;
        } else {
            break;
        }
    }
    (i + 1) as usize
}

fn strip_modifiers(s: &str) -> &str {
    let s = s.trim();
    let s = if s.starts_with("pub(") {
        if let Some(end) = s.find(')') {
            s[end + 1..].trim()
        } else {
            s
        }
    } else if let Some(stripped) = s.strip_prefix("pub ") {
        stripped.trim()
    } else if let Some(stripped) = s.strip_prefix("pub\t") {
        stripped.trim()
    } else {
        s
    };
    let s = s.strip_prefix("async ").unwrap_or(s);
    let s = s.strip_prefix("unsafe ").unwrap_or(s);
    let s = s.strip_prefix("extern ").unwrap_or(s);
    s
}

fn try_match_symbol(lines: &[&str], line_idx: usize) -> Option<(SymbolKind, String, Option<usize>)> {
    let line = lines[line_idx];
    let trimmed = line.trim();
    let core = strip_modifiers(trimmed);

    if let Some(name) = match_keyword(core, "fn ") {
        if !name.starts_with(|c: char| c.is_whitespace() || c == '(') {
            return Some((SymbolKind::Function, extract_name(name), find_body_end(lines, line_idx)));
        }
    }
    if let Some(name) = match_keyword(core, "struct ") {
        let name = extract_name(name);
        if !name.is_empty() && !name.starts_with('{') {
            return Some((SymbolKind::Struct, name, find_body_end(lines, line_idx)));
        }
    }
    if let Some(name) = match_keyword(core, "enum ") {
        let name = extract_name(name);
        if !name.is_empty() && !name.starts_with('{') {
            return Some((SymbolKind::Enum, name, find_body_end(lines, line_idx)));
        }
    }
    if let Some(name) = match_keyword(core, "trait ") {
        let name = extract_name(name);
        if !name.is_empty() && !name.starts_with('{') && !name.starts_with('(') {
            return Some((SymbolKind::Trait, name, find_body_end(lines, line_idx)));
        }
    }
    if let Some(after_impl) = core.strip_prefix("impl ") {
        let rest = after_impl.trim();
        let rest = rest.strip_prefix("unsafe ").unwrap_or(rest);
        let rest = rest.strip_prefix("pub ").unwrap_or(rest);
        if let Some(target) = rest.split(['{', 'w', ':']).next() {
            let target = target.trim();
            if !target.is_empty() && target != " " {
                let name = target.split_whitespace().next().unwrap_or(target).trim().to_string();
                let name = name.split('<').next().unwrap_or(&name).trim().to_string();
                if !name.is_empty() {
                    return Some((SymbolKind::Impl, name, find_body_end(lines, line_idx)));
                }
            }
        }
        return Some((SymbolKind::Impl, "impl".to_string(), find_body_end(lines, line_idx)));
    }
    if let Some(name) = match_keyword(core, "const ") {
        let name = extract_name(name);
        if !name.is_empty() {
            let has_body = trimmed.contains('{');
            let body_end = if has_body { find_body_end(lines, line_idx) } else { None };
            return Some((SymbolKind::Const, name, body_end));
        }
    }
    if let Some(name) = match_keyword(core, "type ") {
        let name = extract_name(name);
        if !name.is_empty() && !name.starts_with('=') {
            let has_body = trimmed.contains('{');
            let body_end = if has_body { find_body_end(lines, line_idx) } else { None };
            return Some((SymbolKind::Type, name, body_end));
        }
    }
    if let Some(name) = core.strip_prefix("macro_rules! ") {
        let name = name.split_whitespace().next().unwrap_or(name).trim().to_string();
        if !name.is_empty() {
            return Some((SymbolKind::Macro, name, find_body_end(lines, line_idx)));
        }
    }
    if let Some(name) = match_keyword(core, "mod ") {
        let name = name.trim();
        if !name.is_empty() {
            let name = name.split([';', '{']).next().unwrap_or(name).trim().to_string();
            if !name.is_empty() {
                let has_body = trimmed.contains('{');
                let body_end = if has_body { find_body_end(lines, line_idx) } else { None };
                return Some((SymbolKind::Module, name, body_end));
            }
        }
    }

    None
}

fn match_keyword<'a>(s: &'a str, keyword: &str) -> Option<&'a str> {
    s.strip_prefix(keyword)
}

fn extract_name(s: &str) -> String {
    let s = s.trim();
    let s = if s.starts_with("pub(") {
        if let Some(end) = s.find(')') {
            s[end + 1..].trim()
        } else {
            s
        }
    } else if let Some(stripped) = s.strip_prefix("pub ") {
        stripped.trim()
    } else if let Some(stripped) = s.strip_prefix("pub\t") {
        stripped.trim()
    } else {
        s
    };
    let s = s.strip_prefix("async ").unwrap_or(s);
    let s = s.strip_prefix("unsafe ").unwrap_or(s);
    let s = s.strip_prefix("extern ").unwrap_or(s);
    s.split(['<', '(', ' ', '\t', ':'])
        .next()
        .unwrap_or(s)
        .trim()
        .to_string()
}

fn find_body_end(lines: &[&str], start_line: usize) -> Option<usize> {
    let mut brace_depth = 0i32;
    let mut in_string = false;
    let mut in_char = false;
    let mut found_open = false;

    for (i, &line) in lines.iter().enumerate().skip(start_line) {
        let bytes = line.as_bytes();
        let mut j = 0;

        while j < bytes.len() {
            let c = bytes[j] as char;

            if in_string {
                if c == '\\' && j + 1 < bytes.len() {
                    j += 2;
                    continue;
                }
                if c == '"' {
                    in_string = false;
                }
                j += 1;
                continue;
            }

            if in_char {
                if c == '\\' && j + 1 < bytes.len() {
                    j += 2;
                    continue;
                }
                if c == '\'' {
                    in_char = false;
                }
                j += 1;
                continue;
            }

            if c == '/' && j + 1 < bytes.len() && bytes[j + 1] as char == '*' {
                j += 2;
                while j < bytes.len() {
                    if bytes[j] as char == '*' && j + 1 < bytes.len() && bytes[j + 1] as char == '/' {
                        j += 2;
                        break;
                    }
                    j += 1;
                }
                continue;
            }

            if c == '/' && j + 1 < bytes.len() && bytes[j + 1] as char == '/' {
                break;
            }

            match c {
                '"' => in_string = true,
                '\'' => {
                    if j + 1 < bytes.len() && bytes[j + 1] as char != '\'' {
                        in_char = true;
                    }
                }
                '{' => {
                    found_open = true;
                    brace_depth += 1;
                }
                '}' => {
                    brace_depth -= 1;
                    if found_open && brace_depth == 0 {
                        return Some(i);
                    }
                }
                ';' => {
                    if !found_open {
                        return None;
                    }
                }
                _ => {}
            }

            j += 1;
        }
    }

    if found_open {
        Some(lines.len() - 1)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// 符号格式化输出
// ---------------------------------------------------------------------------

fn format_symbol(sym: &SymbolInfo, lines: &[&str], context_lines: usize, include_body: bool) -> String {
    let mut out = String::new();

    let total_lines = if include_body && sym.end_line > sym.line {
        sym.end_line - sym.start_line + 1
    } else {
        1
    };
    out.push_str(&format!(
        "[read_symbol] 找到符号: {} ({})\n",
        sym.qualified_name, sym.kind.name()
    ));
    out.push_str(&format!(
        "文件: 第 {}-{} 行（共 {} 行）\n\n",
        sym.start_line, sym.end_line, total_lines
    ));

    for doc in &sym.doc_comments {
        out.push_str(&format!("/// {}\n", doc));
    }

    for attr in &sym.attrs {
        out.push_str(&format!("{}\n", attr));
    }

    if sym.line > 0 && sym.line <= lines.len() {
        let def_line = lines[sym.line - 1];
        out.push_str(def_line);
        out.push('\n');
    }

    if include_body && sym.end_line > sym.line {
        for line in lines.iter().take(sym.end_line.min(lines.len())).skip(sym.line) {
            out.push_str(line);
            out.push('\n');
        }
    }

    if context_lines > 0 && sym.start_line > 1 {
        let ctx_start = (sym.start_line as isize - 1 - context_lines as isize).max(0) as usize;
        let mut ctx_out = String::new();
        for line in lines.iter().take(sym.start_line - 1).skip(ctx_start) {
            ctx_out.push_str(&format!("  {}\n", line));
        }
        if !ctx_out.is_empty() {
            out.insert_str(0, &format!("... 上下文前 {} 行:\n{}", sym.start_line - 1 - ctx_start, ctx_out));
            out.insert(0, '\n');
        }
    }

    if context_lines > 0 && sym.end_line < lines.len() {
        let ctx_end = (sym.end_line + context_lines).min(lines.len());
        out.push_str(&format!("\n... 上下文后 {} 行:\n", ctx_end - sym.end_line));
        for line in lines.iter().take(ctx_end).skip(sym.end_line) {
            out.push_str(&format!("  {}\n", line));
        }
    }

    out.push_str(&format!(
        "\n符号摘要:\n- 类型: {}\n- 行数: {}\n",
        sym.kind.name(),
        sym.end_line - sym.start_line + 1
    ));
    if !sym.attrs.is_empty() {
        out.push_str(&format!("- 属性: {} 个\n", sym.attrs.len()));
    }
    if !sym.doc_comments.is_empty() {
        out.push_str(&format!("- 文档注释: {} 行\n", sym.doc_comments.len()));
    }

    out
}

fn format_available_symbols(symbols: &[SymbolInfo]) -> String {
    let mut out = String::from("文件中的可用符号:\n");

    for kind in SymbolKind::all() {
        let matching: Vec<&SymbolInfo> = symbols.iter().filter(|s| s.kind == *kind).collect();
        if !matching.is_empty() {
            for sym in &matching {
                out.push_str(&format!(
                    "  {:8}  {} (第 {} 行)\n",
                    kind.name(),
                    sym.qualified_name,
                    sym.line
                ));
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

fn read_symbol_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    let file_path = args.arguments["file_path"]
        .as_str()
        .ok_or_else(|| AppError::Llm("file_path is required".to_string()))?;

    let symbol_name = args.arguments["symbol"]
        .as_str()
        .ok_or_else(|| AppError::Llm("symbol is required".to_string()))?;

    let kind_filter = args.arguments["kind"]
        .as_str()
        .unwrap_or("any");

    let context_lines = common::get_lenient_usize(&args.arguments["context_lines"], "context_lines", 0)
        .map_err(AppError::Llm)?;

    let include_body = args.arguments["include_body"]
        .as_bool()
        .unwrap_or(true);

    let full_path = common::resolve_model_path(&context.working_dir, file_path);

    let content = std::fs::read_to_string(&full_path).map_err(|e| {
        AppError::Llm(format!("Failed to read file '{}': {}", full_path.display(), e))
    })?;

    let lines: Vec<&str> = content.lines().collect();

    // 优先使用 tree-sitter 扫描，失败时降级到手写扫描
    let symbols = match scan_symbols_ts(&content) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(error = %e, "tree-sitter 扫描失败，降级到手写扫描");
            scan_symbols(&lines)
        }
    };

    let kind_filter = kind_filter.to_lowercase();
    let matching: Vec<&SymbolInfo> = symbols.iter().filter(|s| {
        if kind_filter != "any" && s.kind.name() != kind_filter {
            return false;
        }
        // 支持三种匹配方式：
        // 1. 精确匹配 qualified_name（如 "ContextManager::method"）
        // 2. 精确匹配 name（如 "method"）
        // 3. 后缀匹配（qualified_name 以 "::symbol" 结尾）
        s.name == symbol_name
            || s.qualified_name == symbol_name
            || s.qualified_name.ends_with(&format!("::{}", symbol_name))
    }).collect();

    if matching.is_empty() {
        let mut result = format!(
            "[read_symbol] ❌ 未找到符号 '{}' 在文件 '{}' 中\n\n",
            symbol_name,
            file_path
        );

        if kind_filter != "any" {
            let all_matching: Vec<&SymbolInfo> = symbols.iter()
                .filter(|s| s.name == symbol_name || s.qualified_name == symbol_name)
                .collect();
            if !all_matching.is_empty() {
                result.push_str(&format!(
                    "提示: 符号 '{}' 存在，但类型为 '{}'，不是 '{}'\n\n",
                    symbol_name,
                    all_matching[0].kind.name(),
                    kind_filter
                ));
            }
        }

        result.push_str(&format_available_symbols(&symbols));

        return Ok(ToolResult::failure(
            result,
            ErrorCategory::Permanent,
        ));
    }

    let result = format_symbol(matching[0], &lines, context_lines, include_body);

    Ok(ToolResult::success(result))
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 手写扫描测试（fallback 路径） ----

    #[test]
    fn scan_simple_function() {
        let content = "fn hello() {\n    println!(\"hello\");\n}\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "hello");
        assert_eq!(symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn scan_struct_with_derive() {
        let content = "#[derive(Debug)]\nstruct Foo {\n    x: i32,\n}\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Foo");
        assert_eq!(symbols[0].kind, SymbolKind::Struct);
        assert_eq!(symbols[0].attrs.len(), 1);
    }

    #[test]
    fn scan_with_doc_comments() {
        let content = "/// This is a doc comment\nfn documented() {}\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].doc_comments.len(), 1);
    }

    #[test]
    fn scan_impl_block() {
        let content = "impl Foo {\n    fn bar() {}\n}\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Foo");
        assert_eq!(symbols[0].kind, SymbolKind::Impl);
    }

    // ---- tree-sitter 扫描测试 ----

    #[test]
    fn ts_scan_function() {
        let content = "fn hello() {\n    println!(\"hello\");\n}\n";
        let symbols = scan_symbols_ts(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "hello" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn ts_scan_impl_method() {
        let content = "impl ContextManager {\n    pub fn inject_historical_summaries(&mut self, kb_root: &std::path::Path) {\n        // body\n    }\n}\n";
        let symbols = scan_symbols_ts(content).unwrap();
        let method = symbols.iter().find(|s| s.name == "inject_historical_summaries");
        assert!(method.is_some(), "impl 内的方法应被识别");
        let m = method.unwrap();
        assert_eq!(m.kind, SymbolKind::Method);
        assert_eq!(m.qualified_name, "ContextManager::inject_historical_summaries");
    }

    #[test]
    fn ts_scan_struct_field() {
        let content = "struct Foo {\n    x: i32,\n    y: String,\n}\n";
        let symbols = scan_symbols_ts(content).unwrap();
        let x = symbols.iter().find(|s| s.name == "x");
        assert!(x.is_some(), "struct 字段应被识别");
        assert_eq!(x.unwrap().kind, SymbolKind::Field);
    }

    #[test]
    fn ts_scan_enum_variant() {
        let content = "enum Color {\n    Red,\n    Green,\n    Blue,\n}\n";
        let symbols = scan_symbols_ts(content).unwrap();
        let red = symbols.iter().find(|s| s.name == "Red");
        assert!(red.is_some(), "enum 变体应被识别");
        assert_eq!(red.unwrap().kind, SymbolKind::Variant);
    }

    #[test]
    fn ts_scan_trait_method() {
        let content = "pub trait Into<T> {\n    fn into(self) -> T;\n}\n";
        let symbols = scan_symbols_ts(content).unwrap();
        let into = symbols.iter().find(|s| s.name == "into");
        assert!(into.is_some(), "trait 方法应被识别");
        assert_eq!(into.unwrap().kind, SymbolKind::Method);
    }

    #[test]
    fn ts_scan_multiple_impls() {
        let content = "impl Foo {\n    fn a() {}\n}\nimpl Bar {\n    fn a() {}\n}\n";
        let symbols = scan_symbols_ts(content).unwrap();
        let a_methods: Vec<_> = symbols.iter().filter(|s| s.name == "a").collect();
        assert_eq!(a_methods.len(), 2, "同名方法在不同 impl 中应都被识别");
        let qualifieds: Vec<_> = a_methods.iter().map(|s| s.qualified_name.clone()).collect();
        assert!(qualifieds.contains(&"Foo::a".to_string()));
        assert!(qualifieds.contains(&"Bar::a".to_string()));
    }

    #[test]
    fn ts_scan_const_and_type() {
        let content = "const MAX: usize = 1024;\ntype Result<T> = std::result::Result<T, Error>;\n";
        let symbols = scan_symbols_ts(content).unwrap();
        assert!(symbols.iter().any(|s| s.name == "MAX" && s.kind == SymbolKind::Const));
        assert!(symbols.iter().any(|s| s.name == "Result" && s.kind == SymbolKind::Type));
    }
}
