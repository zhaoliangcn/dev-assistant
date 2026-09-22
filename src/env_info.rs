//! 运行环境探测。
//!
//! 在进程启动时收集一次操作系统与 Shell 信息，注入系统提示词，
//! 让 LLM 知道该用什么命令语法（如 Windows 下 bash 用 POSIX 路径、cmd.exe 不支持 `head` 等）。

use std::env;
use std::process::Command;

/// 探测到的运行环境信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvInfo {
    /// 操作系统，如 `windows`、`macos`、`linux`
    pub os: &'static str,
    /// CPU 架构，如 `x86_64`、`aarch64`
    pub arch: &'static str,
    /// Shell 名称与语法提示，如 `bash`、`cmd.exe`、`powershell`、`/bin/zsh`
    pub shell: String,
    /// 针对当前 Shell 的可操作语法指引
    pub shell_guidance: String,
}

impl EnvInfo {
    /// 启动时探测一次运行环境。
    pub fn detect() -> Self {
        let os = env::consts::OS;
        let arch = env::consts::ARCH;
        let (shell, shell_guidance) = detect_shell();

        Self {
            os,
            arch,
            shell,
            shell_guidance,
        }
    }

    /// 渲染为系统提示词中的"运行环境"段落（含标题）。
    pub fn render_for_prompt(&self) -> String {
        format!(
            "## 运行环境\n\n- 操作系统：{} ({})\n- Shell：{} — {}\n",
            self.os, self.arch, self.shell, self.shell_guidance
        )
    }
}

/// 探测当前 Shell。
///
/// Windows：优先认 `SHELL` 环境变量（Git Bash / MSYS2 会设置），
/// 否则检测 `PSModulePath` 判断 PowerShell，缺省 `cmd.exe`。
/// Unix：读 `SHELL`，缺省 `/bin/sh`。
fn detect_shell() -> (String, String) {
    if env::consts::FAMILY == "windows" {
        if let Ok(sh) = env::var("SHELL") {
            let name = sh
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&sh)
                .to_string();
            return (
                format!("{} (Windows 上的 POSIX Shell)", name),
                "命令使用 POSIX/bash 语法（`$(...)`、`&&`、正斜杠路径 `C:/foo` 或 `/c/foo`）；Windows 原生工具（where/tasklist/reg）仍可直接调用，venv 工具在 `Scripts\\` 下".to_string(),
            );
        }
        if env::var("PSModulePath").is_ok() {
            return (
                "powershell".to_string(),
                "命令使用 PowerShell 语法（`$env:VAR`、`Select-String`、反引号续行）；Unix 命令（grep/head/ls）多数不可用，用 PowerShell 等价物".to_string(),
            );
        }
        return (
            "cmd.exe".to_string(),
            "命令使用 cmd.exe 语法（`%VAR%`、`findstr`、`dir`）；Unix 命令（grep/head/ls）不可用，用 cmd 等价物，路径用反斜杠".to_string(),
        );
    }

    let sh = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let name = sh.rsplit('/').next().unwrap_or(&sh).to_string();
    (name, "命令使用 POSIX shell 语法".to_string())
}

/// 可选：读取内核/OS 版本描述，失败返回 None（提示词中省略）。
#[allow(dead_code)]
fn os_version() -> Option<String> {
    if env::consts::FAMILY == "windows" {
        let out = Command::new("cmd").args(["/c", "ver"]).output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let out = Command::new("uname").arg("-rs").output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_consistent_consts() {
        let env_info = EnvInfo::detect();
        assert_eq!(env_info.os, env::consts::OS);
        assert_eq!(env_info.arch, env::consts::ARCH);
        assert!(!env_info.shell.is_empty());
        assert!(!env_info.shell_guidance.is_empty());
    }

    #[test]
    fn render_includes_os_and_shell() {
        let env_info = EnvInfo {
            os: "windows",
            arch: "x86_64",
            shell: "bash".to_string(),
            shell_guidance: "使用 POSIX 语法".to_string(),
        };
        let rendered = env_info.render_for_prompt();
        assert!(rendered.contains("## 运行环境"));
        assert!(rendered.contains("windows (x86_64)"));
        assert!(rendered.contains("bash"));
        assert!(rendered.contains("使用 POSIX 语法"));
    }
}
