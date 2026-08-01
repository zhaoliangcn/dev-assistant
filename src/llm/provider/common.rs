//! 共享的 LLM 响应解析工具函数。
//!
//! 用于处理 LLM 返回的工具参数 JSON 中常见的格式错误。

use serde_json::Value;
use tracing::debug;

use crate::utils::error::AppError;

/// 解析工具调用参数（通用版本，适用于所有 provider）。
///
/// LLM 经常生成格式不规范的 JSON（未转义换行符、markdown fence、trailing comma 等），
/// 此函数通过多重修复策略提高解析成功率，避免因格式问题导致工具执行失败和重试循环。
pub(crate) fn parse_arguments(args: Value) -> Result<Value, AppError> {
    if let Some(s) = args.as_str() {
        if let Ok(parsed) = try_parse_json_args(s) {
            return Ok(parsed);
        }
        // 尽力修复后仍无法解析，返回原始字符串作为 Value，
        // 避免整个响应解析失败，让工具执行器有机会给出可读错误。
        let preview: String = s.chars().take(80).collect();
        debug!(
            len = s.len(),
            preview = %preview,
            "Failed to parse tool arguments as JSON, falling back to raw string"
        );
        Ok(serde_json::Value::String(s.to_string()))
    } else {
        Ok(args)
    }
}

/// 尝试以多种容错策略解析 LLM 生成的工具参数 JSON。
///
/// 常见模型错误：
/// - 字符串值中包含未转义的换行符
/// - 外层包裹 markdown code fence
/// - 首尾空白
/// - 尾随逗号
pub(crate) fn try_parse_json_args(raw: &str) -> Result<Value, serde_json::Error> {
    // 1. 原样解析
    if let Ok(v) = serde_json::from_str(raw) {
        return Ok(v);
    }

    // 2. 去除首尾空白和 markdown fence
    let trimmed = raw.trim();
    let without_fence = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|s| s.strip_suffix("```"))
        .unwrap_or(trimmed)
        .trim();
    if let Ok(v) = serde_json::from_str(without_fence) {
        return Ok(v);
    }

    // 3. 转义字符串值内部的实际换行符（LLM 最容易犯的 JSON 错误）
    let escaped = escape_newlines_in_json(without_fence);
    if let Ok(v) = serde_json::from_str(&escaped) {
        return Ok(v);
    }

    // 4. 移除 trailing comma（仅在字符串外部）
    let no_trailing_comma = remove_trailing_commas(without_fence);
    if let Ok(v) = serde_json::from_str(&no_trailing_comma) {
        return Ok(v);
    }

    serde_json::from_str(raw)
}

/// 在 JSON 字符串字面量内部转义未转义的换行符。
///
/// 使用轻量级状态机：遇到 `"` 切换 in_string 状态；
/// 在字符串内部将实际换行符替换为转义序列 `\n`。
fn escape_newlines_in_json(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_string = false;
    let mut escape = false;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if escape {
            result.push(c);
            escape = false;
            continue;
        }
        if c == '\\' {
            result.push(c);
            escape = true;
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            result.push(c);
            continue;
        }
        if in_string && (c == '\n' || c == '\r' || c == '\t') {
            match c {
                '\n' | '\r' => {
                    // 将实际换行符转换为转义序列 `\n`
                    result.push_str("\\n");
                    // 跳过 \r 后的 \n，避免生成 \n\n
                    if c == '\r' && chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                }
                '\t' => result.push_str("\\t"),
                _ => unreachable!(),
            }
            continue;
        }
        result.push(c);
    }

    result
}

/// 移除 JSON 中的尾随逗号（仅在字符串外部）。
///
/// 使用状态机跟踪是否在字符串内部，避免误修改字符串值中的逗号。
/// 例如 `{"cmd": "echo foo,}"}` 不会被破坏。
fn remove_trailing_commas(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_string = false;
    let mut escape = false;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();

    for i in 0..len {
        let c = chars[i];
        if escape {
            result.push(c);
            escape = false;
            continue;
        }
        if c == '\\' && in_string {
            result.push(c);
            escape = true;
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            result.push(c);
            continue;
        }
        // 仅在字符串外部移除尾随逗号
        if !in_string && c == ',' {
            // 检查下一个非空白字符是否是 } 或 ]
            let mut j = i + 1;
            while j < len && chars[j].is_whitespace() {
                j += 1;
            }
            if j < len && (chars[j] == '}' || chars[j] == ']') {
                continue; // 跳过这个逗号
            }
        }
        result.push(c);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_arguments_parses_valid_json_object() {
        let args = serde_json::json!({"path": "test.rs", "encoding": "utf-8"});
        let parsed = parse_arguments(args).unwrap();
        assert_eq!(parsed["path"], "test.rs");
    }

    #[test]
    fn test_parse_arguments_handles_unescaped_newlines_in_strings() {
        // LLM 经常生成字符串内部含未转义换行符的非法 JSON，例如：
        // {"path":"file.rs","content":"fn main() {
        //     println!(\"hello\");
        // }"}
        // 其中 \n 是真实换行，但 \"hello\" 被正确转义
        let raw_json = "{\n  \"path\": \"file.rs\",\n  \"content\": \"fn main() {\n    println!(\\\"hello\\\");\n  }\"\n}";
        let args = serde_json::Value::String(raw_json.to_string());
        let parsed = parse_arguments(args).unwrap();
        assert_eq!(parsed["path"], "file.rs");
        assert!(parsed["content"].as_str().unwrap().contains("println!"));
    }

    #[test]
    fn test_parse_arguments_strips_markdown_fence() {
        let args = serde_json::json!("```json\n{\"key\": \"value\"}\n```");
        let parsed = parse_arguments(args).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn test_parse_arguments_removes_trailing_comma() {
        let args = serde_json::json!("\
            {\
                \"key1\": \"value1\",\
                \"key2\": \"value2\",\
            }\
        ");
        let parsed = parse_arguments(args).unwrap();
        assert_eq!(parsed["key1"], "value1");
        assert_eq!(parsed["key2"], "value2");
    }

    #[test]
    fn test_parse_arguments_falls_back_to_raw_string_on_unrecoverable_input() {
        // 完全无法修复的输入应回退为原始字符串
        let args = serde_json::json!("[invalid json{{{");
        let parsed = parse_arguments(args).unwrap();
        // 回退时保持原始字符串
        assert!(parsed.is_string());
    }

    #[test]
    fn test_escape_newlines_preserves_existing_escapes() {
        // 已转义的 \n 不应被再次处理
        let input = r#"{"key": "value\nwith\nnewlines"}"#;
        let result = escape_newlines_in_json(input);
        // 已存在的 \n（两个字符）应保持原样
        assert_eq!(result, input);
    }

    #[test]
    fn test_escape_newlines_only_touches_real_newlines() {
        // 字符串内部的真实换行符应被转义
        let input = "{\n  \"text\": \"line1\nline2\"\n}";
        let result = escape_newlines_in_json(input);
        // 输出应该是合法的 JSON，\n 是两个字符合法转义序列
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["text"], "line1\nline2");
    }
}
