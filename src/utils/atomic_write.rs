//! 原子文件写入工具。
//!
//! 崩溃恢复依赖的持久化文件（检查点、pipeline 上下文、阶段上下文、分层摘要、
//! 会话状态等）若直接 `fs::write`，进程在写入中途崩溃会留下截断/损坏的文件，
//! 导致 `--resume-pipeline` / 摘要重建失败。本模块提供 [`atomic_write`]：
//! 先写同目录临时文件并 `fsync`，再 `rename` 原子覆盖目标，保证读者
//! 要么看到旧的完整文件、要么看到新的完整文件，永远不会看到半写状态。

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use super::error::AppError;

/// 进程内唯一计数，用于生成不会冲突的临时文件名。
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 原子地将 `data` 写入 `path`。
///
/// 实现步骤：
/// 1. 确保父目录存在；
/// 2. 在目标同目录创建唯一临时文件（同文件系统，保证 `rename` 原子性）；
/// 3. 写入数据并 `fsync`（落盘后再替换，避免替换到一个尚未写入磁盘的文件）；
/// 4. `rename` 覆盖目标。
///
/// 任一步失败都会清理临时文件并返回错误，目标文件保持原状。
///
/// 注意：为完整性未 `fsync` 父目录——极端断电下 `rename` 可能未持久化，
/// 但最坏情况是回退到旧的完整检查点，仍远优于损坏的检查点。
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), AppError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| {
        AppError::Io(std::io::Error::new(
            e.kind(),
            format!("创建目录失败 ({}): {}", parent.display(), e),
        ))
    })?;

    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("tmp");
    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = parent.join(format!(
        ".{}.{}.{}.tmp",
        file_name,
        std::process::id(),
        unique
    ));

    let result = (|| -> Result<(), AppError> {
        let mut f = fs::File::create(&tmp_path).map_err(|e| {
            AppError::Io(std::io::Error::new(
                e.kind(),
                format!("创建临时文件失败 ({}): {}", tmp_path.display(), e),
            ))
        })?;
        f.write_all(data).map_err(|e| {
            AppError::Io(std::io::Error::new(
                e.kind(),
                format!("写入临时文件失败 ({}): {}", tmp_path.display(), e),
            ))
        })?;
        f.sync_all().map_err(|e| {
            AppError::Io(std::io::Error::new(
                e.kind(),
                format!("同步临时文件失败 ({}): {}", tmp_path.display(), e),
            ))
        })?;
        drop(f);
        fs::rename(&tmp_path, path).map_err(|e| {
            AppError::Io(std::io::Error::new(
                e.kind(),
                format!("替换文件失败 ({}): {}", path.display(), e),
            ))
        })?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn atomic_write_creates_parent_dirs_and_writes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("checkpoint.json");
        atomic_write(&path, b"{\"ok\":true}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"ok\":true}");
    }

    #[test]
    fn atomic_write_overwrites_existing_content() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("summary.md");
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new content").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new content");
    }

    #[test]
    fn atomic_write_leaves_no_temp_files() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        atomic_write(&path, b"data").unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left behind: {:?}", leftovers);
    }
}
