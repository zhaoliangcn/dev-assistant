//! Hook 配置加载：项目级 + 全局级 YAML 配置合并。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::warn;

use super::error::HookError;

/// Hook 事件类型。
///
/// 当前仅 `session-start` 接入执行（见 `HookManager::execute_session_start`），
/// 其余事件保留定义，待后续接入生命周期钩子；未知事件在 YAML 解析阶段直接报错。
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HookEvent {
    /// 会话启动时执行（当前唯一接入执行的事件）。
    SessionStart,
    /// 会话结束/退出前执行。
    SessionEnd,
    /// 工具调用前执行。
    PreTool,
    /// 工具调用后执行。
    PostTool,
    /// 用户输入前执行。
    UserInput,
}

impl HookEvent {
    /// 事件名（kebab-case，与 YAML `event` 字段一致）。
    pub fn as_str(&self) -> &'static str {
        match self {
            HookEvent::SessionStart => "session-start",
            HookEvent::SessionEnd => "session-end",
            HookEvent::PreTool => "pre-tool",
            HookEvent::PostTool => "post-tool",
            HookEvent::UserInput => "user-input",
        }
    }

    /// 从字符串解析事件名（kebab-case），未知事件返回 `None`。
    pub fn parse(s: &str) -> Option<HookEvent> {
        match s {
            "session-start" => Some(HookEvent::SessionStart),
            "session-end" => Some(HookEvent::SessionEnd),
            "pre-tool" => Some(HookEvent::PreTool),
            "post-tool" => Some(HookEvent::PostTool),
            "user-input" => Some(HookEvent::UserInput),
            _ => None,
        }
    }
}

/// 单个 hook 的 YAML 定义。
#[derive(Deserialize, Debug, Clone)]
pub struct HookConfig {
    pub name: String,
    pub event: HookEvent,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    pub type_: String,
    pub command: String,
    pub args: Option<Vec<String>>,
    pub timeout: Option<u64>,
    pub priority: Option<i32>,
    #[allow(dead_code)]
    pub wrap_tag: Option<String>,
    pub max_output_bytes: Option<usize>,
}

/// 顶层 YAML 配置。
#[derive(Deserialize, Debug, Clone, Default)]
pub struct HooksFile {
    pub hooks: Option<Vec<HookConfig>>,
    /// 全部 hook 输出的总字节预算；超出部分按 priority 从低到高丢弃（默认 8KB）。
    pub max_total_bytes: Option<usize>,
}

/// 加载完成后的 hooks 配置：hook 列表 + 总预算。
#[derive(Debug, Clone)]
pub struct LoadedHooks {
    pub hooks: Vec<HookConfig>,
    pub max_total_bytes: usize,
}

/// 默认总字节预算：8KB（约 2 个满额 hook 输出，避免大量 hook 灌爆上下文）。
pub const DEFAULT_MAX_TOTAL_BYTES: usize = 8 * 1024;

/// 项目级 hooks 配置文件路径。
fn project_hooks_path(working_dir: &Path) -> PathBuf {
    working_dir.join(".dev-assistant").join("hooks.yaml")
}

/// 全局级 hooks 配置文件路径。
fn global_hooks_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".dev-assistant").join("hooks.yaml"))
}

/// 从单个 YAML 文件加载 hooks 配置。
fn load_hooks_file(path: &Path) -> Result<HooksFile, HookError> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HooksFile::default()),
        Err(e) => return Err(HookError::Io(e)),
    };

    if content.trim().is_empty() {
        return Ok(HooksFile::default());
    }

    let parsed: HooksFile = serde_yml::from_str(&content)
        .map_err(|e| HookError::Yaml(format!("{}: {}", path.display(), e)))?;

    Ok(parsed)
}

/// 加载所有 hooks 配置，附带总字节预算。
pub fn load_hooks_config_full(working_dir: &Path) -> Result<LoadedHooks, HookError> {
    load_hooks_config_with_global(working_dir, global_hooks_path())
}

