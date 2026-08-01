//! Git 辅助函数：clone、fetch 等操作（基于系统 git 命令）。

use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, warn};

use crate::utils::error::AppError;

/// 将 `owner/repo` 短名展开为 GitHub HTTPS URL。
///
/// - `owner/repo` → `https://github.com/owner/repo.git`
/// - 已包含 `://` 的字符串原样返回
/// - SSH 格式（`git@` 开头）原样返回
pub fn resolve_git_source(source: &str) -> String {
    if source.contains("://") || source.starts_with("git@") {
        return source.to_string();
    }
    if source.contains('/') && !source.contains(':') {
        return format!("https://github.com/{}.git", source);
    }
    source.to_string()
}

/// 解析 `owner/repo:branch/path` 或 `owner/repo@branch/path` 格式。
/// 返回 (base_url, branch, subdir)。
///
/// 分支/子路径语法只对短名（`owner/repo`）生效；
/// 完整 URL（含 `://`）、SSH 地址（`git@`）原样返回，不做拆分。
pub fn parse_git_source(source: &str) -> (String, Option<String>, Option<String>) {
    // SSH 地址（git@github.com:user/repo.git）原样处理
    if source.starts_with("git@") {
        return (source.to_string(), None, None);
    }

    // 仅对短名支持 :branch/path 或 @branch/path 语法
    if !source.contains("://") {
        // owner/repo:branch/path
        if let Some(idx) = source.find(':') {
            let before = &source[..idx];
            let after = &source[idx + 1..];
            let parts: Vec<&str> = after.splitn(2, '/').collect();
            let branch = if parts[0].is_empty() {
                None
            } else {
                Some(parts[0].to_string())
            };
            let subdir = if parts.len() > 1 && !parts[1].is_empty() {
                Some(parts[1].to_string())
            } else {
                None
            };
            return (resolve_git_source(before), branch, subdir);
        }
        // owner/repo@branch/path
        if let Some(idx) = source.find('@') {
            let before = &source[..idx];
            let after = &source[idx + 1..];
            let parts: Vec<&str> = after.splitn(2, '/').collect();
            let branch = if parts[0].is_empty() {
                None
            } else {
                Some(parts[0].to_string())
            };
            let subdir = if parts.len() > 1 && !parts[1].is_empty() {
                Some(parts[1].to_string())
            } else {
                None
            };
            return (resolve_git_source(before), branch, subdir);
        }
    }

    (resolve_git_source(source), None, None)
}

/// 克隆 Git 仓库到目标目录，使用浅克隆（depth=1）。
/// 使用系统 git 命令，需要确保 git 已安装。
pub fn clone_repo(source: &str, target_dir: &Path) -> Result<PathBuf, AppError> {
    let (url, branch, _subdir) = parse_git_source(source);

    debug!(url = %url, branch = ?branch, "Cloning git repo for skill install");

    // 确保目标目录存在
    std::fs::create_dir_all(target_dir).map_err(|e| {
        AppError::Config(format!("Failed to create target dir: {}", e))
    })?;

    let mut cmd = Command::new("git");
    cmd.args(["clone", "--depth", "1"])
        .arg(&url)
        .arg(target_dir)
        .current_dir(target_dir.parent().unwrap_or(target_dir));

    // 传递 GIT_USERNAME/GIT_PASSWORD 环境变量（如有）
    if let Ok(user) = std::env::var("GIT_USERNAME") {
        if let Ok(pass) = std::env::var("GIT_PASSWORD") {
            cmd.env("GIT_USERNAME", &user);
            cmd.env("GIT_PASSWORD", &pass);
        }
    }

    let output = cmd.output().map_err(|e| {
        AppError::Config(format!("git clone failed: {}", e))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Config(format!(
            "Failed to clone {}: {}\n{}",
            source, output.status, stderr
        )));
    }

    Ok(target_dir.to_path_buf())
}

