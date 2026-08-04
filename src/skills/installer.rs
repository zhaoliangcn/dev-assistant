//! 技能安装/卸载/更新逻辑。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::{debug, info, warn};

use super::{parse_skill_file, Skill};
use super::discover_skills;
use crate::utils::error::AppError;
use crate::utils::git as git_utils;

/// 技能安装范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallScope {
    /// 全局级：~/.dev-assistant/skills/<name>/
    Global,
    /// 项目级：<cwd>/.dev-assistant/skills/<name>/
    Project,
}

impl std::fmt::Display for InstallScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallScope::Global => write!(f, "global"),
            InstallScope::Project => write!(f, "project"),
        }
    }
}

/// 技能来源类型。
#[derive(Debug, Clone)]
pub enum SkillSource {
    Git {
        source: String,
        branch: Option<String>,
        #[allow(dead_code)]
        subdir: Option<String>,
    },
    Local {
        path: PathBuf,
    },
}

/// 已安装技能的元数据（存储在 .skill-meta.json 中）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillMeta {
    pub source: String,
    pub git_url: Option<String>,
    pub git_branch: Option<String>,
    pub source_path: Option<String>,
    pub installed_at: u64,
}

impl SkillMeta {
    pub fn new(source: &str, skill_source: &SkillSource) -> Self {
        let (git_url, git_branch, _subdir) = match skill_source {
            SkillSource::Git { source, branch, .. } => (Some(source.clone()), branch.clone(), None::<String>),
            SkillSource::Local { .. } => (None, None, None),
        };
        Self {
            source: source.to_string(),
            git_url,
            git_branch,
            source_path: match skill_source {
                SkillSource::Local { path } => Some(path.to_string_lossy().to_string()),
                _ => None,
            },
            installed_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }
}

/// 获取全局技能目录路径。
pub fn global_skills_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".dev-assistant").join("skills")
}

/// 获取项目技能目录路径。
pub fn project_skills_dir(working_dir: &Path) -> PathBuf {
    working_dir.join(".dev-assistant").join("skills")
}

/// 合并全局和项目技能，项目技能优先（同名时覆盖）。
pub fn discover_all_skills(working_dir: &Path) -> Result<Vec<Skill>, AppError> {
    let mut skills: HashMap<String, Skill> = HashMap::new();

    // 先加载全局技能
    let global_dir = global_skills_dir();
    for skill in discover_skills(&global_dir)? {
        skills.insert(skill.meta.name.clone(), skill);
    }

    // 再加载项目技能（覆盖同名的全局技能）
    let project_dir = project_skills_dir(working_dir);
    for skill in discover_skills(&project_dir)? {
        skills.insert(skill.meta.name.clone(), skill);
    }

    let mut result: Vec<Skill> = skills.into_values().collect();
    result.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));
    Ok(result)
}

