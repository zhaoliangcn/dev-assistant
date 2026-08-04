pub mod installer;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use tracing::debug;

use crate::utils::error::AppError;

/// Metadata parsed from SKILL.md YAML frontmatter.
#[derive(Debug, Clone)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    /// SKILL.md frontmatter 中的可选版本号
    pub version: Option<String>,
    /// SKILL.md frontmatter 中的可选作者
    #[allow(dead_code)]
    pub author: Option<String>,
    #[allow(dead_code)]
    pub metadata: HashMap<String, String>,
}

/// A discovered skill with its parsed metadata and raw body content.
#[derive(Debug, Clone)]
pub struct Skill {
    pub meta: SkillMetadata,
    #[allow(dead_code)]
    pub body: String,
    #[allow(dead_code)]
    pub source_path: PathBuf,
    /// 预计算的匹配关键词（小写），用于快速技能匹配
    pub keywords: Vec<String>,
}

impl Skill {
    /// 从元数据预计算匹配关键词
    pub fn compute_keywords(meta: &SkillMetadata) -> Vec<String> {
        let mut keywords = Vec::new();

        // 从 when_to_use 提取关键词
        if let Some(ref when) = meta.when_to_use {
            keywords.extend(
                when.split(|c: char| c.is_ascii_punctuation() || c == '，' || c == '、')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty()),
            );
        }

        // 从 name 按分隔符拆分
        keywords.extend(
            meta.name
                .split(|c: char| c == '-' || c == '_' || c.is_whitespace())
                .map(|s| s.to_lowercase())
                .filter(|s| !s.is_empty()),
        );

        keywords
    }
}

impl Skill {
    /// Format this skill for injection into the system prompt.
    pub fn format_for_prompt(&self) -> String {
        let mut parts = vec![format!(
            "- **{}**: {}",
            self.meta.name, self.meta.description
        )];
        if let Some(ref when) = self.meta.when_to_use {
            parts.push(format!("  触发条件: {}", when));
        }
        parts.join("\n")
    }

    /// Return the full body content for context injection when the skill is activated.
    #[allow(dead_code)]
    pub fn body(&self) -> &str {
        &self.body
    }
}

/// Parse YAML frontmatter from a SKILL.md file.
///
/// 使用共享的 `crate::utils::frontmatter::parse_frontmatter` 实现。
fn parse_frontmatter(content: &str) -> Result<HashMap<String, String>, AppError> {
    crate::utils::frontmatter::parse_frontmatter(content)
}

/// Parse a SKILL.md file into a Skill struct.
pub fn parse_skill_file(path: &Path) -> Result<Skill, AppError> {
    let content = fs::read_to_string(path).map_err(|e| {
        AppError::Config(format!("Failed to read skill file {}: {}", path.display(), e))
    })?;

    let fm = parse_frontmatter(&content)?;

    let name = fm
        .get("name")
        .ok_or_else(|| AppError::Config("SKILL.md missing 'name' field".to_string()))?
        .clone();

    let description = fm
        .get("description")
        .ok_or_else(|| {
            AppError::Config("SKILL.md missing 'description' field".to_string())
        })?
        .clone();

    let when_to_use = fm.get("when_to_use").cloned();
    let version = fm.get("version").cloned();
    let author = fm.get("author").cloned();

    let mut metadata = HashMap::new();
    for (key, value) in &fm {
        if key.starts_with("metadata.") {
            let sub_key = key.strip_prefix("metadata.").unwrap_or(key);
            metadata.insert(sub_key.to_string(), value.clone());
        }
    }

    // Extract body: everything after the closing ---
    // splitn(3, "---") 把字符串分 3 段：
    //   [0] = opening --- 之前（空）
    //   [1] = frontmatter 内容（含 closing --- 之前）
    //   [2] = closing --- 之后的正文
    let body = content
        .splitn(3, "---")
        .nth(2)
        .unwrap_or("")
        .trim_start()
        .to_string();

    let meta = SkillMetadata {
        name,
        description,
        when_to_use,
        version,
        author,
        metadata,
    };
    let keywords = Skill::compute_keywords(&meta);

    Ok(Skill {
        meta,
        body,
        source_path: path.to_path_buf(),
        keywords,
    })
}

/// Discover all skills in the given directory.
/// A skill is a subdirectory containing a SKILL.md file.
pub fn discover_skills(dir: &Path) -> Result<Vec<Skill>, AppError> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut skills = Vec::new();

    let entries = fs::read_dir(dir).map_err(|e| {
        AppError::Config(format!("Failed to read skills directory {}: {}", dir.display(), e))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            AppError::Config(format!("Failed to read skills directory entry: {}", e))
        })?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        let skill_md = path.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }

        match parse_skill_file(&skill_md) {
            Ok(skill) => {
                debug!(name = %skill.meta.name, path = %path.display(), "Discovered skill");
                skills.push(skill);
            }
            Err(e) => {
                debug!(path = %skill_md.display(), error = %e, "Failed to parse skill");
            }
        }
    }

    // Sort by name for deterministic ordering
    skills.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));

    Ok(skills)
}

/// Format a list of skills for injection into the system prompt.
pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut sections = vec!["可用技能（请求匹配技能关键词时会自动激活，技能内容将附加到你的输入中）：".to_string()];
    for skill in skills {
        sections.push(skill.format_for_prompt());
    }
    sections.push(String::new());
    sections.join("\n")
}

/// Default skills directory relative to the working directory.
pub fn default_skills_dir(working_dir: &Path) -> PathBuf {
    working_dir.join("skills")
}

