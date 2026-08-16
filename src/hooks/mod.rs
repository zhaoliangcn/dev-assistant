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

use std::path::{Path, PathBuf};

use tracing::{debug, warn};

use crate::hooks::config::{HookConfig, HookEvent};
use crate::hooks::shell::{execute_shell_hook, execute_tool_hook};

/// Hook 管理器：负责按配置执行 hooks 并收集输出。
pub struct HookManager {
    hooks: Vec<HookConfig>,
    working_dir: PathBuf,
    max_total_bytes: usize,
    enabled: bool,
}

impl HookManager {
    /// 从工作目录加载 hooks 配置。
    ///
    /// `enabled` 由 `--no-hooks` CLI 参数控制，为 false 时不加载也不执行任何 hook。
    pub fn load(working_dir: &Path, enabled: bool) -> Self {
        let (hooks, max_total_bytes) = if enabled {
            match config::load_hooks_config_full(working_dir) {
                Ok(loaded) => (loaded.hooks, loaded.max_total_bytes),
                Err(e) => {
                    warn!(error = %e, "Failed to load hooks config, hooks disabled");
                    (Vec::new(), config::DEFAULT_MAX_TOTAL_BYTES)
                }
            }
        } else {
            debug!("Hooks disabled via --no-hooks");
            (Vec::new(), config::DEFAULT_MAX_TOTAL_BYTES)
        };
        Self {
            hooks,
            working_dir: working_dir.to_path_buf(),
            max_total_bytes,
            enabled,
        }
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

    /// 预览将执行的 hooks（不实际执行），返回人类可读的多行清单。
    pub fn dry_run(&self) -> String {
        if !self.enabled {
            return "Hooks are disabled (--no-hooks); nothing would run.".to_string();
        }
        if self.hooks.is_empty() {
            return "No hooks configured.".to_string();
        }
        let mut out = String::new();
        for h in &self.hooks {
            let args = h
                .args
                .as_deref()
                .map(|a| a.join(" "))
                .unwrap_or_default();
            out.push_str(&format!(
                "• [{}] {} — {} {} (priority {}, timeout {}s, max {}B)\n",
                h.event.as_str(),
                h.name,
                h.command,
                args,
                h.priority.unwrap_or(50),
                h.timeout.unwrap_or(5),
                h.max_output_bytes.unwrap_or(crate::hooks::shell::DEFAULT_MAX_OUTPUT),
            ));
        }
        out.push_str(&format!("Total byte budget: {} bytes", self.max_total_bytes));
        out
    }

    /// 执行全部 session-start hooks，返回格式化后的注入内容。
    pub fn execute_session_start(&self) -> String {
        self.execute_event(HookEvent::SessionStart, None)
    }

    /// 执行指定事件的 hooks（可按名称过滤），返回格式化后的注入内容。
    ///
    /// - 仅分派配置中存在的该事件 hooks（其余事件在加载时已告警提示）
    /// - 各 hook 并行执行（`std::thread::scope`），join 按配置顺序收集，保持 priority 顺序
    /// - 单个 hook 失败不中断整体：成功与失败都会进入注入内容，并携带 `status` / `reason`
    pub fn execute_event(&self, event: HookEvent, name_filter: Option<&str>) -> String {
        if !self.enabled || self.hooks.is_empty() {
            return String::new();
        }

        let hooks: Vec<&HookConfig> = self
            .hooks
            .iter()
            .filter(|c| c.event == event)
            .filter(|c| name_filter.is_none_or(|n| c.name == n))
            .collect();
        if hooks.is_empty() {
            return String::new();
        }

        // 并行执行：每个 hook 一个 scoped 线程，join 顺序即配置顺序
        let outcomes = self.run_parallel(hooks, |config| {
            let event = config.event;
            let workdir = self.working_dir.clone();
            match execute_shell_hook(config, &event, &workdir) {
                Ok(result) => HookOutcome {
                    name: result.name.clone(),
                    success: true,
                    detail: result.output,
                },
                Err(e) => HookOutcome {
                    name: config.name.clone(),
                    success: false,
                    detail: e.to_string(),
                },
            }
        });

        let failures = outcomes.iter().filter(|o| !o.success).count();
        debug!(total = outcomes.len(), failures, "Hooks executed");
        if failures > 0 {
            warn!(failures, "Some hooks failed; status included in injected output");
        }

        // 总字节预算：outcomes 已按 priority 升序（高优先级在前），
        // 从低优先级（末尾）开始丢弃，直到总大小不超预算。
        let mut kept: Vec<HookOutcome> = Vec::new();
        let mut total_bytes = 0usize;
        for outcome in outcomes {
            if total_bytes + outcome.detail.len() <= self.max_total_bytes {
                total_bytes += outcome.detail.len();
                kept.push(outcome);
            } else {
                warn!(
                    name = %outcome.name,
                    budget = self.max_total_bytes,
                    "Hook output dropped by total byte budget"
                );
            }
        }

        format_hook_output(&kept)
    }

    /// 执行工具级 hooks（pre-tool / post-tool），携带工具名与参数上下文。
    ///
    /// 返回各 hook 的结果（按 priority 顺序）。hook 输出**不注入模型上下文**，
    /// 仅用于决策（pre-tool）或日志（post-tool）。
    fn execute_tool_hooks(
        &self,
        event: HookEvent,
        tool_name: &str,
        tool_args: &serde_json::Value,
    ) -> Vec<HookOutcome> {
        if !self.enabled || self.hooks.is_empty() {
            return Vec::new();
        }

        let hooks: Vec<&HookConfig> = self
            .hooks
            .iter()
            .filter(|c| c.event == event)
            .collect();
        if hooks.is_empty() {
            return Vec::new();
        }

        self.run_parallel(hooks, |config| {
            let event = config.event;
            let workdir = self.working_dir.clone();
            match execute_tool_hook(config, &event, &workdir, tool_name, tool_args) {
                Ok(result) => HookOutcome {
                    name: result.name.clone(),
                    success: true,
                    detail: result.output,
                },
                Err(e) => HookOutcome {
                    name: config.name.clone(),
                    success: false,
                    detail: e.to_string(),
                },
            }
        })
    }

    /// pre-tool 检查：任一 hook 成功且输出以 `DENY` 开头则拒绝执行工具。
    ///
    /// - `DENY` 后的文本作为拒绝原因
    /// - hook 失败（非零退出/超时）视为放行，避免 hook 故障阻塞工具执行
    pub fn run_pre_tool(&self, tool_name: &str, tool_args: &serde_json::Value) -> PreToolVerdict {
        let outcomes = self.execute_tool_hooks(HookEvent::PreTool, tool_name, tool_args);
        for o in &outcomes {
            if o.success {
                let trimmed = o.detail.trim();
                if let Some(rest) = trimmed.strip_prefix("DENY") {
                    let reason = rest.trim();
                    return PreToolVerdict::Deny(if reason.is_empty() {
                        "denied by pre-tool hook".to_string()
                    } else {
                        reason.to_string()
                    });
                }
            }
        }
        PreToolVerdict::Allow
    }

    /// post-tool 通知：执行工具后运行，返回格式化输出（供日志记录，不注入上下文）。
    pub fn run_post_tool(
        &self,
        tool_name: &str,
        tool_args: &serde_json::Value,
        success: bool,
    ) -> String {
        let outcomes = self.execute_tool_hooks(HookEvent::PostTool, tool_name, tool_args);
        if outcomes.is_empty() {
            return String::new();
        }
        debug!(tool = tool_name, success, count = outcomes.len(), "Post-tool hooks executed");
        format_hook_output(&outcomes)
    }

    /// 并行执行一组 hooks：每个 hook 一个 scoped 线程，join 按配置顺序收集结果。
    fn run_parallel<F>(&self, hooks: Vec<&HookConfig>, make_outcome: F) -> Vec<HookOutcome>
    where
        F: Fn(&HookConfig) -> HookOutcome + Copy + Send,
    {
        std::thread::scope(|scope| {
            let handles: Vec<_> = hooks
                .iter()
                .copied()
                .map(|config| scope.spawn(move || make_outcome(config)))
                .collect();

            handles
                .into_iter()
                .map(|handle| {
                    handle.join().unwrap_or_else(|_| HookOutcome {
                        name: String::from("<panicked>"),
                        success: false,
                        detail: "hook thread panicked".to_string(),
                    })
                })
                .collect()
        })
    }
}

/// pre-tool hook 的执行裁定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreToolVerdict {
    /// 允许继续执行工具。
    Allow,
    /// 拒绝执行工具，携带原因。
    Deny(String),
}

