//! 文件相关工具：read / write / edit / glob / list_directory / file_exists。
//!
//! 拆分自原 `file_tools.rs`（836 行单文件），按职责分散到：
//! - [`io`]：安全 IO 原语（O_NOFOLLOW）
//! - [`read`]：`read_file`、`batch_read_files`
//! - [`write`]：`write_file`、`edit_file`
//! - [`search`]：`glob`、`list_directory`、`file_exists`

pub mod async_io;
pub mod async_read;
pub mod async_write;
pub mod io;
pub mod read;
pub mod read_shared;
pub mod search;
pub mod write;

pub use read::{batch_read_files_tool, read_file_tool};
pub use search::{file_exists_tool, glob_tool, list_directory_tool};
pub use write::{edit_file_tool, write_file_tool};
