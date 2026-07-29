---
type: issue
severity: critical
tags: [security, approval, race-condition, deadlock]
---

# 严重问题 (Critical Issues)

## 1. approval.rs: `PermissionStore` 使用 `RwLock` 但未处理 Poison

**文件**: `src/security/approval.rs`
**位置**: 多处 `RwLock` 的 `read()`/`write()` 调用使用 `.unwrap()`

**问题描述**: 当持有锁的线程 panic 时，`RwLock` 会进入 poison 状态，
后续所有 `.unwrap()` 调用都会 panic，导致整个进程崩溃。

**影响范围**: 所有审批操作（`has_permission`, `add_permission`, `revoke_permission` 等）

**建议修复**: 使用 `.lock()` 替代 `.unwrap()`，或使用 `catch_unwind` 保护。

## 2. session/mod.rs: 日志文件脱敏正则可能绕过

**文件**: `src/session/mod.rs`
**位置**: `sanitize()` 方法中的正则替换链

**问题描述**: 脱敏替换是顺序执行的，但替换后可能形成新的敏感模式。
例如，JWT 替换后如果包含 API Key 模式的片段，后续检查不会再次扫描。

**影响范围**: 日志文件中可能泄露敏感信息

**建议修复**: 使用递归替换或对每个替换后的结果再次执行所有正则检查。

## 3. restart.rs: `exec()` 后 File 描述符可能泄漏

**文件**: `src/restart.rs`
**位置**: `perform_restart` 函数

**问题描述**: 虽然 `SessionLogger` 和 `SessionStore` 设置了 `O_CLOEXEC`，
但 `perform_restart` 函数本身使用 `process::Command::new(&exe).exec()`，
如果调用方在调用 `perform_restart` 前打开了其他文件描述符且未设置 `FD_CLOEXEC`，
这些 fd 会在 exec() 后泄漏。

**影响范围**: 子进程不安全，可能泄漏文件句柄

**建议修复**: 在 `exec()` 前显式关闭所有非必要文件描述符，或使用 `libc::close_range()`。

## 4. client.rs: `Mutex::lock().unwrap()` 可能导致主线程 panic

**文件**: `src/llm/client.rs`
**位置**: `active_idx` 的 `lock().unwrap()` 调用

**问题描述**: 如果 `Mutex` 被 poison，`unwrap()` 会导致 panic。
由于 `LlmClient` 通过 `Arc` 在多个子 Agent 间共享，一个子 Agent 的 panic
会 poison 所有共享的 Mutex，影响所有子 Agent。

**影响范围**: 子 Agent 系统中一个 Agent 失败会导致所有 Agent 无法使用 LLM

**建议修复**: 使用 `lock().unwrap_or_else(|e| e.into_inner())` 恢复。