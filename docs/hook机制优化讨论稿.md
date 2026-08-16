# Hook 机制优化讨论稿

> 日期：2026-08-16
> 范围：`src/hooks/`（config.rs / shell.rs / mod.rs / error.rs）与调用点 `src/app.rs:128-146`
> 结论：当前实现是"单事件、单类型、串行阻塞"的最小可用版本，有 2 个真 bug，其余是设计缺口。

---

## 现状

`.dev-assistant/hooks.yaml`（项目级）+ `~/.dev-assistant/hooks.yaml`（全局级）→ 合并去重（项目级优先）→ 启动时按 `(priority, name)` 串行执行 shell 命令 → stdout 截断（默认 4096B）→ 拼成 `<HOOKS>` 块作为一条 system 消息注入（紧跟系统提示词之后）。

## 一、待修复 Bug

### Bug 1：`truncate_output` 会在多字节字符处 panic

```rust
let mut truncated = s[..max_bytes].to_string();  // shell.rs:124
```

`&str` 按字节切片，若 `max_bytes` 落在 UTF-8 字符中间直接 panic（release 也一样）。hook 输出中文时命中概率不低。应改为按字符边界截断：

```rust
let mut end = max_bytes;
while !s.is_char_boundary(end) { end -= 1; }
let mut truncated = s[..end].to_string();
```

### Bug 2：`event` 字段既被忽略、又能在 debug 下炸掉启动

`config.rs:94-97` 只有一条 `debug_assert!` 断言全是 `session-start`；而 `execute_session_start()`（mod.rs:64）**不按 event 过滤，把所有 hook 全部执行**。后果：

- debug 构建：任何非 `session-start` 的 hook 直接 panic 启动；
- release 构建：`session-end` 之类的 hook 会在会话开始时就跑。

修法：执行前按 event 过滤，未知 event 加载时报错（fail loud，项目已有此惯例）：

```rust
// 在 load 阶段校验，而不是 debug_assert
if let Some(bad) = hooks.iter().find(|h| h.event != "session-start") {
    return Err(HookError::Config(format!("Unsupported event '{}'", bad.event)));
}
```

## 二、架构缺口（按影响排序）

| # | 缺口 | 现状问题 | 优化建议 |
|---|------|---------|---------|
| 1 | **事件模型硬编码** | 只有一个 `session-start`，`event` 是自由字符串 | 定义 `HookEvent` 枚举（session-start / session-end / pre-tool / post-tool / user-input…），只先实现 1-2 个新事件，把事件分派做进 `HookManager`，而不是靠 `debug_assert` 兜底 |
| 2 | **hook 拿不到任何上下文** | 不知道工作目录、事件、payload | 给子进程传 `DEV_ASSISTANT_EVENT`、`DEV_ASSISTANT_WORKDIR` 等环境变量 + stdin 传 JSON payload（`{event, cwd, ...}`），hook 才能做"感知型"工作（如 pre-tool 检查） |
| 3 | **串行 + 阻塞启动** | 每个 hook 最多 5s，N 个就是 5N 秒线性叠加，拖慢启动 | 用 tokio（项目已有）并行执行，整体等待取 `min(总超时)`；或至少给 `execute_session_start` 加总预算 |
| 4 | **轮询忙等** | `shell.rs:39-83` 用 50ms sleep 轮询 `try_wait` | 用 `tokio::process::Command` + `tokio::time::timeout`，或同步版用 `wait_with_output` + 线程 + channel，消除忙等 |
| 5 | **注入格式脆弱、无失败反馈** | 失败 hook 仅打日志，模型看不到"哪个 hook 失败、为什么"；硬编码 XML 风格标签 | 输出里带每个 hook 的状态（`<HOOK name status="ok|failed" reason="...">`），让模型能感知并补救 |
| 6 | **死代码字段** | `wrap_tag`、`success`、`type_` 全是 `#[allow(dead_code)]` | 要么实现（`wrap_tag` 自定义注入标签），要么删掉。`type_` 目前只有 shell 一种，为将来 `builtin` 类型留分派 |
| 7 | **无总上下文预算** | 每个 hook 截断 4096B，10 个 hook 就是 40KB 灌进上下文 | 加 `max_total_bytes`（默认如 8KB），超出按 priority 从低到高丢弃 |
| 8 | **无安全边界** | `hooks.yaml` 里的命令就是任意 shell 命令，克隆恶意仓库即执行 | 项目已有 SecurityPolicy / ApprovalManager，项目级 hook 可走一次审批；或支持 `trusted: true` 标记 |
| 9 | **无 dry-run / 缓存** | 无法预览会执行什么；每次启动全跑 | 加 `--hooks-dry-run` 打印将执行的 hook；纯静态 hook 可按 (命令+参数+文件 mtime) 缓存 |
| 10 | **不可被模型按需调用** | hook 只在启动时跑一次 | 注册一个 `run_hook` 工具，模型可在会话中按需触发（如"跑 pre-commit 检查"），复用同一执行器 |

## 三、建议的实施路线

