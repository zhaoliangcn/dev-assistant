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

/// 获取宽容 usize 值（无默认值，缺失返回 None）
pub fn get_lenient_usize_opt(value: &serde_json::Value, key: &str) -> Result<Option<usize>, String> {
    deserialize_lenient_usize(value, key)
}

/// 宽容反序列化 Option<u64>
pub fn deserialize_lenient_u64(value: &serde_json::Value, _key: &str) -> Result<Option<u64>, String> {
    match value {
        serde_json::Value::Null => Ok(None),
        v => parse_lenient_u64_value(v).map(Some),
    }
}

/// 获取宽容 u64 值（带默认值）
pub fn get_lenient_u64(value: &serde_json::Value, key: &str, default: u64) -> Result<u64, String> {
    deserialize_lenient_u64(value, key).map(|v| v.unwrap_or(default))
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
    let unquoted = trimmed.trim_matches(['"', '\'']).trim();
    if !quote_wrapped {
        return unquoted;
    }
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

/// 扩展用户路径（处理 ~）
pub fn expand_user_path(path: &str) -> PathBuf {
    let sanitized = sanitize_model_path_arg(path);
    let expanded = shellexpand::tilde(sanitized);
    PathBuf::from(expanded.as_ref())
}

/// 规范化路径（处理 ..、.）
pub fn normalize_path(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
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

/// 参数诊断错误类型
#[derive(Debug, Clone, PartialEq)]
pub enum DiagnoseError {
    Missing { name: String, suggestion: Option<String> },
    InvalidType { name: String, expected: &'static str, actual: String },
    InvalidValue { name: String, message: String, suggestion: Option<String> },
    OutOfRange { name: String, min: Option<usize>, max: Option<usize>, value: usize },
}

impl DiagnoseError {
    pub fn to_human_message(&self) -> String {
        match self {
            DiagnoseError::Missing { name, suggestion } => {
                let mut msg = format!("缺少必需参数 `{}`", name);
                if let Some(s) = suggestion {
                    msg.push_str(&format!("。建议: {}", s));
                }
                msg
            }
            DiagnoseError::InvalidType { name, expected, actual } => {
                format!("参数 `{}` 类型错误：期望 `{}`, 实际 `{}`", name, expected, actual)
            }
            DiagnoseError::InvalidValue { name, message, suggestion } => {
                let mut msg = format!("参数 `{}` 值无效：{}", name, message);
                if let Some(s) = suggestion {
                    msg.push_str(&format!("。建议: {}", s));
                }
                msg
            }
            DiagnoseError::OutOfRange { name, min, max, value } => {
                let range_desc = match (min, max) {
                    (Some(min), Some(max)) => format!("范围应在 {} 到 {} 之间", min, max),
                    (Some(min), None) => format!("最小值应为 {}", min),
                    (None, Some(max)) => format!("最大值应为 {}", max),
                    (None, None) => "超出有效范围".to_string(),
                };
                format!("参数 `{}` 超出范围：{}, 当前值: {}", name, range_desc, value)
            }
        }
    }
}

/// 参数诊断结果
pub type DiagnoseResult<T> = Result<T, DiagnoseError>;

/// 参数诊断器
pub struct ArgsDiagnoser<'a> {
    args: &'a serde_json::Value,
    errors: Vec<DiagnoseError>,
}

impl<'a> ArgsDiagnoser<'a> {
    pub fn new(args: &'a serde_json::Value) -> Self {
        Self {
            args,
            errors: Vec::new(),
        }
    }

    /// 获取所有诊断错误
    pub fn errors(&self) -> &[DiagnoseError] {
        &self.errors
    }

    /// 是否有错误
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// 获取格式化的错误消息
    pub fn format_errors(&self) -> String {
        self.errors
            .iter()
            .enumerate()
            .map(|(i, e)| format!("{}. {}", i + 1, e.to_human_message()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 检查参数是否存在
    pub fn require(&mut self, name: &str) -> bool {
        if !self.args.get(name).is_some() {
            self.errors.push(DiagnoseError::Missing {
                name: name.to_string(),
                suggestion: None,
            });
            false
        } else {
            true
        }
    }

    /// 检查参数是否存在，带建议
    pub fn require_with_suggestion(&mut self, name: &str, suggestion: &str) -> bool {
        if !self.args.get(name).is_some() {
            self.errors.push(DiagnoseError::Missing {
                name: name.to_string(),
                suggestion: Some(suggestion.to_string()),
            });
            false
        } else {
            true
        }
    }

    /// 获取字符串参数
    pub fn get_string(&mut self, name: &str) -> Option<String> {
        if let Some(value) = self.args.get(name) {
            if value.is_null() {
                self.errors.push(DiagnoseError::Missing {
                    name: name.to_string(),
                    suggestion: None,
                });
                None
            } else if let Some(s) = value.as_str() {
                Some(s.to_string())
            } else {
                self.errors.push(DiagnoseError::InvalidType {
                    name: name.to_string(),
                    expected: "string",
                    actual: self.value_type_name(value),
                });
                None
            }
        } else {
            self.errors.push(DiagnoseError::Missing {
                name: name.to_string(),
                suggestion: None,
            });
            None
        }
    }

    /// 获取必需的字符串参数
    pub fn require_string(&mut self, name: &str) -> DiagnoseResult<String> {
        if let Some(s) = self.get_string(name) {
            Ok(s)
        } else {
            Err(self.errors.last().cloned().unwrap_or_else(|| DiagnoseError::Missing {
                name: name.to_string(),
                suggestion: None,
            }))
        }
    }

    /// 获取可选的字符串参数
    pub fn get_optional_string(&mut self, name: &str) -> Option<String> {
        if let Some(value) = self.args.get(name) {
            if value.is_null() {
                None
            } else if let Some(s) = value.as_str() {
                Some(s.to_string())
            } else {
                self.errors.push(DiagnoseError::InvalidType {
                    name: name.to_string(),
                    expected: "string",
                    actual: self.value_type_name(value),
                });
                None
            }
        } else {
            None
        }
    }

    /// 获取 usize 参数（宽容模式）
    pub fn get_usize(&mut self, name: &str) -> Option<usize> {
        if let Some(value) = self.args.get(name) {
            match get_lenient_usize(value, name, 0) {
                Ok(v) => Some(v),
                Err(e) => {
                    self.errors.push(DiagnoseError::InvalidValue {
                        name: name.to_string(),
                        message: e,
                        suggestion: None,
                    });
                    None
                }
            }
        } else {
            self.errors.push(DiagnoseError::Missing {
                name: name.to_string(),
                suggestion: None,
            });
            None
        }
    }

    /// 获取 usize 参数（带范围检查）
    pub fn get_usize_range(
        &mut self,
        name: &str,
        min: Option<usize>,
        max: Option<usize>,
    ) -> Option<usize> {
        if let Some(value) = self.get_usize(name) {
            if let Some(min) = min {
                if value < min {
                    self.errors.push(DiagnoseError::OutOfRange {
                        name: name.to_string(),
                        min: Some(min),
                        max,
                        value,
                    });
                    return None;
                }
            }
            if let Some(max) = max {
                if value > max {
                    self.errors.push(DiagnoseError::OutOfRange {
                        name: name.to_string(),
                        min,
                        max: Some(max),
                        value,
                    });
                    return None;
                }
            }
            Some(value)
        } else {
            None
        }
    }

    /// 获取可选的 usize 参数
    pub fn get_optional_usize(&mut self, name: &str) -> Option<usize> {
        if let Some(value) = self.args.get(name) {
            if value.is_null() {
                None
            } else {
                match get_lenient_usize(value, name, 0) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        self.errors.push(DiagnoseError::InvalidValue {
                            name: name.to_string(),
                            message: e,
                            suggestion: None,
                        });
                        None
                    }
                }
            }
        } else {
            None
        }
    }

    /// 获取字符串数组参数
    pub fn get_string_array(&mut self, name: &str) -> Option<Vec<String>> {
        if let Some(value) = self.args.get(name) {
            if value.is_null() {
                Some(Vec::new())
            } else if let Some(arr) = value.as_array() {
                let mut result = Vec::new();
                for (i, item) in arr.iter().enumerate() {
                    if let Some(s) = item.as_str() {
                        result.push(s.to_string());
                    } else {
                        self.errors.push(DiagnoseError::InvalidType {
                            name: format!("{}[{}]", name, i),
                            expected: "string",
                            actual: self.value_type_name(item),
                        });
                        return None;
                    }
                }
                Some(result)
            } else {
                self.errors.push(DiagnoseError::InvalidType {
                    name: name.to_string(),
                    expected: "array of strings",
                    actual: self.value_type_name(value),
                });
                None
            }
        } else {
            Some(Vec::new())
        }
    }

    /// 获取路径参数（已解析）
    pub fn get_path(&mut self, name: &str, cwd: &std::path::Path) -> Option<std::path::PathBuf> {
        self.get_string(name).map(|s| resolve_model_path(cwd, &s))
    }

    /// 获取可选路径参数（已解析）
    pub fn get_optional_path(&mut self, name: &str, cwd: &std::path::Path) -> Option<std::path::PathBuf> {
        self.get_optional_string(name).map(|s| resolve_model_path(cwd, &s))
    }

    fn value_type_name(&self, value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::Null => "null".to_string(),
            serde_json::Value::Bool(_) => "boolean".to_string(),
            serde_json::Value::Number(n) => {
                if n.is_i64() || n.is_u64() {
                    "integer".to_string()
                } else {
                    "float".to_string()
                }
            }
            serde_json::Value::String(_) => "string".to_string(),
            serde_json::Value::Array(_) => "array".to_string(),
            serde_json::Value::Object(_) => "object".to_string(),
        }
    }
}

/// 诊断参数的便捷函数
/// 
/// 为模型提供用户友好的参数错误提示，帮助模型快速修正工具调用
pub fn diagnose_args(args: &serde_json::Value, schema: &serde_json::Value) -> Vec<DiagnoseError> {
    let mut diagnoser = ArgsDiagnoser::new(args);
    
    // 检查必需参数（从 schema 中提取）
    if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
        if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
            for req in required {
                if let Some(name) = req.as_str() {
                    let suggestion = properties
                        .get(name)
                        .and_then(|p| p.get("description"))
                        .and_then(|d| d.as_str())
                        .map(|d| format!("参考: {}", d));
                    
                    if suggestion.is_some() {
                        diagnoser.require_with_suggestion(name, suggestion.as_ref().unwrap());
                    } else {
                        diagnoser.require(name);
                    }
                }
            }
        }
    }
    
    diagnoser.errors
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
    fn test_expand_user_path() {
        let result = expand_user_path("~/src");
        assert!(result.to_string_lossy().contains("src"));
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
