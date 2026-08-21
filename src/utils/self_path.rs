//! 从当前可执行文件路径推导 dev-assistant-rs 源码根目录。
//!
//! 当 `dev-assistant` 通过 `--project` 指向其他项目时，仍能定位自身源码路径，
//! 从而支持自我修改（restart、文件读写等）。
//!
//! # 查找策略
//!
//! 1. 获取 `current_exe()` 路径
//! 2. 从可执行文件所在目录向上逐层查找 `Cargo.toml`
//! 3. 验证 `Cargo.toml` 中 `[package] name = "dev-assistant-rs"`
//! 4. 找到则返回该目录，否则返回 `None`
//!
//! 结果缓存到 `OnceLock` 中，避免重复查找。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static SELF_SOURCE_ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 返回 dev-assistant-rs 的源码根目录（包含 `Cargo.toml` 的目录）。
///
/// 结果被缓存，多次调用仅首次执行实际查找。
pub fn self_source_root() -> Option<&'static Path> {
    SELF_SOURCE_ROOT
        .get_or_init(compute_self_source_root)
        .as_deref()
}

/// 清除缓存（仅用于测试）。
#[allow(dead_code)]
pub fn clear_cache() {
    SELF_SOURCE_ROOT
        .set(None)
        .ok();
}

/// 从可执行文件路径向上查找，定位包含 `Cargo.toml` 且包名为 `dev-assistant-rs` 的目录。
fn compute_self_source_root() -> Option<PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    tracing::debug!(exe = %exe_path.display(), "查找自身源码根目录");

    // 从可执行文件所在目录开始向上查找
    let mut current = exe_path.parent()?;

    loop {
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.is_file() {
            if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                // 检查 Cargo.toml 中是否包含 `name = "dev-assistant-rs"`
                if content.contains("name = \"dev-assistant-rs\"") {
                    tracing::debug!(root = %current.display(), "找到自身源码根目录");
                    return Some(current.to_path_buf());
                }
            }
        }

        // 尝试向上查找
        match current.parent() {
            Some(parent) if parent != current => {
                current = parent;
            }
            _ => {
                tracing::warn!(
                    exe = %exe_path.display(),
                    "无法从可执行文件路径推导出 dev-assistant-rs 源码根目录"
                );
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_self_source_root_found() {
        // 在测试环境中，CARGO_MANIFEST_DIR 应指向 dev-assistant-rs 源码根目录
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = self_source_root();
        assert!(root.is_some(), "应能找到自身源码根目录");
        assert_eq!(root.unwrap(), manifest_dir, "应匹配 CARGO_MANIFEST_DIR");
    }

    #[test]
    fn test_compute_self_source_root() {
        // 验证 compute_self_source_root 能找到当前 Cargo.toml
        let result = compute_self_source_root();
        assert!(result.is_some(), "应能找到自身源码根目录");
        let cargo_toml = result.unwrap().join("Cargo.toml");
        assert!(cargo_toml.is_file(), "Cargo.toml 应存在");
    }
}