/// 单个 hook 的执行结果（成功或失败），用于注入上下文。
struct HookOutcome {
    name: String,
    success: bool,
    /// 成功时是 stdout 输出；失败时是错误描述。
    detail: String,
}

/// 将多个 hook 结果格式化为注入上下文块。
///
/// 成功项携带 `status="ok"`，失败项携带 `status="failed"` 与 `reason` 属性，
/// 让模型能感知哪些 hook 失败、为什么失败。name/detail 均做 XML 转义，
/// 防止 hook 输出（如 `</HOOK>`）破坏注入结构。
fn format_hook_output(outcomes: &[HookOutcome]) -> String {
    let mut out = String::from("<HOOKS>\n");
    for o in outcomes {
        if o.success {
            out.push_str(&format!(
                "<HOOK name=\"{}\" status=\"ok\" type=\"shell\">\n{}\n</HOOK>\n",
                escape_xml_attr(&o.name),
                escape_xml_attr(&o.detail)
            ));
        } else {
            out.push_str(&format!(
                "<HOOK name=\"{}\" status=\"failed\" type=\"shell\" reason=\"{}\">\n</HOOK>\n",
                escape_xml_attr(&o.name),
                escape_xml_attr(&o.detail)
            ));
        }
    }
    out.push_str("</HOOKS>");
    out
}

