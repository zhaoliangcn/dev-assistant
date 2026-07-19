//! 通用 YAML Frontmatter 解析工具。
//!
//! 提供解析、提取和构建 YAML frontmatter 的公共函数。
//! 被 `skills/`、`tools/kb/` 等模块共享使用。
//!
//! # 格式
//!
//! ```markdown
//! ---
//! key: value
//! list: [item1, item2]
//! nested:
//!   sub_key: sub_value
//! ---
//! Body content here...
//! ```

use std::collections::HashMap;

use crate::utils::error::AppError;

/// 解析 YAML frontmatter。
///
/// 期望内容以 `---` 开头，后跟 YAML 键值对，以 `---` 结束。
/// 返回 frontmatter 中的键值对。
///
/// 支持的类型：
/// - 标量值（字符串）
/// - 数组（如 `tags: [a, b]` → 以逗号拼接的字符串 `"a,b"`）
/// - 嵌套 map（如 `metadata: { author: foo }` → `"metadata.author" = "foo"`）
pub fn parse_frontmatter(content: &str) -> Result<HashMap<String, String>, AppError> {
    let mut frontmatter = HashMap::new();

    let mut lines = content.lines();
    let first = lines.next();

    if first != Some("---") {
        return Err(AppError::Config(
            "Content missing opening --- frontmatter delimiter".to_string(),
        ));
    }

    // 收集 closing --- 之前的行
    let mut fm_content = String::new();
    for line in lines {
        if line == "---" {
            break;
        }
        fm_content.push_str(line);
        fm_content.push('\n');
    }

    // 使用 yaml-rust2 解析 YAML
    let yaml = yaml_rust2::yaml::YamlLoader::load_from_str(&fm_content)
        .map_err(|e| AppError::Config(format!("Failed to parse frontmatter: {}", e)))?;

    if let Some(yaml_hash) = yaml.first().and_then(|y| y.as_hash()) {
        for (key, value) in yaml_hash {
            let key_str = key.as_str().unwrap_or_default().to_string();
            if let Some(val_str) = value.as_str() {
                frontmatter.insert(key_str, val_str.to_string());
            } else if value.as_hash().is_some() {
                // 处理嵌套 map（如 metadata: { author, version }）
                if let Some(sub_hash) = value.as_hash() {
                    for (mk, mv) in sub_hash {
                        let mk_str = mk.as_str().unwrap_or_default().to_string();
                        if let Some(mv_str) = mv.as_str() {
                            frontmatter
                                .insert(format!("metadata.{}", mk_str), mv_str.to_string());
                        }
                    }
                }
            } else if value.as_vec().is_some() {
                // 处理数组（如 tags: [a, b]）
                if let Some(vec) = value.as_vec() {
                    let items: Vec<String> = vec
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect();
                    frontmatter.insert(key_str, items.join(","));
                }
            }
        }
    }

    Ok(frontmatter)
}

/// 提取 frontmatter 之后的正文内容。
///
/// 返回 closing `---` 之后的所有内容（去除开头的空白）。
pub fn extract_body(content: &str) -> String {
    content
        .splitn(3, "---")
        .nth(2)
        .unwrap_or("")
        .trim_start()
        .to_string()
}

/// 检查内容是否包含有效的 frontmatter 分隔符。
#[allow(dead_code)]
pub fn has_frontmatter(content: &str) -> bool {
    content.trim().starts_with("---")
}

/// 构建带 frontmatter 的完整 Markdown 内容。
///
/// # 参数
/// * `fields` - frontmatter 字段
/// * `body` - 正文 Markdown 内容
#[allow(dead_code)]
pub fn build_document(fields: &HashMap<String, String>, body: &str) -> String {
    let mut result = String::from("---\n");
    for (key, value) in fields {
        result.push_str(&format!("{}: {}\n", key, value));
    }
    result.push_str("---\n");
    if !body.is_empty() {
        result.push_str(body);
        if !body.ends_with('\n') {
            result.push('\n');
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_requires_opening_delimiter() {
        let content = "name: foo\n---\nbody";
        let err = parse_frontmatter(content).unwrap_err();
        assert!(matches!(err, AppError::Config(_)));
    }

    #[test]
    fn parse_frontmatter_extracts_scalar_fields() {
        let content = "---\nname: code-review\ndescription: 审查代码\n---\nbody";
        let fm = parse_frontmatter(content).unwrap();
        assert_eq!(fm.get("name").map(String::as_str), Some("code-review"));
        assert_eq!(fm.get("description").map(String::as_str), Some("审查代码"));
    }

    #[test]
    fn parse_frontmatter_extracts_array_fields() {
        let content = "---\ntags: [architecture, rendering]\n---\nbody";
        let fm = parse_frontmatter(content).unwrap();
        assert_eq!(fm.get("tags").map(String::as_str), Some("architecture,rendering"));
    }

    #[test]
    fn parse_frontmatter_empty_content() {
        let content = "---\n---\nbody";
        let fm = parse_frontmatter(content).unwrap();
        assert!(fm.is_empty(), "empty frontmatter should produce empty map");
    }

    #[test]
    fn extract_body_returns_content_after_frontmatter() {
        let content = "---\nname: test\n---\n\n## Body Content\n\nHello World";
        let body = extract_body(content);
        assert_eq!(body, "## Body Content\n\nHello World");
    }

    #[test]
    fn extract_body_returns_empty_for_no_frontmatter() {
        let content = "No frontmatter here";
        let body = extract_body(content);
        assert_eq!(body, "");
    }

    #[test]
    fn has_frontmatter_detects_frontmatter() {
        assert!(has_frontmatter("---\nname: test\n---\nbody"));
        assert!(!has_frontmatter("no frontmatter"));
        assert!(!has_frontmatter(""));
    }

    #[test]
    fn build_document_creates_valid_document() {
        let mut fields = HashMap::new();
        fields.insert("type".to_string(), "decision".to_string());
        fields.insert("title".to_string(), "Test".to_string());
        fields.insert("tags".to_string(), "a,b,c".to_string());

        let doc = build_document(&fields, "# Body\n\nContent here.");
        assert!(doc.starts_with("---\n"));
        assert!(doc.contains("type: decision"));
        assert!(doc.contains("title: Test"));
        assert!(doc.contains("tags: a,b,c"));
        assert!(doc.contains("---\n# Body"));
        assert!(doc.contains("Content here."));
    }

    #[test]
    fn build_document_with_empty_body() {
        let mut fields = HashMap::new();
        fields.insert("type".to_string(), "test".to_string());

        let doc = build_document(&fields, "");
        assert!(doc.ends_with("---\n"), "empty body should still end with closing delimiter");
    }
}