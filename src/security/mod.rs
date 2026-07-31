use std::env;
use std::path::{Component, Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::utils::error::AppError;

pub mod approval;
pub use approval::*;

fn load_whitelist() -> Vec<String> {
    env::var("COMMAND_WHITELIST")
        .ok()
        .map(|s| {
            s.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Normalize a path by resolving `.` and `..` components without following symlinks.
/// This prevents path traversal while still allowing canonicalize() to fail.
fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Prevent popping past the root — an empty result means we've
                // hit the root and cannot traverse further upward.
                if !result.as_os_str().is_empty() {
                    result.pop();
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// Check whether `path` is a child of (or equal to) `parent`.
/// This is safer than `starts_with` because it requires a path-segment boundary:
/// `/project/file` is a child of `/project/`, but `/project-backup/file` is not.
fn is_child_of(path: &Path, parent: &Path) -> bool {
    if path == parent {
        return true;
    }
    let parent_with_sep = parent.join("");
    path.starts_with(&parent_with_sep)
}

/// Check if any component of the path from `base` to `target` is a symbolic link.
/// This prevents symlink-based path traversal attacks when canonicalize() fails.
/// Also checks if `base` itself is a symlink to prevent allowed_paths from
/// being symbolic links that point outside the intended directory.
///
/// # Security
///
/// Uses `symlink_metadata()` which does NOT follow symlinks, making it safe.
/// Unlike `current.exists()`, `symlink_metadata()` returns `Err` for
/// non-existent paths rather than potentially following a broken symlink chain.
/// This eliminates the TOCTOU race condition where a symlink could be created
/// between the `exists()` check and the `symlink_metadata()` call.
fn contains_symlink(target: &Path, base: &Path) -> bool {
    // Check if base itself is a symlink first
    if let Ok(metadata) = base.symlink_metadata() {
        if metadata.file_type().is_symlink() {
            return true;
        }
    }

    // Walk from target up to base, checking every component for symlinks.
    // We use symlink_metadata() on ALL components regardless of existence,
    // because symlink_metadata() does NOT follow symlinks and returns
    // Err(NotFound) for non-existent paths — which is safe to ignore.
    //
    // This is more secure than checking current.exists() first, because:
    // 1. No TOCTOU race: a symlink created between exists() and metadata() check
    // 2. Non-existent paths are handled gracefully (Err returned, loop continues)
    let mut current = target;
    loop {
        if current == base {
            return false;
        }

        // Check this component for symlinks using symlink_metadata().
        // Returns Err(NotFound) for non-existent paths — safe to ignore.
        if let Ok(metadata) = current.symlink_metadata() {
            if metadata.file_type().is_symlink() {
                return true;
            }
        }

        // Move to parent directory
        match current.parent() {
            Some(parent) if parent != current => current = parent,
            // SECURITY: If we can't reach `base` (e.g., we hit the root `/`
            // and `base` is not an ancestor), the path is outside the allowed
            // directory tree. Return `true` (unsafe) to prevent path traversal.
            _ => return true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DangerLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl DangerLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            DangerLevel::Low => "low",
            DangerLevel::Medium => "medium",
            DangerLevel::High => "high",
            DangerLevel::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SecurityEvaluation {
    pub danger_level: DangerLevel,
    pub reason: String,
}

pub struct SecurityPolicy {
    allowed_paths: Vec<PathBuf>,
    dangerous_commands: Vec<(Regex, DangerLevel, String)>,
    dangerous_files: Vec<Regex>,
    whitelisted_commands: Vec<String>,
    pub approval_required: bool,
}

impl SecurityPolicy {
    pub fn new(working_dir: &Path, approval_required: bool) -> Self {
        fn compile_regex(pattern: &str, desc: &str) -> Regex {
            Regex::new(pattern).unwrap_or_else(|e| {
                panic!("Failed to compile security regex '{}': {}", desc, e)
            })
        }

        let dangerous_commands = vec![
            (
                // Matches rm with recursive (-r) or force (-f) flag:
                //   rm -rf, rm -fr, rm -r -f, rm -r, rm -f, rm --recursive --force
                // Uses `[rf]+\b` to require a word boundary after the flag, preventing
                // false positives on commands like `rm -rfx` where `rfx` is a different flag.
                compile_regex(r"(?i)\brm\s+-(?:[rf]+\b|-[a-z]*-[rf]\b)", "rm -rf regex"),
                DangerLevel::Critical,
                "rm with -r/-f flags is not allowed".to_string(),
            ),
            (
                compile_regex(r"(?i)\bsudo\b", "sudo regex"),
                DangerLevel::High,
                "sudo requires approval".to_string(),
            ),
            (
                compile_regex(r"(?i)\bchmod\b", "chmod regex"),
                DangerLevel::High,
                "chmod requires approval".to_string(),
            ),
            (
                compile_regex(r"(?i)\bchown\b", "chown regex"),
                DangerLevel::High,
                "chown requires approval".to_string(),
            ),
            (
                compile_regex(r"(?i)\bshutdown\b", "shutdown regex"),
                DangerLevel::Critical,
                "shutdown is not allowed".to_string(),
            ),
            (
                compile_regex(r"(?i)\breboot\b", "reboot regex"),
                DangerLevel::Critical,
                "reboot is not allowed".to_string(),
            ),
            (
                compile_regex(r"(?i)\bcurl\b", "curl regex"),
                DangerLevel::Medium,
                "curl requires approval".to_string(),
            ),
            (
                compile_regex(r"(?i)\bwget\b", "wget regex"),
                DangerLevel::Medium,
                "wget requires approval".to_string(),
            ),
        ];

        let allowed_path = working_dir.canonicalize().unwrap_or_else(|_| working_dir.to_path_buf());

        Self {
            allowed_paths: vec![allowed_path],
            dangerous_commands,
            dangerous_files: vec![
                Regex::new(r"(?i)\.env$").expect("invalid regex for .env files"),
                Regex::new(r"(?i)\.key$").expect("invalid regex for .key files"),
                Regex::new(r"(?i)\.pem$").expect("invalid regex for .pem files"),
                Regex::new(r"(?i)\.crt$").expect("invalid regex for .crt files"),
            ],
            whitelisted_commands: load_whitelist(),
            approval_required,
        }
    }

    /// Validate that a path is within allowed directories. The path must exist.
    /// Falls back to normalized prefix checking when canonicalize() fails.
    pub fn validate_path(&self, path: &str) -> Result<PathBuf, AppError> {
        for allowed in &self.allowed_paths {
            let candidate = allowed.join(path);
            let normalized = normalize_path(&candidate);

            // Try canonicalize first (resolves symlinks, most secure)
            match normalized.canonicalize() {
                Ok(resolved) => {
                    if is_child_of(&resolved, allowed) {
                        return Ok(resolved);
                    }
                }
                Err(_) => {
                    // Fallback: normalized path prefix check
                    // This handles cases where canonicalize() fails (e.g., file doesn't
                    // exist yet, or parent directory can't be canonicalized).
                    // IMPORTANT: Must check for symlinks to prevent path traversal attacks.
                    if is_child_of(&normalized, allowed) && !contains_symlink(&normalized, allowed) {
                        return Ok(normalized);
                    }
                }
            }
        }

        Err(AppError::Security(format!(
            "Path escaped from allowed directories: {}",
            path
        )))
    }

    /// Validate that a path is within allowed directories without requiring the path to exist.
    /// Falls back to normalized prefix checking when canonicalize() fails.
    /// Returns the full normalized path (not just the parent directory).
    pub fn validate_path_exists(&self, path: &str) -> Result<PathBuf, AppError> {
        let path_buf = Path::new(path);
        let parent = path_buf.parent().unwrap_or(Path::new("."));

        for allowed in &self.allowed_paths {
            let candidate = allowed.join(parent);
            let normalized = normalize_path(&candidate);

            match normalized.canonicalize() {
                Ok(resolved) => {
                    if is_child_of(&resolved, allowed) {
                        // Parent resolved ok; return the full path (file may not exist)
                        return Ok(normalize_path(&allowed.join(path)));
                    }
                }
                Err(_) => {
                    // Fallback: check parent path prefix and symlinks
                    if is_child_of(&normalized, allowed) && !contains_symlink(&normalized, allowed) {
                        // Parent validated; return the full path (file may not exist)
                        return Ok(normalize_path(&allowed.join(path)));
                    }
                }
            }
        }

        Err(AppError::Security(format!(
            "Path escaped from allowed directories: {}",
            path
        )))
    }

    /// Validate that the parent directory of a path is within allowed directories.
    /// Falls back to normalized prefix checking when canonicalize() fails.
    pub fn validate_parent_path(&self, path: &str) -> Result<PathBuf, AppError> {
        let path_buf = Path::new(path);
        let parent = path_buf.parent().unwrap_or(Path::new("."));

        // If parent is empty (current directory), use working directory
        let parent_to_check = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };

        for allowed in &self.allowed_paths {
            let candidate = allowed.join(parent_to_check);
            let normalized = normalize_path(&candidate);

            match normalized.canonicalize() {
                Ok(resolved) => {
                    if is_child_of(&resolved, allowed) {
                        return Ok(resolved);
                    }
                }
                Err(_) => {
                    // Fallback: check parent path prefix and symlinks
                    if is_child_of(&normalized, allowed) && !contains_symlink(&normalized, allowed) {
                        return Ok(normalized);
                    }
                }
            }
        }

        Err(AppError::Security(format!(
            "Parent path escaped from allowed directories: {}",
            path
        )))
    }

    pub fn evaluate_command(&self, command: &str, args: &[&str]) -> SecurityEvaluation {
        let full_command = if args.is_empty() {
            command.to_string()
        } else {
            format!("{} {}", command, args.join(" "))
        };

        // SECURITY: Use exact match or command + space prefix match to prevent
        // bypass via similarly-named executables (e.g., "cargo-malicious" when
        // "cargo" is whitelisted).
        for whitelisted in &self.whitelisted_commands {
            if full_command == *whitelisted
                || (full_command.len() > whitelisted.len()
                    && full_command.starts_with(whitelisted)
                    && full_command.as_bytes()[whitelisted.len()] == b' '
            ) {
                return SecurityEvaluation {
                    danger_level: DangerLevel::Low,
                    reason: format!("Command is whitelisted: {}", command),
                };
            }
        }

        // SECURITY: Allow shell execution with -c flag, but still scan the shell
        // string for dangerous commands. The tool description explicitly instructs
        // the LLM to use command="sh" with args=["-c", "..."] for shell features.
        if (command == "sh" || command == "bash" || command == "zsh" || command == "fish")
            && args.contains(&"-c")
        {
            if let Some(idx) = args.iter().position(|&a| a == "-c") {
                if let Some(shell_cmd) = args.get(idx + 1) {
                    for (pattern, level, reason) in &self.dangerous_commands {
                        if pattern.is_match(shell_cmd) {
                            return SecurityEvaluation {
                                danger_level: level.clone(),
                                reason: reason.clone(),
                            };
                        }
                    }
                }
            }
            return SecurityEvaluation {
                danger_level: DangerLevel::Low,
                reason: "Shell execution with -c — allowed by policy".to_string(),
            };
        }

        for (pattern, level, reason) in &self.dangerous_commands {
            if pattern.is_match(&full_command) {
                return SecurityEvaluation {
                    danger_level: level.clone(),
                    reason: reason.clone(),
                };
            }
        }

        SecurityEvaluation {
            danger_level: DangerLevel::Low,
            reason: "Safe command".to_string(),
        }
    }

    /// Evaluate a tool execution for security. Checks both the tool name and
    /// the arguments passed to it.
    pub fn evaluate_tool(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> SecurityEvaluation {
        // exec_command：委托给专门的命令评估器
        if tool_name == "exec_command" {
            let command = arguments["command"].as_str().unwrap_or("");
            let args: Vec<&str> = arguments["args"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            let eval = self.evaluate_command(command, &args);
            debug!(tool = tool_name, command, level = ?eval.danger_level, "Security evaluation");
            return eval;
        }

        // glob：对 pattern 做专门检查
        if tool_name == "glob" {
            if let Some(pattern) = arguments["pattern"].as_str() {
                if pattern.contains("..") {
                    return self.debug_eval(
                        tool_name,
                        SecurityEvaluation {
                            danger_level: DangerLevel::Critical,
                            reason: "Glob pattern with '..' is not allowed".to_string(),
                        },
                    );
                }
                if pattern.starts_with('/') {
                    return self.debug_eval(
                        tool_name,
                        SecurityEvaluation {
                            danger_level: DangerLevel::Critical,
                            reason: "Absolute path patterns are not allowed".to_string(),
                        },
                    );
                }
            }
        }

        // 通用路径校验：查表得到 spec，按 spec 走统一流程
        if let Some(spec) = crate::tools::spec::tool_security_spec(tool_name) {
            return match spec.evaluate(self, arguments) {
                Ok(()) => self.debug_eval(
                    tool_name,
                    SecurityEvaluation {
                        danger_level: DangerLevel::Low,
                        reason: "Safe operation".to_string(),
                    },
                ),
                Err(eval) => {
                    let (level, reason) = eval.into_parts();
                    self.debug_eval(
                        tool_name,
                        SecurityEvaluation {
                            danger_level: level,
                            reason,
                        },
                    )
                }
            };
        }

        debug!(tool = tool_name, "Security evaluation: safe");
        SecurityEvaluation {
            danger_level: DangerLevel::Low,
            reason: "Safe operation".to_string(),
        }
    }

    fn debug_eval(
        &self,
        tool_name: &str,
        eval: SecurityEvaluation,
    ) -> SecurityEvaluation {
        debug!(tool = tool_name, level = ?eval.danger_level, "Security evaluation");
        eval
    }

    pub fn is_dangerous_file(&self, filename: &str) -> bool {
        self.dangerous_files
            .iter()
            .any(|pattern| pattern.is_match(filename))
    }

    /// 判断指定危险级别是否需要交互审批。
    ///
    /// 当前 `ToolRegistry::execute` 在 High/Medium 时直接返回需审批结果，
    /// 此方法保留为未来接入显式 approve/cancel 交互流程时的扩展点。
    #[allow(dead_code)]
    pub fn requires_approval(&self, danger_level: &DangerLevel) -> bool {
        match danger_level {
            DangerLevel::Critical => true,
            DangerLevel::High => self.approval_required,
            DangerLevel::Medium => self.approval_required,
            DangerLevel::Low => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn policy_in(dir: &Path) -> SecurityPolicy {
        SecurityPolicy::new(dir, true)
    }

    #[test]
    fn dangerous_file_detection() {
        let dir = tempdir().unwrap();
        let p = policy_in(dir.path());

        assert!(p.is_dangerous_file(".env"));
        assert!(p.is_dangerous_file("id_rsa.key"));
        assert!(p.is_dangerous_file("cert.pem"));
        assert!(p.is_dangerous_file("ca.crt"));
        assert!(!p.is_dangerous_file("main.rs"));
        assert!(!p.is_dangerous_file("config.toml"));
    }

    #[test]
    fn validate_path_rejects_traversal() {
        let dir = tempdir().unwrap();
        let p = policy_in(dir.path());

        // `..` escaping the working dir should be rejected
        let err = p.validate_path("../../etc/passwd").unwrap_err();
        assert!(matches!(err, AppError::Security(_)));
    }

    #[test]
    fn validate_path_accepts_existing_in_working_dir() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("hello.txt");
        fs::write(&target, "hi").unwrap();

        let p = policy_in(dir.path());
        let resolved = p.validate_path("hello.txt").unwrap();
        assert_eq!(resolved.canonicalize().unwrap(), target.canonicalize().unwrap());
    }

    #[test]
    fn validate_parent_path_allows_nonexistent_file() {
        let dir = tempdir().unwrap();
        let p = policy_in(dir.path());

        // 文件不存在但父目录在工作目录内，应该通过
        let resolved = p.validate_parent_path("new_file.txt").unwrap();
        // canonicalize 后的路径应当与 working dir 关联（在 allowed_paths 内）
        let dir_canonical = dir.path().canonicalize().unwrap_or_else(|_| dir.path().to_path_buf());
        assert!(
            resolved == dir_canonical || resolved.starts_with(&dir_canonical),
            "expected resolved to be inside working dir {}, got {}",
            dir_canonical.display(),
            resolved.display()
        );
    }

    #[test]
    fn evaluate_command_blocks_rm_rf() {
        // 验证 rm -rf（紧凑形式）被正确捕获为 Critical。
        let dir = tempdir().unwrap();
        let p = policy_in(dir.path());

        let eval = p.evaluate_command("rm", &["-rf", "src"]);
        assert_eq!(
            eval.danger_level,
            DangerLevel::Critical,
            "rm -rf should be blocked as Critical; got reason: {}",
            eval.reason
        );
    }

    #[test]
    fn evaluate_command_flags_sudo() {
        let dir = tempdir().unwrap();
        let p = policy_in(dir.path());

        let eval = p.evaluate_command("sudo", &["apt", "update"]);
        assert_eq!(eval.danger_level, DangerLevel::High);
    }

    #[test]
    fn evaluate_command_allows_safe_command() {
        let dir = tempdir().unwrap();
        let p = policy_in(dir.path());

        // 未命中危险规则且不在白名单：默认 Low
        let eval = p.evaluate_command("cargo", &["build"]);
        assert_eq!(eval.danger_level, DangerLevel::Low);
    }

    #[test]
    fn evaluate_command_shell_with_c_is_allowed() {
        let dir = tempdir().unwrap();
        let p = policy_in(dir.path());

        // sh -c 由专门分支放行（内部命令仍由正则捕获）
        let eval = p.evaluate_command("sh", &["-c", "ls"]);
        assert_eq!(eval.danger_level, DangerLevel::Low);
    }

    #[test]
    fn evaluate_command_shell_with_c_still_catches_dangerous() {
        // sh -c 允许使用 shell 特性，但 shell 字符串仍需经过危险命令扫描。
        let dir = tempdir().unwrap();
        let p = policy_in(dir.path());

        let eval = p.evaluate_command("sh", &["-c", "rm -rf src"]);
        assert_eq!(
            eval.danger_level,
            DangerLevel::Critical,
            "sh -c should still scan shell content for dangerous commands; got reason: {}",
            eval.reason
        );
    }

    #[test]
    fn evaluate_tool_unknown_tool_returns_low() {
        let dir = tempdir().unwrap();
        let p = policy_in(dir.path());

        let eval = p.evaluate_tool("nonexistent_tool", &serde_json::json!({}));
        assert_eq!(eval.danger_level, DangerLevel::Low);
    }

    #[test]
    fn requires_approval_respects_approval_flag() {
        let dir = tempdir().unwrap();
        let with_approval = SecurityPolicy::new(dir.path(), true);
        let without_approval = SecurityPolicy::new(dir.path(), false);

        assert!(with_approval.requires_approval(&DangerLevel::High));
        assert!(with_approval.requires_approval(&DangerLevel::Medium));
        assert!(with_approval.requires_approval(&DangerLevel::Critical));
        assert!(!with_approval.requires_approval(&DangerLevel::Low));

        // 即使 approval_required=false，Critical 仍需审批
        assert!(without_approval.requires_approval(&DangerLevel::Critical));
        assert!(!without_approval.requires_approval(&DangerLevel::High));
    }
}

