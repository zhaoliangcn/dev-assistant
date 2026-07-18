//! restart 工具的执行流程：编译并重启进程。

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process;

use crate::utils::message_level::MessageLevel;

/// 执行 restart 流程：
/// 1. 运行 `cargo build` 验证修改是否编译通过
/// 2. 编译成功后用 `exec()` 替换当前进程（PID 保持不变）
///
/// 返回 `true` 表示需要继续 REPL（构建失败或 exec 失败）；
/// 返回 `false` 表示已经 exec 成功或将要退出。
pub fn perform_restart(
    working_dir: &Path,
    cli_args: &[String],
    emit: &mut dyn FnMut(MessageLevel, String),
) -> bool {
    emit(
        MessageLevel::Info,
        "正在运行 cargo build...".to_string(),
    );

    let build_result = process::Command::new("cargo")
        .arg("build")
        .current_dir(working_dir)
        .status();

    match build_result {
        Ok(status) if status.success() => {
            let exe = match std::env::current_exe() {
                Ok(p) => p,
                Err(e) => {
                    emit(
                        MessageLevel::Error,
                        format!("获取当前可执行文件路径失败: {}。未重启。", e),
                    );
                    return true;
                }
            };

            emit(
                MessageLevel::Success,
                "构建成功，正在重启 (PID 保持不变)...".to_string(),
            );

            // exec() replaces the current process on success (same PID).
            // It only returns on error.
            let exec_err = process::Command::new(&exe)
                .args(cli_args)
                .current_dir(working_dir)
                .exec();

            // If we reach here, exec() failed — show error and exit REPL
            emit(
                MessageLevel::Error,
                format!(
                    "重启失败 (exec 返回错误): {}\n\
                     可执行文件: {}\n\
                     参数: {:?}\n\
                     工作目录: {}\n\
                     请手动运行: {} {}",
                    exec_err,
                    exe.display(),
                    cli_args,
                    working_dir.display(),
                    exe.display(),
                    cli_args.join(" ")
                ),
            );
            false
        }
        Ok(status) => {
            let exit_code = status.code().unwrap_or(-1);
            emit(
                MessageLevel::Error,
                format!("构建失败，退出码: {}。请修复错误后再次尝试重启。", exit_code),
            );
            // Build failed — continue REPL so user can fix and retry
            true
        }
        Err(e) => {
            emit(
                MessageLevel::Error,
                format!("运行 cargo build 失败: {}。请修复错误后再次尝试重启。", e),
            );
            // Build command failed — continue REPL so user can fix and retry
            true
        }
    }
}
