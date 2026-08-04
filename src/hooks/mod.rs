//! Hook 机制。
//!
//! 在会话启动时执行配置的 hook，将输出注入模型上下文。
//!
//! # 模块结构
//!
//! - `mod.rs` — HookManager：加载配置、执行 hooks、收集输出
//! - `config.rs` — YAML 配置加载（项目级 + 全局级合并）
//! - `shell.rs` — shell hook 执行器（进程 spawn、超时、stdout 捕获）
//! - `error.rs` — 专用错误类型

pub mod config;
pub mod error;
pub mod shell;

use std::path::Path;

use tracing::{debug, warn};

use crate::hooks::config::HookConfig;
use crate::hooks::shell::{execute_shell_hook, HookResult};

/// Hook 管理器：负责按配置执行 session-start hooks 并收集输出。
pub struct HookManager {
    hooks: Vec<HookConfig>,
    enabled: bool,
}

impl HookManager {
    /// 从工作目录加载 hooks 配置。
    ///
    /// `enabled` 由 `--no-hooks` CLI 参数控制，为 false 时不加载也不执行任何 hook。
    pub fn load(working_dir: &Path, enabled: bool) -> Self {
        let hooks = if enabled {
            match config::load_hooks_config(working_dir) {
                Ok(hooks) => hooks,
                Err(e) => {
                    warn!(error = %e, "Failed to load hooks config, hooks disabled");
                    Vec::new()
                }
            }
        } else {
            debug!("Hooks disabled via --no-hooks");
            Vec::new()
        };
        Self { hooks, enabled }
    }

    /// 是否启用了 hook 机制。
    #[allow(dead_code)]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// 已加载的 hook 数量。
    #[allow(dead_code)]
    pub fn hook_count(&self) -> usize {
        self.hooks.len()
    }

    /// 执行全部 session-start hooks，返回格式化后的注入内容。
    ///
    /// 单个 hook 失败不中断整体：失败者被跳过，成功者输出保留。
    pub fn execute_session_start(&self) -> String {
        if !self.enabled || self.hooks.is_empty() {
            return String::new();
        }

        let mut results: Vec<HookResult> = Vec::new();
        let mut failures = 0usize;

        for config in &self.hooks {
            match execute_shell_hook(config) {
                Ok(result) => {
                    debug!(name = %result.name, "Hook executed successfully");
                    results.push(result);
                }
                Err(e) => {
                    failures += 1;
                    warn!(name = %config.name, error = %e, "Hook execution failed");
                }
            }
        }

        if results.is_empty() {
            if failures > 0 {
                debug!(failures, "All hooks failed, nothing to inject");
            }
            return String::new();
        }

        format_hook_output(&results)
    }
}

/// 将多个 hook 结果格式化为注入上下文块。
fn format_hook_output(results: &[HookResult]) -> String {
    let mut out = String::from("<HOOKS>\n");
    for r in results {
        out.push_str(&format!(
            "<HOOK name=\"{}\" type=\"shell\">\n{}\n</HOOK>\n",
            r.name, r.output
        ));
    }
    out.push_str("</HOOKS>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_hooks_yaml(dir: &std::path::Path, yaml: &str) {
        let hooks_dir = dir.join(".dev-assistant");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("hooks.yaml"), yaml).unwrap();
    }

    #[test]
    fn disabled_manager_runs_nothing() {
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: boom
    event: session-start
    type: shell
    command: /nonexistent/command
"#,
        );
        let mgr = HookManager::load(dir.path(), false);
        assert!(!mgr.enabled());
        assert_eq!(mgr.execute_session_start(), "");
    }

    #[test]
    fn executes_shell_hook_and_formats_output() {
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: greet
    event: session-start
    type: shell
    command: echo
    args: ["hello-from-hook"]
"#,
        );
        let mgr = HookManager::load(dir.path(), true);
        assert_eq!(mgr.hook_count(), 1);
        let output = mgr.execute_session_start();
        assert!(output.contains("<HOOK name=\"greet\""));
        assert!(output.contains("hello-from-hook"));
        assert!(output.contains("</HOOKS>"));
    }

    #[test]
    fn failed_hook_does_not_panic() {
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: bad
    event: session-start
    type: shell
    command: /nonexistent/command
"#,
        );
        let mgr = HookManager::load(dir.path(), true);
        assert_eq!(mgr.execute_session_start(), "");
    }

    #[test]
    fn no_hooks_file_returns_empty() {
        let dir = tempdir().unwrap();
        let mgr = HookManager::load(dir.path(), true);
        assert_eq!(mgr.hook_count(), 0);
        assert_eq!(mgr.execute_session_start(), "");
    }
}