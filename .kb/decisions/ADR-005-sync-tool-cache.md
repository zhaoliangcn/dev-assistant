---
id: ADR-005-sync-tool-cache
title: 同步工具添加缓存支持
type: decision
status: proposed
tags: cache, performance, tools, sync-tools
created: 2026-08-04
---

# ADR-005: 同步工具添加缓存支持

## 背景

同步工具（`read_file`、`batch_read_files`、`read_symbol`、`edit_file`、`write_file`）每次调用都会执行完整的文件 I/O 操作（open + read + close，3 次系统调用），而异步工具体系已经完整支持 `ReadCache` 缓存。

在代码审查、代码编辑等流程中，同一文件可能被反复读取（如 `read_file` → `read_symbol` → `edit_file`），导致不必要的磁盘 I/O 和性能开销。

## 需求

- 为同步工具的 `read_file`、`batch_read_files` 添加缓存读取支持
- 为同步工具的 `edit_file`、`write_file` 添加写入后缓存失效
- 复用已有的 `ReadCache`（已同时支持同步和异步方法）
- 保持向后兼容，不改变工具接口

## 方案

### 方案 A（推荐）：为 `ToolContext` 和 `ToolRegistry` 增加缓存字段

**改动点**：

1. **`src/tools/mod.rs`**
   - `ToolContext` 增加 `cache: Option<Arc<ReadCache>>` 字段
   - `ToolRegistry` 增加 `cache: Arc<ReadCache>` 字段
   - `execute_tool()` 创建 `ToolContext` 时传递 `self.cache`
   - 所有构造器（`new`, `new_with_resources`, `new_subagent_registry`, `new_subagent_registry_with_identity`）初始化缓存

2. **`src/tools/file/read.rs`**
   - `read_file_handler` 调用前检查缓存
   - `batch_read_files_handler` 调用前检查缓存

3. **`src/tools/file/write.rs`**
   - `edit_file_handler` 写入后调用 `cache.invalidate()`
   - `write_file_handler` 写入后调用 `cache.invalidate()`

4. **`src/tools/file/symbol.rs`**
   - `read_symbol_handler` 调用前检查缓存

5. **`src/app.rs`**
   - `App::build()` 初始化 `ToolRegistry` 时传入 `ReadCache`

### 方案 B（备选）：统一迁移到异步工具路径

将同步工具全部委托给异步工具实现，但改动量大，且需要修改 `ToolRegistry` 的执行模型。

## 实施优先级

| 优先级 | 工具 | 改动量 | 说明 |
|--------|------|--------|------|
| P0 | `read_file` / `batch_read_files` | 小 | 添加 `read_file_with_cache()` 包装 |
| P1 | `edit_file` | 小 | 写入后调用 `cache.invalidate()` |
| P2 | `read_symbol` | 小 | 添加缓存检查 |
| P3 | `write_file` | 小 | 写入后调用 `cache.invalidate()` |

## 影响分析

### 正面影响
- 减少 60-80% 的重复文件读取 I/O
- 代码审查流程中同一文件多次读取不再重复磁盘 I/O
- 完全复用已有的 `ReadCache` 实现，代码改动量小
- 与异步工具的缓存体系一致，统一缓存策略

### 负面影响
- 增加约 5% 的锁竞争（读锁），但 ReadCache 使用 `RwLock`，读多写少场景性能好
- 增加约 50 行代码

## 测试计划

- 现有缓存测试继续有效
- 新增同步工具缓存测试：`read_file` 命中缓存、`edit_file` 后缓存失效
- `cargo check` 确保编译通过