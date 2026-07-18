//! 文件相关工具的安全 IO 原语。
//!
//! 所有打开操作在 Unix 上使用 `O_NOFOLLOW` 防止 symlink TOCTOU 攻击。

use std::path::Path;

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
pub fn read_file_content(path: &Path) -> Result<String, std::io::Error> {
    let mut file = open_file_read(path)?;
    let mut content = String::new();
    use std::io::Read;
    file.read_to_string(&mut content)?;
    Ok(content)
}

/// Write content to file with O_NOFOLLOW on Unix to prevent symlink-based TOCTOU.
pub fn write_file_content(path: &Path, content: &str) -> Result<(), std::io::Error> {
    let mut file = open_file_write(path)?;
    use std::io::Write;
    file.write_all(content.as_bytes())?;
    Ok(())
}