/// 转义 XML 属性中的特殊字符，避免 hook 输出破坏 `<HOOK>` 标签结构。
fn escape_xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
    fn failed_hook_does_not_panic_and_reports_status() {
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
        let output = mgr.execute_session_start();
        // 失败 hook 也进入注入内容，携带 status="failed" 与 reason
        assert!(output.contains("status=\"failed\""));
        assert!(output.contains("reason="));
    }

    #[test]
    fn successful_hook_reports_ok_status() {
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: greet
    event: session-start
    type: shell
    command: echo
    args: ["hello"]
"#,
        );
        let mgr = HookManager::load(dir.path(), true);
        let output = mgr.execute_session_start();
        assert!(output.contains("status=\"ok\""));
        assert!(output.contains("hello"));
    }

    #[test]
    fn hook_output_escapes_xml_attrs() {
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: tricky
    event: session-start
    type: shell
    command: sh
    args: ["-c", "echo '\"quoted\" & <tag>'"]
"#,
        );
        let mgr = HookManager::load(dir.path(), true);
        let output = mgr.execute_session_start();
        // 输出中的特殊字符被转义，不会破坏 <HOOK> 结构
        assert!(output.contains("&quot;quoted&quot;"));
        assert!(output.contains("&amp;"));
        assert!(output.contains("&lt;tag&gt;"));
    }

    #[test]
    fn no_hooks_file_returns_empty() {
        let dir = tempdir().unwrap();
        let mgr = HookManager::load(dir.path(), true);
        assert_eq!(mgr.hook_count(), 0);
        assert_eq!(mgr.execute_session_start(), "");
    }

    #[test]
    fn non_session_start_events_are_skipped() {
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: end-hook
    event: session-end
    type: shell
    command: echo
    args: ["should-not-run"]
  - name: start-hook
    event: session-start
    type: shell
    command: echo
    args: ["hello-from-start"]
"#,
        );
        let mgr = HookManager::load(dir.path(), true);
        assert_eq!(mgr.hook_count(), 2);
        let output = mgr.execute_session_start();
        assert!(output.contains("hello-from-start"));
        assert!(!output.contains("should-not-run"));
    }

    #[test]
    fn parallel_execution_preserves_priority_order() {
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: slow-first
    event: session-start
    type: shell
    command: sh
    args: ["-c", "sleep 0.3; echo first"]
    priority: 1
  - name: fast-second
    event: session-start
    type: shell
    command: sh
    args: ["-c", "echo second"]
    priority: 2
"#,
        );
        let mgr = HookManager::load(dir.path(), true);
        let output = mgr.execute_session_start();
        let first_pos = output.find("first").expect("first hook output present");
        let second_pos = output.find("second").expect("second hook output present");
        assert!(first_pos < second_pos, "output must follow priority order");
    }

    #[test]
    fn pre_tool_deny_blocks_tool() {
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: guard
    event: pre-tool
    type: shell
    command: echo
    args: ["DENY 高危操作已被拦截"]
"#,
        );
        let mgr = HookManager::load(dir.path(), true);
        let verdict = mgr.run_pre_tool("write_file", &serde_json::json!({"file_path": "x"}));
        assert_eq!(
            verdict,
            PreToolVerdict::Deny("高危操作已被拦截".to_string())
        );
    }

    #[test]
    fn pre_tool_allow_passes() {
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: guard
    event: pre-tool
    type: shell
    command: echo
    args: ["ALLOW"]
"#,
        );
        let mgr = HookManager::load(dir.path(), true);
        let verdict = mgr.run_pre_tool("read_file", &serde_json::json!({"file_path": "x"}));
        assert_eq!(verdict, PreToolVerdict::Allow);
    }

    #[test]
    fn pre_tool_failure_is_fail_open() {
        // hook 执行失败（命令不存在）不应阻塞工具执行
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: broken-guard
    event: pre-tool
    type: shell
    command: /nonexistent/command
"#,
        );
        let mgr = HookManager::load(dir.path(), true);
        let verdict = mgr.run_pre_tool("read_file", &serde_json::json!({}));
        assert_eq!(verdict, PreToolVerdict::Allow);
    }

    #[test]
    fn pre_tool_no_hooks_allows() {
        let dir = tempdir().unwrap();
        let mgr = HookManager::load(dir.path(), true);
        let verdict = mgr.run_pre_tool("read_file", &serde_json::json!({}));
        assert_eq!(verdict, PreToolVerdict::Allow);
    }

    #[test]
    fn post_tool_runs_and_returns_output() {
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: notify
    event: post-tool
    type: shell
    command: echo
    args: ["done-notify"]
"#,
        );
        let mgr = HookManager::load(dir.path(), true);
        let output = mgr.run_post_tool("read_file", &serde_json::json!({"file_path": "x"}), true);
        assert!(output.contains("done-notify"));
        assert!(output.contains("<HOOK name=\"notify\""));
    }
}