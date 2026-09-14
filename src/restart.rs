//! restart 工具的执行流程：编译并重启进程。
//!
//! # 平台语义
//!
//! - **Unix**：调用 `exec()` 原地替换当前进程，PID 保持不变。
//!   `SessionLogger`/`SessionStore` 创建文件时设置的 `FD_CLOEXEC`
//!   （`libc::O_CLOEXEC`）保证 `exec()` 后旧 fd 自动关闭，避免资源泄漏。
//! - **Windows**：无 exec 语义，采用 spawn-and-exit——`spawn` 新 exe
//!   子进程（共享同一控制台），当前进程随即 `exit(0)` 让出终端。
//!   PID 会变化；当前进程持有的文件句柄随进程退出释放。

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(not(unix))]
use std::io::Write;
use std::path::Path;
use std::process;

use crate::utils::message_level::MessageLevel;

/// 执行 restart 流程：
/// 1. 运行 `cargo build` 验证修改是否编译通过
/// 2. 编译成功后用 `exec()` 替换当前进程（PID 保持不变）
///
/// `source_root` 是 dev-assistant-rs 的源码根目录（`cargo build` 在此运行），
/// 当 `--project` 指向其他项目时，`source_root` 与 `working_dir` 不同。
/// `working_dir` 是项目工作目录（`exec` 后的进程工作目录）。
///
/// 返回 `true` 表示需要继续 REPL（构建失败或 exec 失败）；
/// 返回 `false` 表示已经 exec 成功或将要退出。
///
/// # 资源安全
///
/// 执行 `exec()` 前无需手动关闭文件句柄，因为 `SessionLogger` 和
/// `SessionStore` 在创建文件时已设置 `FD_CLOEXEC` 标志，`exec()`
/// 后内核会自动关闭这些文件描述符。
pub fn perform_restart(
    source_root: &Path,
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
        .current_dir(source_root)
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

            #[cfg(unix)]
            let restart_msg = "构建成功，正在重启 (PID 保持不变)...";
            #[cfg(not(unix))]
            let restart_msg = "构建成功，正在重启 (新进程将接管终端)...";
            emit(MessageLevel::Success, restart_msg.to_string());

            // exec() replaces the current process on success (same PID).
            // It only returns on error.
            #[cfg(unix)]
            {
                let exec_err = process::Command::new(&exe)
                    .args(cli_args)
                    .current_dir(working_dir)
                    .exec();

                // If we reach here, exec() failed — show error and continue REPL
                // (return true so user can fix the issue and retry)
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
                true
            }

            // Windows: spawn-and-exit. Start the new binary as a child
            // sharing this console, flush stdio, then exit so the child
            // takes over the terminal. PID changes; there is no exec().
            #[cfg(not(unix))]
            {
                match process::Command::new(&exe)
                    .args(cli_args)
                    .current_dir(working_dir)
                    .spawn()
                {
                    Ok(child) => {
                        emit(
                            MessageLevel::Info,
                            format!(
                                "已启动新进程 (PID {})，当前进程退出。",
                                child.id()
                            ),
                        );
                        // 让残余输出先落屏，再交还终端给新进程。
                        let _ = std::io::stdout().flush();
                        let _ = std::io::stderr().flush();
                        process::exit(0);
                    }
                    Err(e) => {
                        emit(
                            MessageLevel::Error,
                            format!(
                                "重启失败 (spawn 返回错误): {}\n\
                                 可执行文件: {}\n\
                                 参数: {:?}\n\
                                 工作目录: {}\n\
                                 请手动运行: {} {}",
                                e,
                                exe.display(),
                                cli_args,
                                working_dir.display(),
                                exe.display(),
                                cli_args.join(" ")
                            ),
                        );
                        true
                    }
                }
            }
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
