//! 共享工具函数：宽容参数反序列化、路径规范化。

use std::path::{Path, PathBuf};

/// 最大精确整数限制 (2^53)
const F64_EXACT_INTEGER_LIMIT: f64 = 9_007_199_254_740_992.0;

/// 解析字符串为 f64
fn parse_string_to_f64(s: &str) -> Result<f64, String> {
    s.parse().map_err(|_| format!("expected number, got string \"{s}\""))
}

/// 解析完整 f64 为 i64（拒绝非有限、小数、超出范围的值）
fn parse_lenient_whole_f64(f: f64) -> Result<i64, String> {
    if !f.is_finite() {
        return Err("expected finite number".into());
    }
    if f == 0.0 {
        return Ok(0);
    }
    if f.fract() != 0.0 {
        return Err(format!("expected whole number, got {f}"));
    }
    if f.abs() > F64_EXACT_INTEGER_LIMIT {
        return Err(format!(
            "number {f} exceeds f64 integer precision (whole floats above {F64_EXACT_INTEGER_LIMIT} may be inaccurate)"
        ));
    }
    if f > i64::MAX as f64 || f < i64::MIN as f64 {
        return Err("number out of range for i64".into());
    }
    Ok(f as i64)
}

/// 解析 JSON 值为 u64（支持数字、字符串形式）
fn parse_lenient_u64_value(value: &serde_json::Value) -> Result<u64, String> {
    match value {
        serde_json::Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                return Ok(u);
            }
            if let Some(i) = n.as_i64() {
                if i < 0 {
                    return Err("expected non-negative number".into());
                }
                return u64::try_from(i).map_err(|_| "number out of range for u64".into());
            }
            if let Some(f) = n.as_f64() {
                let i = parse_lenient_whole_f64(f)?;
                return u64::try_from(i).map_err(|_| "expected non-negative number".to_string());
            }
            Err("expected number, got invalid numeric representation".into())
        }
        serde_json::Value::String(s) => {
            let i = parse_lenient_whole_f64(parse_string_to_f64(s)?)?;
            u64::try_from(i).map_err(|_| "expected non-negative number".to_string())
        }
        other => Err(format!("expected number, got {other}")),
    }
}

/// 宽容反序列化 Option<usize>
/// 
/// 支持以下输入格式：
/// - 整数: `50`
/// - 字符串整数: `"50"`
/// - 浮点数（必须是整数）: `50.0`
/// - 字符串浮点数（必须是整数）: `"50.0"`
/// - null / 缺失: 返回 None
pub fn deserialize_lenient_usize(value: &serde_json::Value, key: &str) -> Result<Option<usize>, String> {
    match value {
        serde_json::Value::Null => Ok(None),
        v => {
            let u = parse_lenient_u64_value(v)?;
            usize::try_from(u)
                .map(Some)
                .map_err(|_| format!("{} out of range for usize: {}", key, u))
        }
    }
}

/// 获取宽容 usize 值（带默认值）
pub fn get_lenient_usize(value: &serde_json::Value, key: &str, default: usize) -> Result<usize, String> {
    deserialize_lenient_usize(value, key).map(|v| v.unwrap_or(default))
}

/// 清理模型提供的路径参数（去除引号、转义序列）
/// 
/// 处理模型可能发送的格式：
/// - `"path/to/file"` → `path/to/file`
/// - `'path/to/file'` → `path/to/file`
/// - `"path/to/file\n"` → `path/to/file`
pub fn sanitize_model_path_arg(input: &str) -> &str {
    let trimmed = input.trim();
    let quote_wrapped =
        trimmed.len() >= 2 && trimmed.starts_with(['"', '\'']) && trimmed.ends_with(['"', '\'']);
    if !quote_wrapped {
        return trimmed;
    }
    let unquoted = trimmed.trim_matches(['"', '\'']).trim();
    let mut result = unquoted;
    while let Some(stripped) = result
        .strip_suffix("\\n")
        .or_else(|| result.strip_suffix("\\r"))
        .or_else(|| result.strip_suffix("\\t"))
    {
        result = stripped.trim_end();
    }
    result
}

/// 解析模型提供的路径
/// 
/// - 支持 ~ 扩展
/// - 支持引号包裹的路径
/// - 支持转义序列清理
/// - 相对路径会自动解析为相对于工作目录的绝对路径
pub fn resolve_model_path(cwd: &Path, input: &str) -> PathBuf {
    let input = sanitize_model_path_arg(input);
    let expanded = shellexpand::tilde(input);
    let input_path = Path::new(expanded.as_ref());
    
    if input_path.is_absolute() {
        input_path.to_path_buf()
    } else if expanded.is_empty() {
        cwd.to_path_buf()
    } else {
        cwd.join(input_path)
    }
}

