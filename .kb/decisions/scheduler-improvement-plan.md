---
title: 定时任务模块改进计划
type: decision
tags: [scheduler, improvement, architecture]
created: 2026-08-01
status: draft
---

# 定时任务模块改进计划

## 问题清单

| 优先级 | 问题 | 严重程度 | 影响范围 |
|--------|------|----------|----------|
| P0 | Agent 任务执行是模拟实现，未真正调用 spawn_subagent | 高 | 功能完整性 |
| P1 | 缺少单个任务的暂停/恢复功能 | 中 | 用户体验 |
| P1 | 缺少任务编辑功能（更新调度参数、执行模式等） | 中 | 用户体验 |
| P2 | 时间轮溢出队列需要线性扫描，效率低 | 中 | 性能 |
| P2 | cron 解析不支持特殊字符 (L, W, #) | 低 | 兼容性 |
| P2 | list_scheduled_tasks 工具 handler 使用 now_or_never() hack | 低 | 代码质量 |
| P3 | Paused 状态在调度循环中未使用 | 低 | 代码完整性 |

## 改进方案

### P0: 实现真正的 Agent 任务执行

**当前状态**: `execute_agent_task()` 返回模拟结果，未真正调用子代理。

**目标**: 当定时任务触发时，通过 `spawn_subagent` 创建子代理执行指令。

**方案**:
```rust
// handler.rs 中的 execute_agent_task
async fn execute_agent_task(
    task_id: &str,
    instruction: &str,
    max_retries: u32,
) -> Result<String, AppError> {
    // 1. 创建子代理执行器
    // 2. 通过 spawn_subagent 运行指令
    // 3. 收集结果输出
    // 4. 返回执行结果
}
```

**注意**: 这需要子代理有独立的工具和上下文，防止与主 Agent 冲突。

### P1: 单个任务的暂停/恢复

**当前状态**: 只能暂停/恢复整个调度器，无法操作单个任务。

**目标**: 在 Scheduler 和 store 层添加暂停/恢复单个任务的能力。

**新增 API**:
```rust
impl Scheduler {
    /// 暂停单个任务（不执行但保留调度位置）
    pub fn pause_task(&self, task_id: &str) -> Result<bool, AppError>;
    
    /// 恢复单个任务
    pub fn resume_task(&self, task_id: &str) -> Result<bool, AppError>;
}
```

**工具接口**:
- `pause_task(task_id)` — 暂停单个任务
- `resume_task(task_id)` — 恢复单个任务

### P1: 任务编辑功能

**目标**: 允许用户更新已创建任务的参数。

**新增 API**:
```rust
impl Scheduler {
    /// 更新任务（支持更新调度类型、执行模式、标签等）
    pub fn update_task(&self, task_id: &str, update: TaskUpdate) -> Result<bool, AppError>;
}
```

**数据结构**:
```rust
pub struct TaskUpdate {
    pub name: Option<String>,
    pub schedule: Option<ScheduleType>,
    pub mode: Option<TaskExecutionMode>,
    pub max_retries: Option<u32>,
    pub tags: Option<Vec<String>>,
    pub timeout_secs: Option<u64>,
}
```

### P2: 时间轮优化

**当前状态**: 单层时间轮（3600槽），超过1小时的任务放入溢出队列，每次tick扫描所有溢出条目。

**目标**: 优化溢出队列处理，减少不必要的扫描。

**方案**:
1. 将溢出队列改为**按时间排序的 BTreeMap**，每个 tick 只检查最早到期的任务
2. 或者实现**分层时间轮**（小时级、分钟级），减少溢出

### P2: cron 解析增强

**当前状态**: 不支持 L（最后）、W（最近工作日）、#（第N个星期X）等特殊字符。

**目标**: 增加对常见特殊字符的支持。

**方案**:
- L: 月份的最后一天 / 星期的最后一天
- W: 最接近指定日期的工作日
- #: 第N个星期X（如 `1#2` = 第二个星期一）

### P2: 消除 now_or_never hack

**当前状态**: `list_scheduled_tasks_handler` 使用 `now_or_never()` 获取异步结果。

**目标**: 为工具 handler 提供异步执行能力，或改用同步 API。

**方案**:
1. 将 `get_all_tasks` 改为同步方法（存储层读缓存）
2. 或让工具 handler 支持异步执行

### P3: 利用 Paused 状态

**当前状态**: `Paused` 状态已定义，但引擎中只检查 `Active`。

**目标**: 在调度循环中尊重 Paused 状态。

**方案**:
- 时间轮中保留 Paused 任务的位置，但 tick 时跳过执行
- 恢复时直接使用已有调度位置

## 实现优先级

1. **P0**: Agent 任务执行 — 核心功能完整性
2. **P1**: 暂停/恢复 + 编辑 — 基础用户体验
3. **P2**: 时间轮优化 + cron 增强 — 性能和兼容性
4. **P3**: Paused 状态利用 — 代码完善

## 测试策略

每个改进点都需要：
- 单元测试（核心逻辑）
- 集成测试（跨模块交互）
- 工具 handler 测试（端到端）