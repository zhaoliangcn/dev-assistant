# Dev-Assistant-RS 代码自查报告

**日期**: 2026-08-04  
**范围**: 全部 `src/` 目录（Rust 源代码）  
**检查项**: 编译警告、测试失败、代码质量、安全、架构

---

## 1. 编译状态

### 1.1 编译通过 ✅
- `cargo check` 编译通过，无 error
- 存在 **7 个 warning**（均为 `dead_code`）

### 1.2 编译警告详情

| # | 文件 | 行号 | 类型 | 说明 | 优先级 |
|---|------|------|------|------|--------|
| W1 | `src/agent/context.rs` | 159 | `dead_code` | `ContextBudgetManager::should_compress` 方法从未使用 | P2 |
| W2 | `src/agent/context.rs` | 167 | `dead_code` | `ContextBudgetManager::set_memory_tokens` 方法从未使用 | P2 |
| W3 | `src/agent/context.rs` | 244 | `dead_code` | `ContextManager::set_memory_tokens` 方法从未使用 | P2 |
| W4 | `src/agent/summary.rs` | 64 | `dead_code` | `LayeredSummaries` 结构体从未被构造 | P2 |
| W5 | `src/agent/summary.rs` | 78 | `dead_code` | `session_id` 字段从未被读取 | P2 |
| W6 | `src/agent/summary.rs` | 101 | `dead_code` | `session_id()`, `root()`, `load_final()`, `load_all()` 方法从未使用 | P2 |
| W7 | `src/orchestrator/checkpoint.rs` | 118 | `dead_code` | `kb_root` 字段从未被读取 | P2 |
| W8 | `src/orchestrator/checkpoint.rs` | 276 | `dead_code` | `rebuild_context_from_checkpoint` 方法从未使用 | P2 |

**建议**: 所有 warning 均为 `dead_code`，建议在下一轮清理中移除或添加 `#[allow(dead_code)]` 明确标注意图。

---

## 2. 测试状态

### 2.1 总体
- **通过**: 351 个
- **失败**: 6 个（均在 `src/tools/file/symbol.rs`）
- **跳过**: 0

### 2.2 失败测试分析

| # | 测试名 | 断言 | 期望值 | 实际值 | 根因分析 |
|---|--------|------|--------|--------|----------|
| F1 | `scan_trait` | `symbols[0].name` | `"Into"` | `"into"` | 泛型 trait 名称解析问题：`pub trait Into<T>` 中泛型参数 `<T>` 导致名称截断或解析异常 |
| F2 | `scan_const` | `symbols[0].name` | `"MAX"` | `"MAX:"` | 冒号处理：`const MAX: usize = 1024` 中类型注解的冒号被计入名称 |
| F3 | `scan_module` | `symbols.len()` | 1 | 0 | 分号结尾的模块声明 `pub mod foo;` 未被识别 |
| F4 | `pub_struct_with_visibility` | `symbols.len()` | 1 | 0 | `pub struct FooBar` 中 `pub` 可见性修饰符导致解析失败 |
| F5 | `async_function` | `symbols.len()` | 1 | 0 | `pub async fn` 中 `async` 关键字未被识别 |
| F6 | `pub_crate_struct` | `symbols.len()` | 1 | 0 | `pub(crate)` 复合可见性修饰符导致解析失败 |

**根因总结**: `scan_symbols` 函数（`src/tools/file/symbol.rs`）的正则/行解析逻辑存在以下缺陷：

1. **可见性修饰符未处理**：只处理了无修饰符的 `fn`/`struct`/`enum` 等，但 `pub fn`、`pub async fn`、`pub(crate) struct` 等均未被识别
2. **泛型参数干扰**：`trait Into<T>` 中 `<T>` 导致名称解析为小写 `into`
3. **冒号处理缺陷**：`const MAX: usize` 中类型注解的冒号被包含在名称中
4. **分号模块遗漏**：`pub mod foo;` 这种分号结尾的模块声明未被识别

**建议修复**: 在 `scan_symbols` 中添加对 `pub`、`pub(crate)`、`pub(super)`、`async`、`unsafe` 等修饰符的跳过逻辑，并修复上述解析缺陷。

---

## 3. 代码质量分析

### 3.1 架构质量 ✅
- **模块化清晰**: `agent/`、`tools/`、`llm/`、`scheduler/`、`security/` 等模块职责分明
- **单层抽象**: main.rs 仅做入口，业务逻辑在 app.rs 和 repl.rs 中
- **依赖注入**: 使用 `Arc<SecurityPolicy>` 共享安全策略，避免生命周期问题
- **资源管理**: `Resources` 容器用于依赖注入，设计合理

### 3.2 代码异味