/// 切换已克隆仓库到指定分支。
pub fn checkout_branch(repo_path: &Path, branch: Option<&str>) -> Result<(), AppError> {
    let branch = match branch {
        Some(b) if !b.is_empty() => b,
        _ => return Ok(()),
    };

    // 尝试 checkout 远程分支
    let output = Command::new("git")
        .args(["checkout", "-b", branch, &format!("origin/{}", branch)])
        .current_dir(repo_path)
        .output();

    match output {
        Ok(out) if out.status.success() => Ok(()),
        Ok(_out) => {
            // 分支不存在，尝试切换到默认分支
            warn!(branch = %branch, "Branch {} not found, trying to find remote branches", branch);
            let branches_out = Command::new("git")
                .args(["branch", "-r"])
                .current_dir(repo_path)
                .output();
            if let Ok(branches) = branches_out {
                let branch_list = String::from_utf8_lossy(&branches.stdout);
                if branch_list.contains(branch) {
                    let _ = Command::new("git")
                        .args(["checkout", branch])
                        .current_dir(repo_path)
                        .output();
                }
            }
            Ok(())
        }
        Err(e) => {
            warn!(error = %e, "Failed to checkout branch");
            Ok(())
        }
    }
}

/// 获取仓库中所有包含 SKILL.md 的子目录路径。
pub fn list_skill_dirs(repo_path: &Path, subdir: Option<&str>) -> Vec<PathBuf> {
    let search_root = match subdir {
        Some(s) if !s.is_empty() => {
            let combined = repo_path.join(s);
            if combined.exists() { combined } else { repo_path.to_path_buf() }
        }
        _ => repo_path.to_path_buf(),
    };

    if !search_root.is_dir() {
        return Vec::new();
    }

    match std::fs::read_dir(&search_root) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter(|e| e.path().join("SKILL.md").exists())
            .map(|e| e.path())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// 更新已克隆的仓库（git fetch + reset --hard）。
/// 仅在 `repo_path` 是有效 git 仓库时执行更新。
#[allow(dead_code)]
pub fn update_repo(repo_path: &Path, branch: Option<&str>) -> Result<bool, AppError> {
    // 检查是否是 git 仓库
    let is_git = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(repo_path)
        .output()
        .map(|o| o.status.success());

    if !is_git.unwrap_or(false) {
        return Ok(false);
    }

    debug!(path = %repo_path.display(), "Fetching skill repo updates");

    let fetch_ok = Command::new("git")
        .args(["fetch", "origin"])
        .current_dir(repo_path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !fetch_ok {
        return Ok(false);
    }

    let target = match branch {
        Some(b) => format!("origin/{}", b),
        None => "origin/HEAD".to_string(),
    };

    // 检查是否有新 commit
    let diff_output = Command::new("git")
        .args(["log", "--oneline", &format!("HEAD..{}", target)])
        .current_dir(repo_path)
        .output();

    let has_updates = match diff_output {
        Ok(out) if out.status.success() => {
            let log = String::from_utf8_lossy(&out.stdout);
            !log.trim().is_empty()
        }
        _ => false,
    };

    if has_updates {
        let reset_ok = Command::new("git")
            .args(["reset", "--hard", &target])
            .current_dir(repo_path)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        return Ok(reset_ok);
    }

    Ok(false)
}

/// 安全地清理临时目录。
pub fn cleanup_temp_dir(dir: &Path) {
    if dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(dir) {
            warn!(path = %dir.display(), error = %e, "Failed to cleanup temp dir");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_git_source_short_name() {
        assert_eq!(resolve_git_source("vercel-labs/agent-skills"),
            "https://github.com/vercel-labs/agent-skills.git");
    }

    #[test]
    fn test_resolve_git_source_full_url() {
        let url = "https://github.com/user/skills.git";
        assert_eq!(resolve_git_source(url), url);
    }

    #[test]
    fn test_resolve_git_source_ssh() {
        let ssh = "git@github.com:user/skills.git";
        assert_eq!(resolve_git_source(ssh), ssh);
    }

    #[test]
    fn test_parse_git_source_with_branch() {
        let (base, branch, subdir) = parse_git_source("owner/repo:feature/path");
        assert_eq!(base, "https://github.com/owner/repo.git");
        assert_eq!(branch, Some("feature".to_string()));
        assert_eq!(subdir, Some("path".to_string()));
    }

    #[test]
    fn test_parse_git_source_no_branch() {
        let (base, branch, subdir) = parse_git_source("owner/repo");
        assert_eq!(base, "https://github.com/owner/repo.git");
        assert!(branch.is_none());
        assert!(subdir.is_none());
    }

    #[test]
    fn test_parse_git_source_with_at() {
        let (base, branch, subdir) = parse_git_source("owner/repo@v2/skills");
        assert_eq!(base, "https://github.com/owner/repo.git");
        assert_eq!(branch, Some("v2".to_string()));
        assert_eq!(subdir, Some("skills".to_string()));
    }
}