/// 安装单个技能。
///
/// - Git 来源：clone 到临时目录 → 找到目标技能 → 复制到目标路径
/// - 本地来源：直接复制
///
/// 返回已安装的技能列表。
pub async fn install_skill(
    source: &str,
    target_scope: InstallScope,
    working_dir: &Path,
    skill_name_filter: Option<&[String]>,
) -> Result<Vec<Skill>, AppError> {
    let skill_source = parse_source(source);
    let target_dir = match target_scope {
        InstallScope::Global => global_skills_dir(),
        InstallScope::Project => project_skills_dir(working_dir),
    };

    fs::create_dir_all(&target_dir).map_err(|e| {
        AppError::Config(format!("Failed to create skills directory {}: {}", target_dir.display(), e))
    })?;

    let mut installed = Vec::new();
    let temp_dir = match &skill_source {
        SkillSource::Git { .. } => {
            let temp = std::env::temp_dir().join(format!("dev-assistant-skill-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&temp).map_err(|e| AppError::Config(format!("Failed to create temp dir: {}", e)))?;

            let repo_path = git_utils::clone_repo(source, &temp)?;

            // 解析分支和子目录
            let (_base, branch, subdir) = git_utils::parse_git_source(source);
            let _ = git_utils::checkout_branch(&repo_path, branch.as_deref());

            Some((temp, repo_path, branch, subdir))
        }
        SkillSource::Local { path } => {
            if !path.is_dir() {
                return Err(AppError::Config(format!(
                    "Local skill source is not a directory: {}",
                    path.display()
                )));
            }
            None
        }
    };

    // 确定要扫描的目录
    let scan_dirs = match &skill_source {
        SkillSource::Git { .. } => {
            let (_, repo_path, _branch, subdir) = temp_dir.as_ref().unwrap();
            git_utils::list_skill_dirs(repo_path, subdir.as_deref())
        }
        SkillSource::Local { path } => {
            git_utils::list_skill_dirs(path, None)
        }
    };

    // 解析每个 SKILL.md
    let mut all_skills: Vec<(Skill, PathBuf)> = Vec::new();
    for dir in &scan_dirs {
        let skill_md = dir.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }
        match parse_skill_file(&skill_md) {
            Ok(skill) => all_skills.push((skill, dir.clone())),
            Err(e) => {
                debug!(path = %skill_md.display(), error = %e, "Failed to parse skill file");
            }
        }
    }

    // 如果没有指定 skill 名，安装所有；否则只安装匹配的
    let should_install = |name: &str| -> bool {
        match skill_name_filter {
            Some(filters) if !filters.is_empty() => filters.iter().any(|f| name == f || name.contains(f.as_str())),
            Some(_) => true, // 空列表 = 安装所有
            None => true,
        }
    };

    for (skill, src_dir) in all_skills {
        if !should_install(&skill.meta.name) {
            continue;
        }

        let dest_dir = target_dir.join(&skill.meta.name);

        // 检查冲突
        if dest_dir.exists() {
            warn!(
                name = %skill.meta.name,
                dest = %dest_dir.display(),
                "Skill already exists, overwriting"
            );
            fs::remove_dir_all(&dest_dir).map_err(|e| {
                AppError::Config(format!("Failed to remove existing skill {}: {}", skill.meta.name, e))
            })?;
        }

        // 复制目录
        copy_dir_all(&src_dir, &dest_dir).map_err(|e| {
            AppError::Config(format!("Failed to copy skill {}: {}", skill.meta.name, e))
        })?;

        // 写入元数据
        let meta = SkillMeta::new(source, &skill_source);
        let meta_json = serde_json::to_string_pretty(&meta)
            .map_err(|e| AppError::Config(format!("Failed to serialize skill meta: {}", e)))?;
        let meta_path = dest_dir.join(".skill-meta.json");
        fs::write(&meta_path, meta_json).map_err(|e| {
            AppError::Config(format!("Failed to write skill metadata: {}", e))
        })?;

        debug!(name = %skill.meta.name, scope = ?target_scope, "Installed skill");
        installed.push(skill);
    }

    // 清理临时目录
    if let Some((temp, _repo_path, _, _)) = temp_dir {
        git_utils::cleanup_temp_dir(&temp);
    }

    if installed.is_empty() {
        if let Some(filters) = skill_name_filter {
            return Err(AppError::Config(format!(
                "No skills matching {:?} found in source: {}",
                filters, source
            )));
        }
        return Err(AppError::Config(format!(
            "No valid skills (SKILL.md) found in source: {}",
            source
        )));
    }

    Ok(installed)
}

/// 移除已安装的技能。
pub fn remove_skill(name: &str, scope: InstallScope, working_dir: &Path) -> Result<(), AppError> {
    let dir = match scope {
        InstallScope::Global => global_skills_dir(),
        InstallScope::Project => project_skills_dir(working_dir),
    };
    let skill_dir = dir.join(name);

    if !skill_dir.exists() {
        return Err(AppError::Config(format!("Skill not found: {}", name)));
    }

    fs::remove_dir_all(&skill_dir).map_err(|e| {
        AppError::Config(format!("Failed to remove skill {}: {}", name, e))
    })?;

    info!(name = %name, scope = ?scope, "Removed skill");
    Ok(())
}

/// 更新指定范围的所有 Git 来源技能（本地来源跳过）。
///
/// 已安装目录不保留 git 仓库，因此每次重新浅克隆到临时目录，
/// 与已安装内容做增量对比，仅当内容有变更时替换并刷新元数据。
/// 返回本次实际更新的技能名称列表。
pub fn update_skills(scope: InstallScope, working_dir: &Path) -> Result<Vec<String>, AppError> {
    let dir = match scope {
        InstallScope::Global => global_skills_dir(),
        InstallScope::Project => project_skills_dir(working_dir),
    };

    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut updated = Vec::new();

    let entries = fs::read_dir(&dir).map_err(|e| {
        AppError::Config(format!("Failed to read skills directory {}: {}", dir.display(), e))
    })?;

    for entry in entries {
        let entry = entry?;
        let skill_dir = entry.path();
        if !skill_dir.is_dir() {
            continue;
        }

        let meta_path = skill_dir.join(".skill-meta.json");
        if !meta_path.exists() {
            continue; // 非安装的技能，跳过
        }

        let meta_str = fs::read_to_string(&meta_path)
            .map_err(|e| AppError::Config(format!("Failed to read meta: {}", e)))?;
        let meta: SkillMeta = serde_json::from_str(&meta_str)
            .map_err(|e| AppError::Config(format!("Failed to parse meta: {}", e)))?;

        let git_url = match meta.git_url {
            Some(ref url) if !url.is_empty() => url.clone(),
            _ => continue, // 本地来源或缺失来源信息，跳过
        };

        // 重新浅克隆到临时目录
        let temp = std::env::temp_dir().join(format!(
            "dev-assistant-skill-update-{}",
            uuid::Uuid::new_v4()
        ));
        if let Err(e) = fs::create_dir_all(&temp) {
            warn!(path = %temp.display(), error = %e, "Failed to create temp dir, skipping update");
            continue;
        }

        let result = (|| -> Result<Option<String>, AppError> {
            let repo_path = git_utils::clone_repo(&git_url, &temp)?;
            let _ = git_utils::checkout_branch(&repo_path, meta.git_branch.as_deref());

            // 在最新代码中查找同名技能目录
            let skill_name = skill_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let new_src = git_utils::list_skill_dirs(&repo_path, None)
                .into_iter()
                .find(|d| {
                    parse_skill_file(&d.join("SKILL.md"))
                        .map(|s| s.meta.name == skill_name)
                        .unwrap_or(false)
                });

            let new_src = match new_src {
                Some(d) => d,
                None => return Ok(None), // 上游已删除该技能，跳过
            };

            // 内容无变化则跳过（增量更新）
            if !dirs_differ(&new_src, &skill_dir) {
                return Ok(None);
            }

            // 替换已安装内容
            fs::remove_dir_all(&skill_dir).map_err(|e| {
                AppError::Config(format!("Failed to remove old skill {}: {}", skill_name, e))
            })?;
            copy_dir_all(&new_src, &skill_dir).map_err(|e| {
                AppError::Config(format!("Failed to update skill {}: {}", skill_name, e))
            })?;

            // 刷新元数据时间戳
            let mut new_meta = meta.clone();
            new_meta.installed_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let meta_json = serde_json::to_string_pretty(&new_meta)
                .map_err(|e| AppError::Config(format!("Failed to serialize skill meta: {}", e)))?;
            fs::write(&meta_path, meta_json).map_err(|e| {
                AppError::Config(format!("Failed to write skill metadata: {}", e))
            })?;

            info!(name = %skill_name, "Updated skill");
            Ok(Some(skill_name))
        })();

        git_utils::cleanup_temp_dir(&temp);

        match result {
            Ok(Some(name)) => updated.push(name),
            Ok(None) => {}
            Err(e) => warn!(error = %e, "Failed to update skill, skipping"),
        }
    }

    Ok(updated)
}

/// 列出指定范围的所有技能。
pub fn list_skills(scope: InstallScope, working_dir: &Path) -> Result<Vec<Skill>, AppError> {
    let dir = match scope {
        InstallScope::Global => global_skills_dir(),
        InstallScope::Project => project_skills_dir(working_dir),
    };
    discover_skills(&dir)
}

/// 解析来源字符串为 SkillSource。
fn parse_source(source: &str) -> SkillSource {
    // 本地路径
    if source.starts_with("./") || source.starts_with('/') || source.starts_with('~') {
        let path = shellexpand::full(source)
            .map(|s| PathBuf::from(s.into_owned()))
            .unwrap_or_else(|_| PathBuf::from(source));
        return SkillSource::Local { path };
    }

    // Git 来源
    let (url, branch, subdir) = git_utils::parse_git_source(source);
    SkillSource::Git {
        source: url,
        branch,
        subdir,
    }
}

/// 递归复制目录。
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let name = entry.file_name();

        // 跳过 .skill-meta.json 和目标自身
        if name == ".skill-meta.json" || name == ".git" {
            continue;
        }

        let src_path = entry.path();
        let dst_path = dst.join(name);

        if ty.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else if ty.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

/// 递归比较两个目录内容是否一致（忽略 .git 与 .skill-meta.json）。
fn dirs_differ(a: &Path, b: &Path) -> bool {
    fn collect(dir: &Path, base: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name == ".git" || name == ".skill-meta.json" {
                    continue;
                }
                let path = entry.path();
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    collect(&path, base, out);
                } else if let Ok(content) = fs::read(&path) {
                    out.push((
                        path.strip_prefix(base).unwrap_or(&path).to_path_buf(),
                        content,
                    ));
                }
            }
        }
    }

    let mut fa = Vec::new();
    let mut fb = Vec::new();
    collect(a, a, &mut fa);
    collect(b, b, &mut fb);

    if fa.len() != fb.len() {
        return true;
    }
    fa.sort_by(|x, y| x.0.cmp(&y.0));
    fb.sort_by(|x, y| x.0.cmp(&y.0));
    fa.iter().zip(fb.iter()).any(|(x, y)| x.0 != y.0 || x.1 != y.1)
}

