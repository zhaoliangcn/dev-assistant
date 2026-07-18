use std::env;
use std::path::{Component, Path, PathBuf};

use regex::Regex;
use tracing::debug;

use crate::utils::error::AppError;

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
fn contains_symlink(target: &Path, base: &Path) -> bool {
    // Check if base itself is a symlink first
    if let Ok(metadata) = base.symlink_metadata() {
        if metadata.file_type().is_symlink() {
            return true;
        }
    }

    // If target doesn't exist, check all existing ancestor components
    // up to base to detect symlinks in the path.
    let mut current = target;
    loop {
        if current == base {
            return false;
        }

        // Check existing path components for symlinks
        if current.exists() {
            if let Ok(metadata) = current.symlink_metadata() {
                if metadata.file_type().is_symlink() {
                    return true;
                }
            }
        }

        // Move to parent directory
        match current.parent() {
            Some(parent) if parent != current => current = parent,
            _ => return false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
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
        let dangerous_commands = vec![
            (
                // Matches rm with force (-f) flag, optionally combined with recursive (-r):
                //   rm -rf, rm -r -f, rm -fr, rm --recursive --force
                Regex::new(r"(?i)\brm\s+-(?:r\w*\s+)?f\w*").expect("invalid regex for rm -rf"),
                DangerLevel::Critical,
                "rm -rf is not allowed".to_string(),
            ),
            (
                Regex::new(r"(?i)\bsudo\b").expect("invalid regex for sudo"),
                DangerLevel::High,
                "sudo requires approval".to_string(),
            ),
            (
                Regex::new(r"(?i)\bchmod\b").expect("invalid regex for chmod"),
                DangerLevel::High,
                "chmod requires approval".to_string(),
            ),
            (
                Regex::new(r"(?i)\bchown\b").expect("invalid regex for chown"),
                DangerLevel::High,
                "chown requires approval".to_string(),
            ),
            (
                Regex::new(r"(?i)\bshutdown\b").expect("invalid regex for shutdown"),
                DangerLevel::Critical,
                "shutdown is not allowed".to_string(),
            ),
            (
                Regex::new(r"(?i)\breboot\b").expect("invalid regex for reboot"),
                DangerLevel::Critical,
                "reboot is not allowed".to_string(),
            ),
            (
                Regex::new(r"(?i)\bcurl\b").expect("invalid regex for curl"),
                DangerLevel::Medium,
                "curl requires approval".to_string(),
            ),
            (
                Regex::new(r"(?i)\bwget\b").expect("invalid regex for wget"),
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
                    // IMPORTANT: Also check for symlinks to prevent path traversal attacks.
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

        // SECURITY: Block shell execution with -c flag to prevent bypassing
        // file path validation and other security checks via shell syntax.
        // Users should use direct command execution instead of shell pipelines.
        if (command == "sh" || command == "bash" || command == "zsh" || command == "fish")
            && args.contains(&"-c") {
            return SecurityEvaluation {
                danger_level: DangerLevel::High,
                reason: format!(
                    "Shell execution with -c is restricted. Use direct command execution instead. \
                     If shell features are truly needed, request approval with justification."
                ),
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
        // For exec_command, check the actual command + args
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

        // For file tools that read existing files, the file must exist within allowed directories
        if matches!(tool_name, "read_file" | "edit_file") {
            if let Some(file_path) = arguments["file_path"].as_str() {
                let file_name = std::path::Path::new(file_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(file_path);

                if self.is_dangerous_file(file_name) {
                    let eval = SecurityEvaluation {
                        danger_level: DangerLevel::High,
                        reason: format!(
                            "Access to sensitive file '{}' requires approval",
                            file_path
                        ),
                    };
                    debug!(tool = tool_name, file = file_path, level = ?eval.danger_level, "Security evaluation");
                    return eval;
                }

                if let Err(e) = self.validate_path(file_path) {
                    let eval = SecurityEvaluation {
                        danger_level: DangerLevel::Critical,
                        reason: e.to_string(),
                    };
                    debug!(tool = tool_name, file = file_path, level = ?eval.danger_level, "Security evaluation");
                    return eval;
                }
            }
        }

        // For file tools that check existence or list directories, validate the path scope
        // without requiring the target to exist
        if matches!(tool_name, "file_exists" | "list_directory") {
            if let Some(file_path) = arguments["file_path"].as_str() {
                let file_name = std::path::Path::new(file_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(file_path);

                if self.is_dangerous_file(file_name) {
                    let eval = SecurityEvaluation {
                        danger_level: DangerLevel::High,
                        reason: format!(
                            "Access to sensitive file '{}' requires approval",
                            file_path
                        ),
                    };
                    debug!(tool = tool_name, file = file_path, level = ?eval.danger_level, "Security evaluation");
                    return eval;
                }

                if let Err(e) = self.validate_path_exists(file_path) {
                    let eval = SecurityEvaluation {
                        danger_level: DangerLevel::Critical,
                        reason: e.to_string(),
                    };
                    debug!(tool = tool_name, file = file_path, level = ?eval.danger_level, "Security evaluation");
                    return eval;
                }
            }
        }

        // For write_file, the file may not exist yet — validate parent directory instead
        if tool_name == "write_file" {
            if let Some(file_path) = arguments["file_path"].as_str() {
                let file_name = std::path::Path::new(file_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(file_path);

                if self.is_dangerous_file(file_name) {
                    let eval = SecurityEvaluation {
                        danger_level: DangerLevel::High,
                        reason: format!(
                            "Access to sensitive file '{}' requires approval",
                            file_path
                        ),
                    };
                    debug!(tool = tool_name, file = file_path, level = ?eval.danger_level, "Security evaluation");
                    return eval;
                }

                if let Err(e) = self.validate_parent_path(file_path) {
                    let eval = SecurityEvaluation {
                        danger_level: DangerLevel::Critical,
                        reason: e.to_string(),
                    };
                    debug!(tool = tool_name, file = file_path, level = ?eval.danger_level, "Security evaluation");
                    return eval;
                }
            }
        }

        // For batch_read_files, validate each file path exists within allowed directories
        if tool_name == "batch_read_files" {
            if let Some(files) = arguments["files"].as_array() {
                for file in files {
                    if let Some(file_path) = file.as_str() {
                        let file_name = std::path::Path::new(file_path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(file_path);

                        if self.is_dangerous_file(file_name) {
                            let eval = SecurityEvaluation {
                                danger_level: DangerLevel::High,
                                reason: format!(
                                    "Access to sensitive file '{}' requires approval",
                                    file_path
                                ),
                            };
                            debug!(tool = tool_name, file = file_path, level = ?eval.danger_level, "Security evaluation");
                            return eval;
                        }

                        if let Err(e) = self.validate_path_exists(file_path) {
                            let eval = SecurityEvaluation {
                                danger_level: DangerLevel::Critical,
                                reason: e.to_string(),
                            };
                            debug!(tool = tool_name, file = file_path, level = ?eval.danger_level, "Security evaluation");
                            return eval;
                        }
                    }
                }
            }
        }

        // For glob, validate the pattern doesn't escape allowed directories
        if tool_name == "glob" {
            if let Some(pattern) = arguments["pattern"].as_str() {
                // Reject patterns with parent directory traversal that could escape
                if pattern.contains("..") {
                    let eval = SecurityEvaluation {
                        danger_level: DangerLevel::Critical,
                        reason: "Glob pattern with '..' is not allowed".to_string(),
                    };
                    debug!(tool = tool_name, pattern, level = ?eval.danger_level, "Security evaluation");
                    return eval;
                }
                // Reject absolute paths
                if pattern.starts_with('/') {
                    let eval = SecurityEvaluation {
                        danger_level: DangerLevel::Critical,
                        reason: "Absolute path patterns are not allowed".to_string(),
                    };
                    debug!(tool = tool_name, pattern, level = ?eval.danger_level, "Security evaluation");
                    return eval;
                }
            }
        }

        debug!(tool = tool_name, "Security evaluation: safe");
        SecurityEvaluation {
            danger_level: DangerLevel::Low,
            reason: "Safe operation".to_string(),
        }
    }

    pub fn is_dangerous_file(&self, filename: &str) -> bool {
        self.dangerous_files
            .iter()
            .any(|pattern| pattern.is_match(filename))
    }

    pub fn requires_approval(&self, danger_level: &DangerLevel) -> bool {
        match danger_level {
            DangerLevel::Critical => true,
            DangerLevel::High => self.approval_required,
            DangerLevel::Medium => self.approval_required,
            DangerLevel::Low => false,
        }
    }
}
