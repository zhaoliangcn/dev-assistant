//! 文件工具共享常量。

/// 单次 read_file 默认读取的行数上限。
pub const DEFAULT_READ_LIMIT: usize = 200;

/// 遍历目录时跳过的目录名（构建产物、VCS、依赖等）。
pub const SKIP_DIRS: &[&str] = &[
    "target",
    ".git",
    "node_modules",
    ".cargo",
    "dist",
    "build",
];