/// 内部实现：允许测试注入全局配置路径，避免污染真实 HOME。
fn load_hooks_config_with_global(
    working_dir: &Path,
    global_path: Option<PathBuf>,
) -> Result<LoadedHooks, HookError> {
    let mut by_name: BTreeMap<String, HookConfig> = BTreeMap::new();
    let mut max_total_bytes: Option<usize> = None;

    // 全局级（先加载，项目级会覆盖同名项）
    if let Some(global_path) = global_path {
        if global_path.exists() {
            let file = load_hooks_file(&global_path)?;
            for h in file.hooks.unwrap_or_default() {
                by_name.insert(h.name.clone(), h);
            }
            if file.max_total_bytes.is_some() {
                max_total_bytes = file.max_total_bytes;
            }
        }
    }

    // 项目级（后加载，覆盖同名；预算同样以项目级为准）
    let project_file = load_hooks_file(&project_hooks_path(working_dir))?;
    for h in project_file.hooks.unwrap_or_default() {
        by_name.insert(h.name.clone(), h);
    }
    if project_file.max_total_bytes.is_some() {
        max_total_bytes = project_file.max_total_bytes;
    }

    let mut result: Vec<HookConfig> = by_name.into_values().collect();
    result.sort_by_key(|h| (h.priority.unwrap_or(50), h.name.clone()));

    // 未知事件在 YAML 解析阶段已报错（serde 枚举校验）；
    // 已定义但尚未接入分派的事件（如 session-end）在此告警，避免静默丢失。
    for h in result.iter().filter(|h| h.event != HookEvent::SessionStart) {
        warn!(
            name = %h.name,
            event = h.event.as_str(),
            "Hook event is not dispatched yet, hook will be skipped"
        );
    }

    Ok(LoadedHooks {
        hooks: result,
        max_total_bytes: max_total_bytes.unwrap_or(DEFAULT_MAX_TOTAL_BYTES),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_hooks_yaml(dir: &Path, yaml: &str) -> PathBuf {
        let hooks_dir = dir.join(".dev-assistant");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let path = hooks_dir.join("hooks.yaml");
        std::fs::write(&path, yaml).unwrap();
        path
    }

    #[test]
    fn load_empty_file_returns_empty() {
        let dir = tempdir().unwrap();
        let configs = load_hooks_config_full(dir.path()).unwrap();
        assert!(configs.hooks.is_empty());
    }

    #[test]
    fn load_single_hook() {
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: test-hook
    event: session-start
    type: shell
    command: /bin/echo
    args: ["hello"]
    timeout: 5
    priority: 1
"#,
        );
        let configs = load_hooks_config_full(dir.path()).unwrap();
        assert_eq!(configs.hooks.len(), 1);
        assert_eq!(configs.hooks[0].name, "test-hook");
        assert_eq!(configs.hooks[0].event, HookEvent::SessionStart);
        assert_eq!(configs.hooks[0].type_, "shell");
        assert_eq!(configs.hooks[0].command, "/bin/echo");
        assert_eq!(configs.hooks[0].args, Some(vec!["hello".to_string()]));
        assert_eq!(configs.hooks[0].timeout, Some(5));
        assert_eq!(configs.hooks[0].priority, Some(1));
    }

    #[test]
    fn project_overrides_global() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".dev-assistant")).unwrap();
        std::fs::write(
            home.join(".dev-assistant").join("hooks.yaml"),
            r#"
hooks:
  - name: same-hook
    event: session-start
    type: shell
    command: /global/script
    priority: 10
"#,
        )
        .unwrap();

        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: same-hook
    event: session-start
    type: shell
    command: /project/script
    priority: 1
"#,
        );

        let configs = load_hooks_config_with_global(
            dir.path(),
            Some(home.join(".dev-assistant").join("hooks.yaml")),
        )
        .unwrap();
        assert_eq!(configs.hooks.len(), 1);
        assert_eq!(configs.hooks[0].command, "/project/script");
        assert_eq!(configs.hooks[0].priority, Some(1));
    }

    #[test]
    fn budget_defaults_to_8k() {
        let dir = tempdir().unwrap();
        write_hooks_yaml(
            dir.path(),
            r#"
hooks:
  - name: h
    event: session-start
    type: shell
    command: /bin/echo
"#,
        );
        let loaded = load_hooks_config_full(dir.path()).unwrap();
        assert_eq!(loaded.max_total_bytes, DEFAULT_MAX_TOTAL_BYTES);
    }

    #[test]
    fn project_budget_overrides_global() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".dev-assistant")).unwrap();
        std::fs::write(
            home.join(".dev-assistant").join("hooks.yaml"),
            r#"
max_total_bytes: 2048
hooks:
  - name: g
    event: session-start
    type: shell
    command: /global/script
"#,
        )
        .unwrap();

        write_hooks_yaml(
            dir.path(),
            r#"
max_total_bytes: 1024
hooks:
  - name: p
    event: session-start
    type: shell
    command: /project/script
"#,
        );

        let loaded = load_hooks_config_with_global(
            dir.path(),
            Some(home.join(".dev-assistant").join("hooks.yaml")),
        )
        .unwrap();
        assert_eq!(loaded.max_total_bytes, 1024);
    }
}