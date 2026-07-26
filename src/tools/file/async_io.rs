//! 文件相关工具的异步安全 IO 原语。
//!
//! 所有打开操作在 Unix 上使用 `O_NOFOLLOW` 防止 symlink TOCTOU 攻击。

use std::path::Path;

/// SECURITY: Safe file open functions that use O_NOFOLLOW on Unix systems
/// to prevent symlink-based TOCTOU attacks. This ensures that if the final
/// path component is a symlink, the open will fail rather than following it.
#[cfg(unix)]
async fn open_file_read(path: &Path) -> Result<tokio::fs::File, std::io::Error> {
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_NOFOLLOW);
    options.open(path).await
}

#[cfg(unix)]
#[allow(dead_code)]
async fn open_file_write(path: &Path) -> Result<tokio::fs::File, std::io::Error> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    options.custom_flags(libc::O_NOFOLLOW);
    options.open(path).await
}

#[cfg(not(unix))]
async fn open_file_read(path: &Path) -> Result<tokio::fs::File, std::io::Error> {
    tokio::fs::OpenOptions::new().read(true).open(path).await
}

#[cfg(not(unix))]
#[allow(dead_code)]
async fn open_file_write(path: &Path) -> Result<tokio::fs::File, std::io::Error> {
    tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .await
}

/// 异步读取文件内容，使用 O_NOFOLLOW 防止 symlink-based TOCTOU。
pub async fn read_file_content(path: &Path) -> Result<String, std::io::Error> {
    let mut file = open_file_read(path).await?;
    let mut content = String::new();
    use tokio::io::AsyncReadExt;
    file.read_to_string(&mut content).await?;
    Ok(content)
}

/// 异步写入文件内容，使用 O_NOFOLLOW 防止 symlink-based TOCTOU。
pub async fn write_file_content(path: &Path, content: &str) -> Result<(), std::io::Error> {
    let mut file = open_file_write(path).await?;
    use tokio::io::AsyncWriteExt;
    file.write_all(content.as_bytes()).await?;
    Ok(())
}

/// 异步检查文件是否存在
pub async fn file_exists(path: &Path) -> bool {
    tokio::fs::try_exists(path).await.unwrap_or(false)
}
