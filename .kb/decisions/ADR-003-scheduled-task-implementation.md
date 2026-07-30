---
type: decision
status: accepted
title: 定时任务功能架构设计（实现方案）
tags: [scheduled-task, architecture, design, implementation]
author: architect
created: 2026-07-26
---

# ADR-003: 定时任务功能实现架构设计

## 背景

基于 ADR-001 的高层设计，需要在 dev-assistant-rs 中具体实现定时任务功能。现有代码库已有 `TaskOrchestrator`（编排式任务）、`ToolRegistry`（工具注册中心）、`Task` 数据结构等，定时任务需要与这些现有模块良好集成。

## 需求

- 支持 cron 表达式调度（如 `0 */1 * * *` 每小时）
- 支持固定间隔调度（如每 30 分钟）
- 支持一次性延迟任务（如 5 分钟后执行）
- 支持任务 CRUD（通过工具接口）
- 支持任务执行记录和状态追踪
- 支持失败重试（最多 N 次）
- 后台调度器：随应用启动运行，不阻塞主 REPL 循环
- 两种执行模式：Agent 子代理执行 / Shell 命令执行

## 架构决策

### 决策 1: 使用时间轮 (TimingWheel) + 文件持久化的混合调度

**方案**：内存中使用分层时间轮（精度 1 秒）进行高精度触发，使用 JSONL 文件持久化任务定义和执行记录。

**理由**：
- 纯轮询（每秒扫文件）在任务量增大时性能差
- 时间轮在内存中维护即将到期的任务，触发延迟低（O(1) 复杂度）
- JSONL 持久化与现有 `persist/` 模块风格一致，无额外依赖
- 应用重启时从文件恢复所有未完成的任务

### 决策 2: 调度器与执行器分离

**方案**：`Scheduler` 负责时间计算、任务触发和状态管理；`ScheduledTaskExecutor` 负责实际执行任务逻辑。

**理由**：
- 职责单一，便于测试
- 执行器可以按任务类型派发（Agent 任务走 `spawn_subagent`，命令任务走 `exec_command`）
- 未来可扩展执行器类型（如 HTTP 回调、Webhook）

### 决策 3: 通过工具接口暴露 CRUD 操作

**方案**：新增 4 个工具：`schedule_task`、`unschedule_task`、`list_scheduled_tasks`、`get_scheduled_task_logs`，注册到 `ToolRegistry`。

**理由**：
- 与现有工具系统一致，Agent 可通过自然语言调用
- 工具自动获得安全评估和审批支持
- 无需新增 CLI 命令或 API 端点

### 决策 4: 任务定义使用 Handler 插件化模式

**方案**：定义 `ScheduledTaskHandler` trait，支持 `AgentTask` 和 `CommandTask` 两种内置实现，未来可扩展。

**理由**：
- 与现有 `ToolHandler` 模式一致
- 新增任务类型无需修改核心调度逻辑
- 便于单元测试（MockHandler）

### 决策 5: 调度器作为后台 tokio 任务运行

**方案**：在 `App::run()` 中启动一个 `tokio::spawn` 后台任务运行调度器主循环，不阻塞主 REPL 交互。

**理由**：
- 与应用已有的异步运行时一致
- 不阻塞用户交互
- 通过 `Arc<Mutex<SchedulerState>>` 共享状态，线程安全

### 决策 6: 使用乐观锁（版本号）防止重复执行

**方案**：每个 `ScheduledTask` 维护 `version` 字段，执行前 CAS 更新状态。

**理由**：
- 未来可能支持多实例部署
- 轻量，无需分布式锁
- 与 ADR-001 决策 4 一致

## 非功能性设计

| 指标 | 目标 |
|------|------|
| 调度精度 | ±1 秒 |
| 单实例任务容量 | 10,000+ 活跃任务 |
| 持久化 | JSONL 文件，路径 `.dev-assistant-store/scheduled_tasks/` |
| 启动恢复 | 重启时自动从文件恢复未完成任务 |
| 可观测 | 执行日志 + 任务状态查询工具 |

## 与现有模块的关系

```
ToolRegistry (新增工具)
    ↓
Scheduler (后台 tokio task)
    ├── TimingWheel (内存时间轮)
    ├── TaskStore (JSONL 持久化)
    └── Executor
        ├── AgentTask → spawn_subagent
        └── CommandTask → exec_command
```

## 备选方案

- **方案 A（纯文件轮询）**：每秒扫描 JSONL 文件，任务量大时性能差，放弃。
- **方案 B（使用 cron 库）**：`tokio-cron-scheduler` 等第三方库，但灵活性不足，且与现有架构风格不一致。
- **方案 C（集成到 TaskOrchestrator）**：`TaskOrchestrator` 是编排式任务（依赖图），与定时任务（时间触发）语义不同，强行耦合会破坏单一职责。