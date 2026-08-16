//! Hook 机制。
//!
//! 在生命周期事件触发时执行配置的 hook，并将输出注入模型上下文。
//!
//! # 已接入的事件
//!
//! - `session-start` — 会话启动（输出注入为 System 消息）
//! - `session-end` — 会话结束（输出仅记日志）
//! - `pre-tool` — 工具调用前（可 `DENY` 拦截）
//! - `post-tool` — 工具调用后（透传工具成败，输出仅记日志）
//! - `user-input` — 顶层用户消息到达时（输出注入为该轮 System 消息，inject-only）
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
use crate::hooks::shell::{execute_shell_hook, execute_shell_hook_with_input, execute_tool_hook};

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

        self.collect_and_format(hooks, |config| {
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
        })
    }

    /// 执行 user-input hooks（仅注入上下文，不否决/不改写用户输入）。
    ///
    /// 将用户消息原文透传给 hook stdin payload 的 `input` 字段；输出经字节预算
    /// 裁剪后格式化，调用方将其作为该轮 System 消息注入模型上下文。与
    /// session-start 同为 inject-only 模型，但按每条顶层用户消息触发。
    pub fn execute_user_input(&self, user_message: &str) -> String {
        if !self.enabled || self.hooks.is_empty() {
            return String::new();
        }

        let hooks: Vec<&HookConfig> = self
            .hooks
            .iter()
            .filter(|c| c.event == HookEvent::UserInput)
            .collect();
        if hooks.is_empty() {
            return String::new();
        }

        self.collect_and_format(hooks, |config| {
            let event = config.event;
            let workdir = self.working_dir.clone();
            match execute_shell_hook_with_input(config, &event, &workdir, user_message) {
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

    /// 并行执行给定 hooks，按字节预算裁剪后格式化为注入内容。
    ///
    /// `make_outcome` 决定单个 hook 如何执行（事件级 / user-input 级），
    /// 其捕获的引用由 `Clone` 复制进每个 scoped 线程。
    fn collect_and_format<F>(&self, hooks: Vec<&HookConfig>, make_outcome: F) -> String
    where
        F: Fn(&HookConfig) -> HookOutcome + Clone + Send,
    {
        let outcomes = self.run_parallel(hooks, make_outcome);

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
    /// `success` 仅 post-tool 有意义：透传至 hook 进程的环境变量与 stdin payload，
    /// 使 post-tool hook 能按工具成败分支。pre-tool 传 `None`（工具尚未执行）。
    ///
    /// 返回各 hook 的结果（按 priority 顺序）。hook 输出**不注入模型上下文**，
    /// 仅用于决策（pre-tool）或日志（post-tool）。
    fn execute_tool_hooks(
        &self,
        event: HookEvent,
        tool_name: &str,
        tool_args: &serde_json::Value,
        success: Option<bool>,
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
            match execute_tool_hook(config, &event, &workdir, tool_name, tool_args, success) {
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

    /// pre-tool 检查：任一 hook 成功且输出以 `DENY`（大小写不敏感）开头则拒绝执行工具。
    ///
    /// - `DENY` 后的文本作为拒绝原因（保留原始大小写）
    /// - hook 失败（非零退出/超时）视为放行，避免 hook 故障阻塞工具执行
    /// - 拒绝原因写入 tracing 日志，便于事后排查
    pub fn run_pre_tool(&self, tool_name: &str, tool_args: &serde_json::Value) -> PreToolVerdict {
        let outcomes = self.execute_tool_hooks(HookEvent::PreTool, tool_name, tool_args, None);
        for o in &outcomes {
            if o.success {
                let trimmed = o.detail.trim();
                // 大小写不敏感匹配 "DENY" 前缀。"DENY" 为 4 个 ASCII 字节，
                // 一旦前缀匹配，第 4 字节必为字符边界，trimmed[4..] 可安全切片。
                if trimmed.len() >= 4 && trimmed[..4].eq_ignore_ascii_case("DENY") {
                    let reason = if trimmed[4..].trim().is_empty() {
                        "denied by pre-tool hook".to_string()
                    } else {
                        trimmed[4..].trim().to_string()
                    };
                    warn!(
                        tool = tool_name,
                        hook = %o.name,
                        reason = %reason,
                        "Pre-tool hook denied tool execution"
                    );
                    return PreToolVerdict::Deny(reason);
                }
            }
        }
        PreToolVerdict::Allow
    }

    /// post-tool 通知：执行工具后运行，返回格式化输出（供调用方按需记录，不注入上下文）。
    ///
    /// 格式化输出同时写入 `debug` 日志，避免此前"计算后即丢弃"的浪费——
    /// post-tool hook 的 stdout 至少留有日志踪迹。
    pub fn run_post_tool(
        &self,
        tool_name: &str,
        tool_args: &serde_json::Value,
        success: bool,
    ) -> String {
        let outcomes =
            self.execute_tool_hooks(HookEvent::PostTool, tool_name, tool_args, Some(success));
        if outcomes.is_empty() {
            return String::new();
        }
        let formatted = format_hook_output(&outcomes);
        debug!(
            tool = tool_name,
            success,
            count = outcomes.len(),
            output = %formatted,
            "Post-tool hooks executed"
        );
        formatted
    }

    /// 并行执行一组 hooks：每个 hook 一个 scoped 线程，join 按配置顺序收集结果。
    fn run_parallel<F>(&self, hooks: Vec<&HookConfig>, make_outcome: F) -> Vec<HookOutcome>
    where
        F: Fn(&HookConfig) -> HookOutcome + Clone + Send,
    {
        std::thread::scope(|scope| {
            let handles: Vec<_> = hooks
                .iter()
                .copied()
                .map(|config| {
                    // Clone 而非 Copy：允许闭包捕获非 Copy 值（如 owned 数据）。
                    // 每个线程拿到自己的副本，spawn 闭包 move 该副本。
                    let f = make_outcome.clone();
                    scope.spawn(move || f(config))
                })
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

    #[test]
    fn post_tool_forwards_success_to_hook() {
        // post-tool hook 通过环境变量读取工具成败状态
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: success-echo
    event: post-tool
    type: shell
    command: sh
    args: ["-c", "printf %s \"$DEV_ASSISTANT_TOOL_SUCCESS\""]
"#,
        );
        let mgr = HookManager::load(dir.path(), true);
        let ok_out = mgr.run_post_tool("write_file", &serde_json::json!({}), true);
        assert!(ok_out.contains("true"), "post-tool hook should see success=true: {ok_out}");
        let fail_out = mgr.run_post_tool("write_file", &serde_json::json!({}), false);
        assert!(fail_out.contains("false"), "post-tool hook should see success=false: {fail_out}");
    }

    #[test]
    fn user_input_injects_hook_output() {
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: ctx
    event: user-input
    type: shell
    command: echo
    args: ["per-turn-context"]
"#,
        );
        let mgr = HookManager::load(dir.path(), true);
        let out = mgr.execute_user_input("hello");
        assert!(out.contains("<HOOK name=\"ctx\""));
        assert!(out.contains("per-turn-context"));
    }

    #[test]
    fn user_input_no_hooks_returns_empty() {
        // 仅有 session-start hook，无 user-input hook → 返回空字符串
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: only-start
    event: session-start
    type: shell
    command: echo
    args: ["x"]
"#,
        );
        let mgr = HookManager::load(dir.path(), true);
        assert_eq!(mgr.execute_user_input("hello"), "");
    }

    #[test]
    fn user_input_disabled_returns_empty() {
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: ctx
    event: user-input
    type: shell
    command: echo
    args: ["x"]
"#,
        );
        let mgr = HookManager::load(dir.path(), false);
        assert_eq!(mgr.execute_user_input("hello"), "");
    }
}