/// 检查路径是否被 gitignore 忽略
/// 
/// 返回 None 表示不应忽略，Some(reason) 表示应忽略及原因
pub fn check_gitignore(
    path: &Path,
    resources: &Option<crate::tools::resources::SharedResources>,
) -> Option<String> {
    if let Some(ref resources) = resources {
        let resources = resources.read().unwrap();
        
        // 检查是否启用了 gitignore 过滤
        if let Some(respect_gitignore) = resources.get::<crate::tools::resources::RespectGitignore>() {
            if !respect_gitignore.0 {
                return None;
            }
        }
        
        // 获取 gitignore 过滤器
        if let Some(filter) = resources.get::<crate::tools::resources::GitignoreFilter>() {
            if filter.is_ignored(path) {
                return Some(format!(
                    "路径 {} 被 .gitignore 忽略，如需读取请修改 .gitignore 或禁用 gitignore 过滤",
                    path.display()
                ));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deserialize_lenient_usize_accepts_integer() {
        let value = json!(50);
        assert_eq!(deserialize_lenient_usize(&value, "test").unwrap(), Some(50));
    }

    #[test]
    fn deserialize_lenient_usize_accepts_string_integer() {
        let value = json!("50");
        assert_eq!(deserialize_lenient_usize(&value, "test").unwrap(), Some(50));
    }

    #[test]
    fn deserialize_lenient_usize_accepts_whole_float() {
        let value = json!(80.0);
        assert_eq!(deserialize_lenient_usize(&value, "test").unwrap(), Some(80));
    }

    #[test]
    fn deserialize_lenient_usize_accepts_string_whole_float() {
        let value = json!("120.0");
        assert_eq!(deserialize_lenient_usize(&value, "test").unwrap(), Some(120));
    }

    #[test]
    fn deserialize_lenient_usize_null_is_none() {
        let value = json!(null);
        assert_eq!(deserialize_lenient_usize(&value, "test").unwrap(), None);
    }

    #[test]
    fn deserialize_lenient_usize_rejects_fractional_float() {
        let value = json!(80.5);
        let err = deserialize_lenient_usize(&value, "test").unwrap_err();
        assert!(err.contains("whole number"));
    }

    #[test]
    fn deserialize_lenient_usize_rejects_negative() {
        let value = json!(-1);
        let err = deserialize_lenient_usize(&value, "test").unwrap_err();
        assert!(err.contains("non-negative"));
    }

    #[test]
    fn deserialize_lenient_usize_rejects_string_fractional_float() {
        let value = json!("80.5");
        let err = deserialize_lenient_usize(&value, "test").unwrap_err();
        assert!(err.contains("whole number"));
    }

    #[test]
    fn deserialize_lenient_usize_rejects_non_numeric_string() {
        let value = json!("abc");
        let err = deserialize_lenient_usize(&value, "test").unwrap_err();
        assert!(err.contains("expected number"));
    }

    #[test]
    fn get_lenient_usize_with_default() {
        let value = json!(null);
        assert_eq!(get_lenient_usize(&value, "test", 100).unwrap(), 100);
        
        let value = json!(50);
        assert_eq!(get_lenient_usize(&value, "test", 100).unwrap(), 50);
    }

    #[test]
    fn sanitize_model_path_arg_strips_quotes() {
        assert_eq!(sanitize_model_path_arg("\"src/main.rs\""), "src/main.rs");
        assert_eq!(sanitize_model_path_arg("'src/main.rs'"), "src/main.rs");
    }

    #[test]
    fn sanitize_model_path_arg_strips_escapes() {
        assert_eq!(sanitize_model_path_arg("\"src/main.rs\\n\""), "src/main.rs");
        assert_eq!(sanitize_model_path_arg("\"src/main.rs\\r\\n\""), "src/main.rs");
    }

    #[test]
    fn sanitize_model_path_arg_keeps_literal_backslash() {
        assert_eq!(sanitize_model_path_arg("src/main.rs"), "src/main.rs");
        assert_eq!(sanitize_model_path_arg("src\\main.rs"), "src\\main.rs");
    }

    #[test]
    fn resolve_model_path_relative() {
        let cwd = Path::new("/workspace");
        let result = resolve_model_path(cwd, "src/main.rs");
        assert_eq!(result, PathBuf::from("/workspace/src/main.rs"));
    }

    #[test]
    fn resolve_model_path_absolute() {
        let cwd = Path::new("/workspace");
        let result = resolve_model_path(cwd, "/etc/hosts");
        assert_eq!(result, PathBuf::from("/etc/hosts"));
    }

    #[test]
    fn resolve_model_path_with_tilde() {
        let cwd = Path::new("/workspace");
        let result = resolve_model_path(cwd, "~/src/main.rs");
        assert!(result.to_string_lossy().contains("src/main.rs"));
    }

    #[test]
    fn resolve_model_path_with_quotes() {
        let cwd = Path::new("/workspace");
        let result = resolve_model_path(cwd, "\"src/main.rs\"");
        assert_eq!(result, PathBuf::from("/workspace/src/main.rs"));
    }
}