```
P0（1 小时内，纯修复）: 修 Bug 1、Bug 2 → cargo test 全绿
P1（半天，价值最高）  : HookEvent 枚举 + 按 event 分派 + env/stdin 上下文 + 并行执行（tokio）
P2（可选）           : 失败反馈进注入、总预算、run_hook 工具、dry-run
```

**P0 是必须做的**（两个都是会导致实际运行出错的 bug），**P1 的"并行执行 + 上下文传递"收益最大**——它把 hook 从"启动时跑一下"升级成"生命周期事件钩子"。

## 四、实施进度（2026-08-16）

### ✅ 已完成

| 项 | 内容 | 涉及文件 |
|----|------|---------|
| P0-Bug1 | `truncate_output` 改为按 UTF-8 字符边界截断（`is_char_boundary` 回退），新增 `truncation_respects_utf8_boundary` 测试 | `src/hooks/shell.rs` |
| P0-Bug2 | 引入 `HookEvent` 枚举（kebab-case serde 校验，未知事件在 YAML 解析阶段报错）；删除失效的 `debug_assert!`，改为对未分派事件告警 | `src/hooks/config.rs` |
| P1-事件分派 | `HookManager::execute_session_start` 仅分派 `session-start` 事件，新增 `non_session_start_events_are_skipped` 测试 | `src/hooks/mod.rs` |
| P1-并行执行 | 改用 `std::thread::scope` 并行执行 hooks（join 顺序保持 priority 顺序），新增 `parallel_execution_preserves_priority_order` 测试 | `src/hooks/mod.rs` |
| P1-上下文传递 | `execute_shell_hook` 增加 `event` / `workdir` 参数，向子进程注入 `DEV_ASSISTANT_EVENT` / `DEV_ASSISTANT_WORKDIR` / `DEV_ASSISTANT_HOOK_NAME` 环境变量，新增 `hook_receives_context_env_vars` 测试 | `src/hooks/shell.rs` |
| P2-失败反馈进注入 | `execute_event` 成功/失败都进入注入内容，`<HOOK status="ok|failed" reason="...">`；name/detail 均做 XML 转义防结构破坏；新增 3 个测试 | `src/hooks/mod.rs` |
| P2-总字节预算 | `hooks.yaml` 顶层 `max_total_bytes`（默认 8KB，项目级覆盖全局级），超限按 priority 从低到高丢弃；新增 2 个测试 | `src/hooks/config.rs` `src/hooks/mod.rs` |
| P2-stdin payload | 向 hook 进程 stdin 写入 `{"event","cwd","name"}` JSON 后关闭；新增 `hook_receives_stdin_json_payload` 测试 | `src/hooks/shell.rs` |
| P2-dry-run | `--hooks-dry-run` CLI 参数 + `HookManager::dry_run()` 预览（事件/命令/参数/优先级/超时/预算） | `src/hooks/mod.rs` `src/main.rs` |
| P2-run_hook 工具 | 注册 `run_hook` meta 工具，模型可按 event/name 按需触发 hooks；新增 `HookEvent::parse` | `src/tools/meta_tools.rs` `src/tools/mod.rs` `src/hooks/config.rs` |
| P2-session-end 事件 | `execute_session_start` 泛化为 `execute_event(event, name_filter)`；App 持有 HookManager，会话结束时执行 `session-end` hooks（仅记录日志，不影响退出码） | `src/hooks/mod.rs` `src/app.rs` |
| P3-pre/post-tool 接线 | Agent 主循环 `process_tool_calls` 在工具执行前调用 `run_pre_tool`（输出以 `DENY` 开头即拦截，hook 失败 fail-open 放行），执行后调用 `run_post_tool`（仅日志）；新增 `execute_tool_hook`（payload 带 tool/arguments + `DEV_ASSISTANT_TOOL_NAME` env）；抽公共 `run_parallel`；HookManager 以 Arc 在 App 与 Agent 间共享；新增 6 个测试 | `src/agent/mod.rs` `src/hooks/mod.rs` `src/hooks/shell.rs` `src/app.rs` |

**验证**：`cargo test` 413 通过（hooks 模块 25 个测试全绿）；`cargo clippy` 无 hooks 相关新警告。

### ⏳ 待办（后续）

- `user-input` 事件接线（枚举已定义，run_hook 工具已可按需触发，但尚未接入用户输入路径）
- 项目级 hook 安全审批（对接 SecurityPolicy / ApprovalManager，防克隆恶意仓库即执行）
- hook 输出缓存（按 命令+参数+文件 mtime）
- dry-run 支持按事件过滤预览
- `pre-tool` 拦截原因写入会话日志（当前仅作为工具失败结果返回模型）

---

## 相关文件

- `src/hooks/mod.rs` — HookManager：加载配置、执行 hooks、收集输出
- `src/hooks/config.rs` — YAML 配置加载（项目级 + 全局级合并）
- `src/hooks/shell.rs` — shell hook 执行器（进程 spawn、超时、stdout 捕获）
- `src/hooks/error.rs` — 专用错误类型
- `src/app.rs:128-146` — 调用点：启动时执行并注入 system 消息
