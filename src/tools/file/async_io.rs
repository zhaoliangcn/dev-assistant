//! 文件相关工具的异步安全 IO 原语。
//!
//! 所有打开操作在 Unix 上使用 `O_NOFOLLOW` 防止 symlink TOCTOU 攻击。

use std::path::Path;

/// 单次读取文件的最大字节数，防止超大文件导致 OOM。
pub const MAX_READ_BYTES: usize = 100 * 1024 * 1024; // 100MB

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
///
/// 读取上限为 [`MAX_READ_BYTES`]，超过上限时返回显式错误，避免超大文件 OOM。
/// 绝不静默截断：截断的内容若被编辑/追加工具写回，会永久丢失文件尾部数据。
pub async fn read_file_content(path: &Path) -> Result<String, std::io::Error> {
    // 先检查真实文件大小，超过上限直接报错，避免读取超大文件导致 OOM。
    let meta = tokio::fs::metadata(path).await?;
    if meta.len() > MAX_READ_BYTES as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            format!(
                "文件大小 {} 字节超过读取上限 {} 字节（{}MB），拒绝读取；如需处理请先拆分文件",
                meta.len(),
                MAX_READ_BYTES,
                MAX_READ_BYTES / (1024 * 1024)
            ),
        ));
    }

    let mut file = open_file_read(path).await?;
    let mut content = String::new();
    use tokio::io::AsyncReadExt;
    // 兜底防护：metadata 与读取之间文件可能被替换/增长，超限即报错而非截断。
    file.take(MAX_READ_BYTES as u64 + 1)
        .read_to_string(&mut content)
        .await?;
    if content.len() > MAX_READ_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            format!(
                "文件实际读取 {} 字节超过读取上限 {} 字节，拒绝返回截断内容",
                content.len(),
                MAX_READ_BYTES
            ),
        ));
    }
    Ok(content)
}

/// 异步写入文件内容，使用 O_NOFOLLOW 防止 symlink-based TOCTOU。
pub async fn write_file_content(path: &Path, content: &str) -> Result<(), std::io::Error> {
    let mut file = open_file_write(path).await?;
    use tokio::io::AsyncWriteExt;
    file.write_all(content.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

/// 异步检查文件是否存在
pub async fn file_exists(path: &Path) -> bool {
    tokio::fs::try_exists(path).await.unwrap_or(false)
}
