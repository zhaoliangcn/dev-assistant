//! 文件相关工具的安全 IO 原语。
//!
//! 所有打开操作在 Unix 上使用 `O_NOFOLLOW` 防止 symlink TOCTOU 攻击。

use std::path::Path;

/// 单次读取文件的最大字节数，防止超大文件导致 OOM。
pub const MAX_READ_BYTES: usize = 100 * 1024 * 1024; // 100MB

/// SECURITY: Safe file open functions that use O_NOFOLLOW on Unix systems
/// to prevent symlink-based TOCTOU attacks. This ensures that if the final
/// path component is a symlink, the open will fail rather than following it.
#[cfg(unix)]
fn open_file_read(path: &Path) -> Result<std::fs::File, std::io::Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_NOFOLLOW);
    options.open(path)
}

#[cfg(unix)]
fn open_file_write(path: &Path) -> Result<std::fs::File, std::io::Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    options.custom_flags(libc::O_NOFOLLOW);
    options.open(path)
}

#[cfg(not(unix))]
fn open_file_read(path: &Path) -> Result<std::fs::File, std::io::Error> {
    std::fs::OpenOptions::new().read(true).open(path)
}

#[cfg(not(unix))]
fn open_file_write(path: &Path) -> Result<std::fs::File, std::io::Error> {
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

/// Read file content with O_NOFOLLOW on Unix to prevent symlink-based TOCTOU.
///
/// 读取上限为 [`MAX_READ_BYTES`]，超过上限时返回显式错误，避免超大文件 OOM。
/// 绝不静默截断：截断的内容若被编辑/追加工具写回，会永久丢失文件尾部数据。
pub fn read_file_content(path: &Path) -> Result<String, std::io::Error> {
    // 先检查真实文件大小，超过上限直接报错，避免读取超大文件导致 OOM。
    let meta = std::fs::metadata(path)?;
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

    let file = open_file_read(path)?;
    let mut content = String::new();
    use std::io::Read;
    // 兜底防护：metadata 与读取之间文件可能被替换/增长，超限即报错而非截断。
    file.take(MAX_READ_BYTES as u64 + 1)
        .read_to_string(&mut content)?;
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

/// Write content to file with O_NOFOLLOW on Unix to prevent symlink-based TOCTOU.
pub fn write_file_content(path: &Path, content: &str) -> Result<(), std::io::Error> {
    let mut file = open_file_write(path)?;
    use std::io::Write;
    file.write_all(content.as_bytes())?;
    Ok(())
}
