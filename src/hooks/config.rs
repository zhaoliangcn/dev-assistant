//! Hook 配置加载：项目级 + 全局级 YAML 配置合并。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::error::HookError;

/// 单个 hook 的 YAML 定义。
#[derive(Deserialize, Debug, Clone)]
pub struct HookConfig {
    pub name: String,
    pub event: String,
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
#[derive(Deserialize, Debug, Clone)]
pub struct HooksFile {
    pub hooks: Option<Vec<HookConfig>>,
}

/// 项目级 hooks 配置文件路径。
fn project_hooks_path(working_dir: &Path) -> PathBuf {
    working_dir.join(".dev-assistant").join("hooks.yaml")
}

/// 全局级 hooks 配置文件路径。
fn global_hooks_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".dev-assistant").join("hooks.yaml"))
}

/// 从单个 YAML 文件加载 hooks 配置。
fn load_hooks_file(path: &Path) -> Result<Vec<HookConfig>, HookError> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(HookError::Io(e)),
    };

    if content.trim().is_empty() {
        return Ok(Vec::new());
    }

    let parsed: HooksFile = serde_yml::from_str(&content)
        .map_err(|e| HookError::Yaml(format!("{}: {}", path.display(), e)))?;

    Ok(parsed.hooks.unwrap_or_default())
}

/// 加载所有 hooks 配置：项目级 + 全局级合并。
///
/// 合并规则：
/// - 项目级优先：同名 hook 以项目级配置为准
/// - 排序：按 `(priority, name)` 升序，未设 priority 的默认 50
pub fn load_hooks_config(working_dir: &Path) -> Result<Vec<HookConfig>, HookError> {
    load_hooks_config_with_global(working_dir, global_hooks_path())
}

/// 内部实现：允许测试注入全局配置路径，避免污染真实 HOME。
fn load_hooks_config_with_global(
    working_dir: &Path,
    global_path: Option<PathBuf>,
) -> Result<Vec<HookConfig>, HookError> {
    let mut by_name: BTreeMap<String, HookConfig> = BTreeMap::new();

    // 全局级（先加载，项目级会覆盖同名项）
    if let Some(global_path) = global_path {
        if global_path.exists() {
            for h in load_hooks_file(&global_path)? {
                by_name.insert(h.name.clone(), h);
            }
        }
    }

    // 项目级（后加载，覆盖同名）
    for h in load_hooks_file(&project_hooks_path(working_dir))? {
        by_name.insert(h.name.clone(), h);
    }

    let mut result: Vec<HookConfig> = by_name.into_values().collect();
    result.sort_by_key(|h| (h.priority.unwrap_or(50), h.name.clone()));

    debug_assert!(
        result.iter().all(|h| h.event == "session-start"),
        "Only session-start event is supported"
    );

    Ok(result)
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
        let configs = load_hooks_config(dir.path()).unwrap();
        assert!(configs.is_empty());
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
        let configs = load_hooks_config(dir.path()).unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "test-hook");
        assert_eq!(configs[0].event, "session-start");
        assert_eq!(configs[0].type_, "shell");
        assert_eq!(configs[0].command, "/bin/echo");
        assert_eq!(configs[0].args, Some(vec!["hello".to_string()]));
        assert_eq!(configs[0].timeout, Some(5));
        assert_eq!(configs[0].priority, Some(1));
    }

    #[test]
    fn project_overrides_global() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home.join(".dev-assistant")).unwrap();
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
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].command, "/project/script");
        assert_eq!(configs[0].priority, Some(1));
    }
}