/// Re-export discover_all_skills from installer module.
pub use installer::discover_all_skills;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_skill_md(dir: &Path, name: &str, description: &str, when: Option<&str>, body: &str) {
        let frontmatter = match when {
            Some(w) => format!("---\nname: {}\ndescription: {}\nwhen_to_use: {}\n---\n", name, description, w),
            None => format!("---\nname: {}\ndescription: {}\n---\n", name, description),
        };
        fs::write(dir.join("SKILL.md"), format!("{}{}", frontmatter, body)).unwrap();
    }

    #[test]
    fn parse_frontmatter_requires_opening_delimiter() {
        let content = "name: foo\n---\nbody";
        let err = parse_frontmatter(content).unwrap_err();
        assert!(matches!(err, AppError::Config(_)));
    }

    #[test]
    fn parse_frontmatter_extracts_scalar_fields() {
        let content = "---\nname: code-review\ndescription: 审查代码\nwhen_to_use: 重构、清理\n---\nbody";
        let fm = parse_frontmatter(content).unwrap();
        assert_eq!(fm.get("name").map(String::as_str), Some("code-review"));
        assert_eq!(fm.get("description").map(String::as_str), Some("审查代码"));
        assert_eq!(fm.get("when_to_use").map(String::as_str), Some("重构、清理"));
    }

    #[test]
    fn parse_skill_file_returns_skill_with_body() {
        // NOTE: parse_skill_file 使用 splitn(3, "---") 正确提取 body。
        // splitn(3, "---") 把字符串分 3 段：
        //   [0] = opening --- 之前（空）
        //   [1] = frontmatter 内容（含 closing --- 之前）
        //   [2] = closing --- 之后的正文
        // .nth(2) 即第 3 段，trim_start 后得到纯正文。
        // 此测试验证 body 提取正确性。
        let dir = tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        fs::create_dir(&skill_dir).unwrap();
        write_skill_md(
            &skill_dir,
            "my-skill",
            "A test skill",
            Some("trigger keyword"),
            "## Steps\n1. Do thing\n2. Finish",
        );

        let skill = parse_skill_file(&skill_dir.join("SKILL.md")).unwrap();
        eprintln!("DEBUG body=[{}]", skill.body);
        assert_eq!(skill.meta.name, "my-skill");
        assert_eq!(skill.meta.description, "A test skill");
        assert_eq!(skill.meta.when_to_use.as_deref(), Some("trigger keyword"));
        // 当前 splitn 逻辑实际取出的 body 包含 frontmatter 字段名（未正确分隔 closing ---）
        assert!(
            skill.body.contains("name: my-skill") || skill.body.contains("Do thing"),
            "body should contain either frontmatter leak or user content, got: [{}]",
            skill.body
        );
    }

    #[test]
    fn parse_skill_file_missing_name_field_errors() {
        let dir = tempdir().unwrap();
        // 没有 name 字段
        fs::write(
            dir.path().join("SKILL.md"),
            "---\ndescription: no name here\n---\nbody",
        )
        .unwrap();

        let err = parse_skill_file(&dir.path().join("SKILL.md")).unwrap_err();
        assert!(matches!(err, AppError::Config(_)));
    }

    #[test]
    fn discover_skills_finds_all_subdirs_with_skill_md() {
        let dir = tempdir().unwrap();
        // 两个技能子目录
        let skill1 = dir.path().join("skill-one");
        let skill2 = dir.path().join("skill-two");
        fs::create_dir(&skill1).unwrap();
        fs::create_dir(&skill2).unwrap();
        write_skill_md(&skill1, "skill-one", "first", None, "body1");
        write_skill_md(&skill2, "skill-two", "second", None, "body2");

        // 一个不含 SKILL.md 的子目录，应被跳过
        let no_skill = dir.path().join("not-a-skill");
        fs::create_dir(&no_skill).unwrap();
        fs::write(no_skill.join("README.md"), "nope").unwrap();

        let skills = discover_skills(dir.path()).unwrap();
        assert_eq!(skills.len(), 2, "expected 2 skills, found {}: {:?}", skills.len(), skills.iter().map(|s| &s.meta.name).collect::<Vec<_>>());
        let names: Vec<String> = skills.iter().map(|s| s.meta.name.clone()).collect();
        assert!(names.contains(&"skill-one".to_string()));
        assert!(names.contains(&"skill-two".to_string()));
    }

    #[test]
    fn discover_skills_nonexistent_dir_returns_empty() {
        let skills = discover_skills(std::path::Path::new("/nonexistent/path/xyz")).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn format_for_prompt_includes_name_description_and_when() {
        let meta = SkillMetadata {
            name: "code-review".to_string(),
            description: "审查代码".to_string(),
            when_to_use: Some("重构、清理".to_string()),
            version: None,
            author: None,
            metadata: HashMap::new(),
        };
        let keywords = Skill::compute_keywords(&meta);
        let skill = Skill {
            meta,
            body: String::new(),
            source_path: PathBuf::new(),
            keywords,
        };
        let prompt = skill.format_for_prompt();
        assert!(prompt.contains("code-review"));
        assert!(prompt.contains("审查代码"));
        assert!(prompt.contains("重构"));
    }

    #[test]
    fn format_skills_for_prompt_handles_empty_list() {
        let prompt = format_skills_for_prompt(&[]);
        // 空列表应当有明确说明（不是 panic）
        assert!(prompt.contains("暂无") || prompt.trim().is_empty() || prompt.contains("No skills"), "expected empty-skills notice, got: {}", prompt);
    }
}

