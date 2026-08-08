//! 符号读取工具：`read_symbol`。
//!
//! 从源文件中按符号名定位并提取定义（函数、结构体、枚举、trait、impl 块、常量、类型别名、宏、模块）。
//! 使用括号匹配算法（不依赖 `syn`），支持属性/文档注释收集。

use crate::tools::{common, ToolArgs, ToolContext, ToolDefinition, ToolResult, ErrorCategory};
use crate::utils::error::AppError;

// ---------------------------------------------------------------------------
// 工具定义
// ---------------------------------------------------------------------------

pub fn read_symbol_tool() -> ToolDefinition {
    ToolDefinition {
        name: "read_symbol".to_string(),
        description: "Read a specific symbol definition (function, struct, enum, etc.) from a source file. Supports bracket matching, attribute/doc collection, and symbol type filtering. When the symbol is not found, lists all available symbols in the file.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Source file path relative to current working directory"
                },
                "symbol": {
                    "type": "string",
                    "description": "Symbol name to look up (e.g., function name, struct name)"
                },
                "kind": {
                    "type": "string",
                    "enum": ["function", "struct", "enum", "trait", "impl", "const", "type", "macro", "module", "any"],
                    "description": "Symbol type filter (optional, default: any)",
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

#[derive(Debug, Clone, Copy, PartialEq)]
enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Const,
    Type,
    Macro,
    Module,
}

impl SymbolKind {
    fn all() -> &'static [SymbolKind] {
        &[
            SymbolKind::Function,
            SymbolKind::Struct,
            SymbolKind::Enum,
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
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
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
#[derive(Debug)]
struct SymbolInfo {
    kind: SymbolKind,
    name: String,
    start_line: usize,   // 1-indexed
    end_line: usize,     // 1-indexed, inclusive
    line: usize,         // 定义行（1-indexed）
    attrs: Vec<String>,  // 属性（#[...]）
    doc_comments: Vec<String>, // 文档注释（///）
}

// ---------------------------------------------------------------------------
// 符号解析器
// ---------------------------------------------------------------------------

/// 按行扫描文件内容，收集所有符号的位置信息。
fn scan_symbols(lines: &[&str]) -> Vec<SymbolInfo> {
    let mut symbols = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // 跳过空行和注释
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("/*") {
            i += 1;
            continue;
        }

        // 收集该行之前的属性/文档注释
        let (attrs, doc_comments) = collect_attributes(lines, i);

        // 尝试匹配符号
        if let Some((kind, name, body_end)) = try_match_symbol(lines, i) {
            let end_line = body_end.unwrap_or(i);
            symbols.push(SymbolInfo {
                kind,
                name,
                start_line: attrs_start_line(lines, i) + 1, // 1-indexed
                end_line: end_line + 1,                      // 1-indexed
                line: i + 1,
                attrs,
                doc_comments,
            });
            // 跳过主体
            if let Some(e) = body_end {
                i = e + 1;
                continue;
            }
        }

        i += 1;
    }

    symbols
}

/// 从给定行向上回溯，收集属性（#[...]）和文档注释（///）。
fn collect_attributes(lines: &[&str], line_idx: usize) -> (Vec<String>, Vec<String>) {
    let mut attrs = Vec::new();
    let mut docs = Vec::new();
    let mut i = line_idx as isize - 1;

    // 收集多行属性（如 #[cfg(...)] 跨行）
    let mut pending_attr: Option<String> = None;

    while i >= 0 {
        let trimmed = lines[i as usize].trim();

        if trimmed.starts_with("///") {
            docs.push(trimmed.trim_start_matches("///").trim().to_string());
        } else if trimmed.starts_with("//!") {
            // 模块文档，不属于符号，停止
            break;
        } else if trimmed.starts_with("#[") && !trimmed.starts_with("#![") {
            // 属性（排除 #![...] 属性）
            if pending_attr.is_some() {
                break; // 前一行有未闭合的属性，但当前行也是属性，停止
            }
            pending_attr = Some(trimmed.to_string());
            // 检查是否需要跨行收集（以 ] 结尾？）
            if trimmed.ends_with(']') {
                attrs.push(pending_attr.take().unwrap());
            }
        } else if trimmed.is_empty() {
            break;
        } else {
            // 检查是否是上一行属性的延续（缩进的内容）
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

    // 如果 pending_attr 还有未闭合的，保留它
    if let Some(attr) = pending_attr {
        if !attr.is_empty() {
            attrs.push(attr);
        }
    }

    attrs.reverse();
    docs.reverse();
    (attrs, docs)
}

/// 计算属性开始的行号（0-indexed）。
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

/// 尝试在指定行匹配一个符号定义。
/// 返回 (符号类型, 符号名, 主体结束行号（0-indexed, 不含 body 则为 None）)
fn try_match_symbol(lines: &[&str], line_idx: usize) -> Option<(SymbolKind, String, Option<usize>)> {
    let line = lines[line_idx];
    let trimmed = line.trim();

    // 按优先级从高到低匹配

    // 1. 函数: fn <name>(
    if let Some(name) = match_keyword(trimmed, "fn ") {
        if !name.starts_with(|c: char| c.is_whitespace() || c == '(') {
            return Some((SymbolKind::Function, extract_name(name), find_body_end(lines, line_idx)));
        }
    }

    // 2. 结构体: struct <name>
    if let Some(name) = match_keyword(trimmed, "struct ") {
        let name = extract_name(name);
        if !name.is_empty() && !name.starts_with('{') {
            return Some((SymbolKind::Struct, name, find_body_end(lines, line_idx)));
        }
    }

    // 3. 枚举: enum <name>
    if let Some(name) = match_keyword(trimmed, "enum ") {
        let name = extract_name(name);
        if !name.is_empty() && !name.starts_with('{') {
            return Some((SymbolKind::Enum, name, find_body_end(lines, line_idx)));
        }
    }

    // 4. trait: trait <name>
    if let Some(name) = match_keyword(trimmed, "trait ") {
        let name = extract_name(name);
        if !name.is_empty() && !name.starts_with('{') && !name.starts_with('(') {
            return Some((SymbolKind::Trait, name, find_body_end(lines, line_idx)));
        }
    }

    // 5. impl <target> (impl 块)
    if let Some(after_impl) = trimmed.strip_prefix("impl ") {
        let rest = after_impl.trim();
        // 跳过 unsafe impl, pub impl 等修饰
        let rest = rest.strip_prefix("unsafe ").unwrap_or(rest);
        let rest = rest.strip_prefix("pub ").unwrap_or(rest);
        // 提取目标类型名（直到 { 或 where 或 :）
        if let Some(target) = rest.split(['{', 'w', ':']).next() {
            let target = target.trim();
            if !target.is_empty() && target != " " {
                // 过滤掉裸 impl (impl 块)
                // 查找目标类型名
                let name = target.split_whitespace().next().unwrap_or(target).trim().to_string();
                // 去掉泛型部分
                let name = name.split('<').next().unwrap_or(&name).trim().to_string();
                if !name.is_empty() {
                    return Some((SymbolKind::Impl, name, find_body_end(lines, line_idx)));
                }
            }
        }
        // 裸 impl 块，使用 impl 本身作为名字
        return Some((SymbolKind::Impl, "impl".to_string(), find_body_end(lines, line_idx)));
    }

    // 6. 常量: const <name>:
    if let Some(name) = match_keyword(trimmed, "const ") {
        let name = extract_name(name);
        if !name.is_empty() {
            // 常量可能没有 body（const X: i32 = 5;）
            let has_body = trimmed.contains('{');
            let body_end = if has_body { find_body_end(lines, line_idx) } else { None };
            return Some((SymbolKind::Const, name, body_end));
        }
    }

    // 7. 类型别名: type <name> =
    if let Some(name) = match_keyword(trimmed, "type ") {
        let name = extract_name(name);
        if !name.is_empty() && !name.starts_with('=') {
            let has_body = trimmed.contains('{');
            let body_end = if has_body { find_body_end(lines, line_idx) } else { None };
            return Some((SymbolKind::Type, name, body_end));
        }
    }

    // 8. 宏: macro_rules! <name>
    if let Some(name) = trimmed.strip_prefix("macro_rules! ") {
        let name = name.split_whitespace().next().unwrap_or(name).trim().to_string();
        if !name.is_empty() {
            return Some((SymbolKind::Macro, name, find_body_end(lines, line_idx)));
        }
    }

    // 9. 模块: mod <name>; 或 mod <name> {
    if let Some(name) = match_keyword(trimmed, "mod ") {
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

/// 检查字符串是否以指定关键字开头（注意空格），并返回关键字后的内容。
fn match_keyword<'a>(s: &'a str, keyword: &str) -> Option<&'a str> {
    s.strip_prefix(keyword)
}

/// 从定义行提取符号名（跳过 pub、pub(crate)、pub(super) 等修饰，以及泛型参数）。
fn extract_name(s: &str) -> String {
    let s = s.trim();
    // 跳过可见性修饰符
    let s = if s.starts_with("pub(") {
        // pub(crate), pub(super), pub(in path)
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

    // 跳过 async, unsafe, extern 等修饰
    let s = s.strip_prefix("async ").unwrap_or(s);
    let s = s.strip_prefix("unsafe ").unwrap_or(s);
    let s = s.strip_prefix("extern ").unwrap_or(s);

    // 取第一个非空 token（直到泛型 < 或括号 (）
    let name = s.split(['<', '(', ' ', '\t'])
        .next()
        .unwrap_or(s)
        .trim()
        .to_string();

    name
}

/// 寻找符号主体的结束行（括号匹配）。
/// 如果符号没有主体（后面跟 ; 或没有 {），返回 None。
/// 返回 0-indexed 行号。
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

            // 字符串字面量跳过
            if in_string {
                if c == '\\' && j + 1 < bytes.len() {
                    j += 2; // 跳过转义
                    continue;
                }
                if c == '"' {
                    in_string = false;
                }
                j += 1;
                continue;
            }

            // 字符字面量跳过
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

            // 块注释跳过
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

            // 行注释跳过（整行，外层循环会跳到下一行）
            if c == '/' && j + 1 < bytes.len() && bytes[j + 1] as char == '/' {
                break;
            }

            match c {
                '"' => in_string = true,
                '\'' => {
                    // 检查是否确实是字符字面量（不是生命周期语法）
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
                        // 没有 body（如 const X: i32 = 5;）
                        return None;
                    }
                }
                _ => {}
            }

            j += 1;
        }

        // 行注释情况下，直接跳到下一行
    }

    // 没有找到闭合括号
    if found_open {
        Some(lines.len() - 1) // 返回文件末尾
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// 符号格式化输出
// ---------------------------------------------------------------------------

/// 格式化单个符号的完整输出。
fn format_symbol(sym: &SymbolInfo, lines: &[&str], context_lines: usize, include_body: bool) -> String {
    let mut out = String::new();

    // 标题行
    let total_lines = if include_body && sym.end_line > sym.line {
        sym.end_line - sym.start_line + 1
    } else {
        1
    };
    out.push_str(&format!(
        "[read_symbol] 找到符号: {} ({})\n",
        sym.name, sym.kind.name()
    ));
    out.push_str(&format!(
        "文件: 第 {}-{} 行（共 {} 行）\n\n",
        sym.start_line, sym.end_line, total_lines
    ));

    // 文档注释
    for doc in &sym.doc_comments {
        out.push_str(&format!("/// {}\n", doc));
    }

    // 属性
    for attr in &sym.attrs {
        out.push_str(&format!("{}\n", attr));
    }

    // 符号定义行
    if sym.line > 0 && sym.line <= lines.len() {
        let def_line = lines[sym.line - 1];
        out.push_str(def_line);
        out.push('\n');
    }

    // 主体
    if include_body && sym.end_line > sym.line {
        for line in lines.iter().take(sym.end_line.min(lines.len())).skip(sym.line) {
            out.push_str(line);
            out.push('\n');
        }
    }

    // 上下文行（前）
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

    // 上下文行（后）
    if context_lines > 0 && sym.end_line < lines.len() {
        let ctx_end = (sym.end_line + context_lines).min(lines.len());
        out.push_str(&format!("\n... 上下文后 {} 行:\n", ctx_end - sym.end_line));
        for line in lines.iter().take(ctx_end).skip(sym.end_line) {
            out.push_str(&format!("  {}\n", line));
        }
    }

    // 符号摘要
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

/// 格式化可用符号列表。
fn format_available_symbols(symbols: &[SymbolInfo]) -> String {
    let mut out = String::from("文件中的可用符号:\n");

    for kind in SymbolKind::all() {
        let matching: Vec<&SymbolInfo> = symbols.iter().filter(|s| s.kind == *kind).collect();
        if !matching.is_empty() {
            for sym in &matching {
                out.push_str(&format!(
                    "  {:8}  {} (第 {} 行)\n",
                    kind.name(),
                    sym.name,
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

    // 读取文件
    let content = std::fs::read_to_string(&full_path).map_err(|e| {
        AppError::Llm(format!("Failed to read file '{}': {}", full_path.display(), e))
    })?;

    let lines: Vec<&str> = content.lines().collect();

    // 扫描所有符号
    let symbols = scan_symbols(&lines);

    // 过滤匹配的符号
    let kind_filter = kind_filter.to_lowercase();
    let matching: Vec<&SymbolInfo> = symbols.iter().filter(|s| {
        // 类型过滤
        if kind_filter != "any" && s.kind.name() != kind_filter {
            return false;
        }
        // 符号名匹配（区分大小写）
        if s.name != symbol_name {
            return false;
        }
        true
    }).collect();

    if matching.is_empty() {
        // 未找到时列出可用符号
        let mut result = format!(
            "[read_symbol] ❌ 未找到符号 '{}' 在文件 '{}' 中\n\n",
            symbol_name,
            file_path
        );

        // 如果指定了类型过滤但没找到，尝试去掉类型过滤
        if kind_filter != "any" {
            let all_matching: Vec<&SymbolInfo> = symbols.iter()
                .filter(|s| s.name == symbol_name)
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

    // 找到符号，格式化输出
    let result = format_symbol(matching[0], &lines, context_lines, include_body);

    Ok(ToolResult::success(result))
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 符号扫描 ----

    #[test]
    fn scan_simple_function() {
        let content = "fn hello() {\n    println!(\"hello\");\n}\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "hello");
        assert_eq!(symbols[0].kind, SymbolKind::Function);
        assert_eq!(symbols[0].line, 1);
        assert_eq!(symbols[0].end_line, 3);
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
        assert!(symbols[0].attrs[0].contains("derive(Debug)"));
    }

    #[test]
    fn scan_with_doc_comments() {
        let content = "/// This is a doc comment\nfn documented() {}\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "documented");
        assert_eq!(symbols[0].doc_comments.len(), 1);
        assert_eq!(symbols[0].doc_comments[0], "This is a doc comment");
    }

    #[test]
    fn scan_nested_brackets() {
        let content = "fn outer() {\n    fn inner() {\n        // nothing\n    }\n    let x = vec![1, 2, 3];\n}\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        // 应该只找到 outer（内嵌 fn 不是顶层定义）
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "outer");
        assert_eq!(symbols[0].end_line, 6);
    }

    #[test]
    fn scan_enum() {
        let content = "enum Color {\n    Red,\n    Green,\n    Blue,\n}\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Color");
        assert_eq!(symbols[0].kind, SymbolKind::Enum);
    }

    #[test]
    fn scan_trait() {
        let content = "pub trait Into<T> {\n    fn into(self) -> T;\n}\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Into");
        assert_eq!(symbols[0].kind, SymbolKind::Trait);
    }

    #[test]
    fn scan_impl_block() {
        let content = "impl Foo {\n    fn bar() {}\n}\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Foo");
        assert_eq!(symbols[0].kind, SymbolKind::Impl);
        assert_eq!(symbols[0].end_line, 3);
    }

    #[test]
    fn scan_const() {
        let content = "const MAX: usize = 1024;\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "MAX");
        assert_eq!(symbols[0].kind, SymbolKind::Const);
    }

    #[test]
    fn scan_type_alias() {
        let content = "type Result<T> = std::result::Result<T, Error>;\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Result");
        assert_eq!(symbols[0].kind, SymbolKind::Type);
    }

    #[test]
    fn scan_macro() {
        let content = "macro_rules! vec {\n    ($($x:expr),*) => { ... };\n}\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "vec");
        assert_eq!(symbols[0].kind, SymbolKind::Macro);
    }

    #[test]
    fn scan_module() {
        let content = "pub mod foo;\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "foo");
        assert_eq!(symbols[0].kind, SymbolKind::Module);
    }

    #[test]
    fn scan_generic_function() {
        let content = "fn foo<T: Debug>(x: T) -> T { x }\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "foo");
        assert_eq!(symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn symbol_not_found_lists_available() {
        let content = "fn hello() {}\nstruct World {}\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        // 验证找到了两个符号
        assert_eq!(symbols.len(), 2);
        // 验证格式
        let list = format_available_symbols(&symbols);
        assert!(list.contains("hello"));
        assert!(list.contains("World"));
        assert!(list.contains("function"));
        assert!(list.contains("struct"));
    }

    #[test]
    fn brackets_in_string_are_skipped() {
        let content = "fn test() {\n    let s = \"hello { world }\";\n    let x = 1;\n}\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        // 括号匹配应该跳过字符串中的 {
        assert_eq!(symbols[0].name, "test");
        assert_eq!(symbols[0].end_line, 4);
    }

    #[test]
    fn pub_struct_with_visibility() {
        let content = "pub struct FooBar { x: i32 }\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "FooBar");
        assert_eq!(symbols[0].kind, SymbolKind::Struct);
    }

    #[test]
    fn async_function() {
        let content = "pub async fn fetch_data(url: &str) -> Result<String> {\n    Ok(String::new())\n}\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "fetch_data");
    }

    #[test]
    fn multi_line_attr() {
        let content = "#[cfg(feature = \"foo\")]\n#[derive(Debug)]\nstruct MultiAttr {\n    x: i32,\n}\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].attrs.len(), 2);
    }

    #[test]
    fn pub_crate_struct() {
        let content = "pub(crate) struct Internal { x: i32 }\n";
        let lines: Vec<&str> = content.lines().collect();
        let symbols = scan_symbols(&lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Internal");
    }
}