#### 3.2.1 `src/app.rs` — 函数过长
- `run_interactive()` 方法约 340 行，包含 slash 命令分发、模型切换、历史查看等复杂逻辑
- 建议将 `/model`、`/history`、`/grep`、`/diff` 等命令处理提取为独立方法

#### 3.2.2 `src/repl.rs` — 命令处理分散
- `handle_slash` 在 `repl.rs` 中，但 `/model`、`/history` 等命令在 `app.rs` 中处理
- 命令分发逻辑分散在两处，维护成本高
- 建议统一为命令注册表模式

#### 3.2.3 `src/agent/mod.rs` — Agent 类过大
- `Agent` 结构体约 1500 行，承担了太多职责（LLM 交互、工具调用、流水线、摘要管理）
- 建议将流水线逻辑（`run_pipeline` 及相关方法）提取到独立的 `PipelineRunner` 中

#### 3.2.4 `src/security/mod.rs` — 函数过长
- `evaluate_command` 方法约 130 行，包含大量正则匹配和条件判断
- 建议拆分为多个策略函数（`evaluate_rm_rf`、`evaluate_sudo`、`evaluate_shell` 等）

### 3.3 重复代码
- `src/agent/identity.rs` 中 `default_tools()` 方法为每个身份重复了几乎相同的工具列表
- 建议：定义公共工具集 + 各身份差异集，通过差集合并

---

## 4. 安全隐患

### 4.1 已存在的安全机制 ✅
- **路径遍历防护**: `normalize_path()` + `is_child_of()` + 符号链接检测
- **命令风险评估**: 基于正则的 `DangerLevel` 评估（rm -rf、sudo、shell 注入等）
- **审批机制**: Critical/High 级别操作需要用户确认
- **文件描述符安全**: `FD_CLOEXEC` 标志防止 exec 后泄漏

### 4.2 潜在风险

#### 4.2.1 `kb_store` 路径规范化 ✅（已修复）
- 当前代码已添加 `trim_start_matches(".kb/")` 处理，防止路径重复拼接
- 但 `update_index` 参数未验证，若传入 `true` 且路径被篡改可能导致索引不一致

#### 4.2.2 `exec_command` 的 sh -c 绕过
- 虽然 `sh -c` 方式仍会经过安全评估，但 `sh` 本身是白名单命令
- 建议：对 `sh -c` 的内容做更严格的危险命令检测

---

## 5. 性能评估

### 5.1 优秀实践 ✅
- **ReadCache**: 文件读取缓存，避免重复 IO
- **异步文件工具**: `AsyncReadFileTool` 等不阻塞主循环
- **原子计数器**: `AtomicUsize` 替代 `Mutex` 用于模型索引

### 5.2 潜在问题

#### 5.2.1 `scan_symbols` 实现
- 当前实现为纯行扫描 + 括号匹配，对复杂 Rust 语法的支持有限
- 建议：考虑使用 `syn` crate 替代手写解析器，或在现有基础上增加更多测试用例

#### 5.2.2 会话日志存储
- 日志文件存储在 `.dev-assistant-store/logs/`，但会话 session 日志文件仍在项目根目录
- 大量 session 日志文件（已观察到 100+ 个）会污染项目根目录

---

## 6. 测试覆盖

### 6.1 测试覆盖较好的模块
- `src/agent/` — 有子代理创建、上下文管理、摘要等测试
- `src/security/mod.rs` — 路径遍历、命令评估、审批流程覆盖较好
- `src/prompt.rs` — 系统提示词构建的各个分支都有测试
- `src/skills/mod.rs` — 技能解析、发现、格式化有完整测试

### 6.2 测试覆盖不足的模块
- `src/tools/file/` — 文件工具（read/write/edit）缺乏单元测试
- `src/scheduler/` — 调度器各组件缺乏单元测试
- `src/hooks/` — Hook 执行器缺乏网络/超时场景测试
- `src/web/` — Web 界面缺乏集成测试

---

## 7. 总结与建议

### 7.1 必须修复（P0）
1. **6 个测试失败**：修复 `scan_symbols` 解析逻辑，处理 pub/async/泛型/冒号/分号模块
2. 这是当前最严重的问题，影响 `read_symbol` 工具的可靠性

### 7.2 建议修复（P1）
1. **清理 dead_code 警告**：移除或标注未使用的代码
2. **独立流水线逻辑**：从 `Agent` 中提取 `PipelineRunner` 模块
3. **统一命令分发**：将 slash 命令处理集中到命令注册表模式

### 7.3 优化建议（P2）
1. **减少重复代码**：`default_tools()` 使用公共工具集 + 差异集
2. **拆分过长函数**：`run_interactive()`、`evaluate_command()` 等
3. **增加测试覆盖**：文件工具、调度器、Hook 执行器
4. **会话日志存储**：统一到 `.dev-assistant-store/logs/` 目录