/// 读取已安装技能的元数据。
pub fn read_skill_meta(skill_dir: &Path) -> Option<SkillMeta> {
    let meta_path = skill_dir.join(".skill-meta.json");
    match fs::read_to_string(&meta_path) {
        Ok(s) => serde_json::from_str(&s).ok(),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn install_local_skill() {
        let tmp = tempdir().unwrap();
        let skills_dir = tmp.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();

        // 创建一个本地技能目录
        let src_skill = tmp.path().join("source-skill");
        fs::create_dir_all(&src_skill).unwrap();
        fs::write(
            src_skill.join("SKILL.md"),
            "---\nname: test-skill\ndescription: A test skill\n---\n## Body\nDo things.",
        )
        .unwrap();

        // 安装到临时 skills 目录
        let skill_dir = tmp.path().join(".dev-assistant").join("skills");
        fs::create_dir_all(&skill_dir).unwrap();

        // 模拟安装：复制技能并写元数据
        let dest = skill_dir.join("test-skill");
        copy_dir_all(&src_skill, &dest).unwrap();

        let meta = SkillMeta::new("local", &SkillSource::Local { path: src_skill.clone() });
        let meta_json = serde_json::to_string_pretty(&meta).unwrap();
        fs::write(dest.join(".skill-meta.json"), meta_json).unwrap();

        // 验证安装结果
        assert!(dest.exists());
        assert!(dest.join("SKILL.md").exists());
        assert!(dest.join(".skill-meta.json").exists());
    }

    #[test]
    fn install_removes_existing_skill() {
        let tmp = tempdir().unwrap();
        let skills_dir = tmp.path().join(".dev-assistant").join("skills");
        fs::create_dir_all(&skills_dir).unwrap();

        // 创建一个旧技能
        let old_skill = skills_dir.join("test-skill");
        fs::create_dir_all(&old_skill).unwrap();
        fs::write(old_skill.join("SKILL.md"), "---\nname: test-skill\ndescription: Old\n---\nold body").unwrap();

        // 创建新技能源
        let src_skill = tmp.path().join("new-skill");
        fs::create_dir_all(&src_skill).unwrap();
        fs::write(
            src_skill.join("SKILL.md"),
            "---\nname: test-skill\ndescription: New version\n---\nnew body",
        )
        .unwrap();

        // 安装（应覆盖旧版本）
        copy_dir_all(&src_skill, &old_skill).unwrap();

        let content = fs::read_to_string(old_skill.join("SKILL.md")).unwrap();
        assert!(content.contains("New version"));
    }

    #[test]
    fn test_remove_skill() {
        let tmp = tempdir().unwrap();
        let skills_dir = tmp.path().join(".dev-assistant").join("skills");
        fs::create_dir_all(&skills_dir).unwrap();

        let skill_dir = skills_dir.join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: test-skill\ndescription: test\n---\nbody").unwrap();

        remove_skill("test-skill", InstallScope::Project, tmp.path()).unwrap();
        assert!(!skill_dir.exists());
    }

    #[test]
    fn test_remove_nonexistent_skill_errors() {
        let tmp = tempdir().unwrap();
        let result = remove_skill("nonexistent", InstallScope::Project, tmp.path());
        assert!(result.is_err());
    }
}
