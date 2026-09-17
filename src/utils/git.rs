//! Git 辅助函数：clone、fetch 等操作（基于系统 git 命令）。

use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, warn};

use crate::utils::error::AppError;

/// 验证分支名称是否安全。
///
/// Git 分支名不能包含空格、~、^、:、?、*、[、\，且不能以 . 或 - 开头。
fn validate_branch_name(branch: &str) -> Result<(), AppError> {
    if branch.is_empty() {
        return Err(AppError::Config("Branch name is empty".to_string()));
    }

    // 检查危险字符
    let dangerous_chars = [' ', '~', '^', ':', '?', '*', '[', '\\', '\n', '\r', '\0'];
    if let Some(c) = branch.chars().find(|c| dangerous_chars.contains(c)) {
        return Err(AppError::Config(format!(
            "Branch name contains invalid character '{}': {}",
            c, branch
        )));
    }

    // 不能以 . 或 - 开头
    if branch.starts_with('.') || branch.starts_with('-') {
        return Err(AppError::Config(format!(
            "Branch name cannot start with '.' or '-': {}",
            branch
        )));
    }

    // 不能包含 ..
    if branch.contains("..") {
        return Err(AppError::Config(format!(
            "Branch name cannot contain '..': {}",
            branch
        )));
    }

    // 不能以 .lock 结尾
    if branch.ends_with(".lock") {
        return Err(AppError::Config(format!(
            "Branch name cannot end with '.lock': {}",
            branch
        )));
    }

    Ok(())
}

/// 验证 URL 是否安全。
///
/// 检查 URL 是否包含 shell 注入字符。
fn validate_url(url: &str) -> Result<(), AppError> {
    // 检查危险字符（shell 注入）
    let dangerous_chars = ['|', '&', ';', '$', '`', '\n', '\r', '(', ')', '{', '}', '<', '>'];
    if let Some(c) = url.chars().find(|c| dangerous_chars.contains(c)) {
        return Err(AppError::Config(format!(
            "URL contains invalid character '{}': {}",
            c, url
        )));
    }

    // URL 必须以有效协议开头
    let valid_prefixes = ["https://", "http://", "git@", "ssh://", "git://"];
    if !valid_prefixes.iter().any(|p| url.starts_with(p)) {
        return Err(AppError::Config(format!(
            "URL must start with one of {:?}: {}",
            valid_prefixes, url
        )));
    }

    Ok(())
}

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

    // 验证 URL 安全性
    validate_url(&url)?;

    // 验证分支名安全性
    if let Some(ref b) = branch {
        validate_branch_name(b)?;
    }

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
    // 注意：不在日志中输出凭据
    if let Ok(user) = std::env::var("GIT_USERNAME") {
        if let Ok(pass) = std::env::var("GIT_PASSWORD") {
            cmd.env("GIT_USERNAME", &user);
            cmd.env("GIT_PASSWORD", &pass);
            debug!("Git credentials set from environment (not logged for security)");
        }
    }

    let output = cmd.output().map_err(|e| {
        AppError::Config(format!("git clone failed: {}", e))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // 过滤掉可能包含凭据的错误信息
        let safe_stderr = sanitize_error_message(&stderr);
        return Err(AppError::Config(format!(
            "Failed to clone {}: {}\n{}",
            source, output.status, safe_stderr
        )));
    }

    Ok(target_dir.to_path_buf())
}

/// 清理错误消息中的敏感信息。
fn sanitize_error_message(msg: &str) -> String {
    // 移除可能包含的凭据
    let sanitized = msg
        .lines()
        .filter(|line| {
            !line.contains("password")
                && !line.contains("token")
                && !line.contains("credential")
        })
        .collect::<Vec<_>>()
        .join("\n");
    sanitized
}

/// 切换已克隆仓库到指定分支。
pub fn checkout_branch(repo_path: &Path, branch: Option<&str>) -> Result<(), AppError> {
    let branch = match branch {
        Some(b) if !b.is_empty() => b,
        _ => return Ok(()),
    };

    // 验证分支名安全性
    validate_branch_name(branch)?;

    // 尝试 checkout 远程分支
    let output = Command::new("git")
        .args(["checkout", "-b", branch, &format!("origin/{}", branch)])
        .current_dir(repo_path)
        .output();

    match output {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            // 分支不存在，尝试切换到默认分支
            let stderr = String::from_utf8_lossy(&out.stderr);
            warn!(branch = %branch, stderr = %stderr, "Branch not found, trying to find remote branches");
            let branches_out = Command::new("git")
                .args(["branch", "-r"])
                .current_dir(repo_path)
                .output();
            if let Ok(branches) = branches_out {
                let branch_list = String::from_utf8_lossy(&branches.stdout);
                if branch_list.contains(branch) {
                    let result = Command::new("git")
                        .args(["checkout", branch])
                        .current_dir(repo_path)
                        .output();
                    match result {
                        Ok(out) if out.status.success() => Ok(()),
                        Ok(out) => {
                            let stderr = String::from_utf8_lossy(&out.stderr);
                            Err(AppError::Config(format!(
                                "Failed to checkout branch '{}': {}",
                                branch, stderr
                            )))
                        }
                        Err(e) => Err(AppError::Config(format!(
                            "Failed to execute git checkout: {}",
                            e
                        ))),
                    }
                } else {
                    Err(AppError::Config(format!(
                        "Branch '{}' not found in remote branches",
                        branch
                    )))
                }
            } else {
                Err(AppError::Config(
                    "Failed to list remote branches".to_string(),
                ))
            }
        }
        Err(e) => Err(AppError::Config(format!(
            "Failed to execute git checkout: {}",
            e
        ))),
    }
}

/// 获取仓库中所有包含 SKILL.md 的子目录路径。
pub fn list_skill_dirs(repo_path: &Path, subdir: Option<&str>) -> Vec<PathBuf> {
    let search_root = match subdir {
        Some(s) if !s.is_empty() => {
            let combined = repo_path.join(s);
            if combined.exists() {
                combined
            } else {
                repo_path.to_path_buf()
            }
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

    // 验证分支名安全性
    if let Some(b) = branch {
        validate_branch_name(b)?;
    }

    debug!(path = %repo_path.display(), "Fetching skill repo updates");

    let fetch_output = Command::new("git")
        .args(["fetch", "origin"])
        .current_dir(repo_path)
        .output();

    let fetch_ok = match fetch_output {
        Ok(o) => {
            if !o.status.success() {
                let stderr = String::from_utf8_lossy(&o.stderr);
                warn!(stderr = %stderr, "git fetch failed");
                return Ok(false);
            }
            true
        }
        Err(e) => {
            warn!(error = %e, "Failed to execute git fetch");
            return Ok(false);
        }
    };

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
        let reset_output = Command::new("git")
            .args(["reset", "--hard", &target])
            .current_dir(repo_path)
            .output();

        match reset_output {
            Ok(o) => {
                if !o.status.success() {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    warn!(stderr = %stderr, "git reset failed");
                    return Ok(false);
                }
                Ok(true)
            }
            Err(e) => {
                warn!(error = %e, "Failed to execute git reset");
                Ok(false)
            }
        }
    } else {
        Ok(false)
    }
}

/// 安全地清理临时目录。
pub fn cleanup_temp_dir(dir: &Path) {
    if dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(dir) {
            warn!(path = %dir.display(), error = %e, "Failed to cleanup temp dir");
        }
    }
}
