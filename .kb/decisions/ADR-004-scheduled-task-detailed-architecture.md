---
type: decision
status: accepted
title: 定时任务功能详细架构设计（模块、接口、数据流）
tags: [scheduled-task, architecture, design, implementation, detailed]
author: architect
created: 2026-07-26
---

# ADR-004: 定时任务功能详细架构设计

## 1. 概述

本文档在 ADR-001（高层架构）和 ADR-003（实现方案）的基础上，提供定时任务功能的**详细模块结构、接口定义、数据流设计和关键决策理由**，可直接指导代码实现。

## 2. 模块结构

```
src/scheduler/
├── mod.rs                  # 模块根，重新导出
├── task.rs                 # ScheduledTask 数据结构
├── store.rs                # ScheduledTaskStore 持久化层
├── wheel.rs                # TimingWheel 时间轮
├── scheduler.rs            # Scheduler 调度器主循环
├── executor.rs             # ScheduledTaskExecutor 执行器
├── handler.rs              # ScheduledTaskHandler trait + 内置实现
└── tools.rs                # 工具 handler（CRUD + 日志查询）
```

### 2.1 模块职责

| 模块 | 职责 |
|------|------|
| `task.rs` | 定义 `ScheduledTask` 数据结构和状态枚举 |
| `store.rs` | JSONL 持久化、CRUD、启动恢复 |
| `wheel.rs` | 分层时间轮（秒/分/时），O(1) 插入/删除/触发 |
| `scheduler.rs` | 调度器主循环：加载任务到时间轮、触发、处理结果 |
| `executor.rs` | 按任务类型派发执行（Agent/Command），处理重试 |
| `handler.rs` | `ScheduledTaskHandler` trait + `AgentTaskHandler` + `CommandTaskHandler` |
| `tools.rs` | 注册到 `ToolRegistry` 的 4 个工具 handler |

## 3. 核心数据结构

### 3.1 ScheduledTask

```rust
/// 任务 ID 类型
pub type ScheduledTaskId = String;

/// 调度类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScheduleType {
    /// Cron 表达式，如 "0 */1 * * *"
    Cron(String),
    /// 固定间隔（秒），如 3600 表示每小时
    Interval(u64),
    /// 一次性延迟（秒），如 300 表示 5 分钟后执行
    Once(u64),
}

/// 执行模式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskExecutionMode {
    /// 通过 Agent 子代理执行（传入自然语言指令）
    Agent { instruction: String },
    /// 执行 Shell 命令
    Command { command: String, working_dir: Option<String> },
}

/// 任务状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ScheduledTaskStatus {
    /// 活跃（等待调度）
    Active,
    /// 已暂停
    Paused,
    /// 已取消
    Cancelled,
    /// 已完成（一次性任务执行后）
    Completed,
    /// 已失败（重试耗尽）
    Failed,
}

/// 定时任务定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    /// 唯一标识
    pub id: ScheduledTaskId,
    /// 任务名称（用户可读）
    pub name: String,
    /// 调度类型
    pub schedule: ScheduleType,
    /// 执行模式
    pub mode: TaskExecutionMode,
    /// 当前状态
    pub status: ScheduledTaskStatus,
    /// 创建时间
    pub created_at: i64,  // Unix timestamp (秒)
    /// 下次调度时间
    pub next_run_at: i64, // Unix timestamp (秒)
    /// 上次执行时间
    pub last_run_at: Option<i64>,
    /// 已执行次数
    pub run_count: u64,
    /// 最大重试次数
    pub max_retries: u32,
    /// 当前重试次数
    pub retry_count: u32,
    /// 乐观锁版本号
    pub version: u64,
    /// 标签（用于分类/过滤）
    pub tags: Vec<String>,
    /// 超时秒数（0=不超时）
    pub timeout_secs: u64,
}
```

### 3.2 ExecutionRecord

```rust
/// 单次执行记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    /// 记录 ID
    pub id: String,
    /// 任务 ID
    pub task_id: ScheduledTaskId,
    /// 执行时间
    pub executed_at: i64,
    /// 是否成功
    pub success: bool,
    /// 输出摘要
    pub output: String,
    /// 耗时（毫秒）
    pub duration_ms: u64,
    /// 错误信息（失败时）
    pub error: Option<String>,
}
```

## 4. 接口定义

### 4.1 Tool 接口（Agent 可调用）

#### 4.1.1 `schedule_task` — 创建定时任务

**输入参数**：
```json
{
  "name": "每日代码审查",
  "schedule": {
    "type": "cron",
    "expression": "0 9 * * *"
  },
  "mode": {
    "type": "agent",
    "instruction": "审查 src/ 目录下今天修改的代码"
  },
  "max_retries": 3,
  "tags": ["review", "daily"],
  "timeout_secs": 600
}
```

**输出**：
```json
{
  "success": true,
  "task_id": "sched_abc123",
  "next_run_at": "2026-07-27T09:00:00Z"
}
```

#### 4.1.2 `unschedule_task` — 取消定时任务

**输入参数**：
```json
{
  "task_id": "sched_abc123"
}
```

**输出**：
```json
{
  "success": true,
  "message": "任务已取消"
}
```

#### 4.1.3 `list_scheduled_tasks` — 列出所有定时任务

**输入参数**：
```json
{
  "status": "active",
  "tags": ["review"],
  "limit": 20,
  "offset": 0
}
```

**输出**：
```json
{
  "tasks": [
    {
      "id": "sched_abc123",
      "name": "每日代码审查",
      "schedule": { "type": "cron", "expression": "0 9 * * *" },
      "status": "active",
      "next_run_at": "2026-07-27T09:00:00Z",
      "run_count": 5,
      "last_run_at": "2026-07-26T09:00:00Z"
    }
  ],
  "total": 1
}
```

#### 4.1.4 `get_scheduled_task_logs` — 查询执行记录

**输入参数**：
```json
{
  "task_id": "sched_abc123",
  "limit": 20,
  "offset": 0
}
```

**输出**：
```json
{
  "task_id": "sched_abc123",
  "task_name": "每日代码审查",
  "records": [
    {
      "id": "rec_001",
      "executed_at": "2026-07-26T09:00:00Z",
      "success": true,
      "output": "审查完成，发现 3 个问题",
      "duration_ms": 45000,
      "error": null
    }
  ],
  "total": 5
}
```

### 4.2 Scheduler 内部接口

#### 4.2.1 Scheduler 主循环

```rust
pub struct Scheduler {
    /// 时间轮
    wheel: TimingWheel,
    /// 任务持久化存储
    store: ScheduledTaskStore,
    /// 执行器
    executor: Arc<ScheduledTaskExecutor>,
    /// 运行状态控制
    running: Arc<AtomicBool>,
    /// 暂停标志
    paused: Arc<AtomicBool>,
    /// 心跳 tick 间隔（秒）
    tick_interval: u64,
}

impl Scheduler {
    /// 启动调度器（后台 tokio task）
    pub async fn start(&self);
    
    /// 优雅关闭
    pub async fn shutdown(&self);
    
    /// 暂停调度
    pub fn pause(&self);
    
    /// 恢复调度
    pub fn resume(&self);
}
```

#### 4.2.2 TimingWheel 时间轮

```rust
pub struct TimingWheel {
    /// 槽位（秒级精度，默认 3600 个槽 = 1 小时）
    slots: Vec<Vec<SlotEntry>>,
    /// 当前指针位置
    cursor: AtomicU64,
    /// 槽大小（秒）
    slot_size: u64,
    /// 总槽数
    num_slots: u64,
}

struct SlotEntry {
    task_id: ScheduledTaskId,
    /// 该任务的调度 epoch 秒
    epoch: u64,
}

impl TimingWheel {
    /// 添加任务到时间轮（计算 epoch 秒，放入对应槽）
    pub fn add_task(&self, task: &ScheduledTask) -> Result<()>;
    
    /// 移除任务
    pub fn remove_task(&self, task_id: &str) -> bool;
    
    /// 推进指针，返回当前 tick 到期的任务 ID 列表
    pub fn tick(&self) -> Vec<ScheduledTaskId>;
    
    /// 清空
    pub fn clear(&self);
}
```

#### 4.2.3 ScheduledTaskStore 持久化

```rust
pub struct ScheduledTaskStore {
    /// 存储根目录
    base_dir: PathBuf,
    /// 任务文件路径
    tasks_path: PathBuf,
    /// 执行记录文件路径
    logs_dir: PathBuf,
    /// 内存缓存（任务 ID -> ScheduledTask）
    cache: RwLock<HashMap<ScheduledTaskId, ScheduledTask>>,
}

impl ScheduledTaskStore {
    /// 创建/打开存储
    pub fn new(base_dir: &Path) -> Result<Self>;
    
    /// 保存任务（追加到 JSONL，更新缓存）
    pub fn save_task(&self, task: &ScheduledTask) -> Result<()>;
    
    /// 加载所有任务（启动时调用）
    pub fn load_all(&self) -> Result<Vec<ScheduledTask>>;
    
    /// 更新任务状态（乐观锁 CAS）
    pub fn update_status(&self, id: &str, status: ScheduledTaskStatus, version: u64) -> Result<bool>;
    
    /// 记录执行结果
    pub fn record_execution(&self, record: &ExecutionRecord) -> Result<()>;
    
    /// 查询执行记录
    pub fn get_execution_logs(&self, task_id: &str, limit: usize, offset: usize) -> Result<Vec<ExecutionRecord>>;
    
    /// 删除任务
    pub fn delete_task(&self, id: &str) -> Result<()>;
}
```

#### 4.2.4 ScheduledTaskHandler trait

```rust
#[async_trait]
pub trait ScheduledTaskHandler: Send + Sync {
    /// 执行任务，返回执行结果
    async fn execute(&self, task: &ScheduledTask) -> Result<ExecutionRecord, AppError>;
    
    /// 处理执行失败后的重试逻辑
    async fn on_retry(&self, task: &ScheduledTask, error: &str) -> Result<(), AppError>;
}
```

#### 4.2.5 内置 Handler 实现

```rust
/// Agent 任务处理器：通过 spawn_subagent 执行
pub struct AgentTaskHandler {
    llm: Arc<LlmClient>,
    tools: ToolRegistry,
}

/// Shell 命令任务处理器：通过 exec_command 执行
pub struct CommandTaskHandler {
    working_dir: PathBuf,
}
```

## 5. 数据流设计

### 5.1 任务创建数据流

```
Agent (自然语言) 
    → schedule_task 工具 handler
        → 解析参数，生成 ScheduledTask 对象
        → 计算 next_run_at
        → ScheduledTaskStore::save_task() (写入 JSONL)
        → Scheduler::schedule_task() (添加到时间轮)
    → 返回 task_id 给 Agent
```

### 5.2 调度触发数据流

```
Scheduler 主循环 (每秒 tick)
    → TimingWheel::tick()
        → 获取到期任务 ID 列表
        → 对每个任务：
            1. 从 Store 加载任务（获取最新版本）
            2. 乐观锁 CAS: status=Active → Running
            3. 派发到 Executor
             ┌─────────────────────────────────────┐
             │ Executor::execute()                 │
             │   → 按 mode 选择 Handler            │
             │   → AgentTaskHandler::execute()     │
             │   → 或 CommandTaskHandler::execute() │
             │   → 返回 ExecutionRecord            │
             └─────────────────────────────────────┘
            4. 记录 ExecutionRecord 到 Store
            5. 更新任务状态：
               - 成功 + 周期任务 → 计算 next_run_at → 重新加入时间轮
               - 成功 + 一次性任务 → status=Completed
               - 失败 + 可重试 → retry_count++ → 延迟重试
               - 失败 + 重试耗尽 → status=Failed
```

### 5.3 启动恢复数据流

```
App::run()
    → Scheduler::new()
        → ScheduledTaskStore::load_all() (从 JSONL 加载)
        → 过滤出 Active 状态的任务
        → 计算每个任务的 next_run_at
        → 添加到 TimingWheel
    → tokio::spawn(scheduler.start())
        → 开始 tick 循环
```

### 5.4 取消任务数据流

```
Agent (自然语言)
    → unschedule_task 工具 handler
        → 从 Store 加载任务
        → UpdateStatus: Active → Cancelled (CAS)
        → TimingWheel::remove_task()
    → 返回确认消息
```

## 6. 关键设计决策

### 决策 1: 分层时间轮精度设计

**方案**：使用单层时间轮，3600 个槽（精度 1 秒，覆盖 1 小时）。

**理由**：
- 单层实现简单，无需处理层级间的级联操作
- 3600 个 `Vec<SlotEntry>` 约 28KB 内存开销，完全可以接受
- 超过 1 小时的任务通过 `next_run_at` 计算相对偏移，仍可放入对应槽
- 如果 `next_run_at - now > 3600`，放入"溢出队列"，每分钟检查一次
- 分层时间轮（秒/分/时）的级联逻辑复杂，在单实例场景下收益不大

### 决策 2: 为什么不使用 `tokio::time::interval` 或现有 cron 库

**方案**：自实现时间轮，不依赖第三方调度库。

**理由**：
- `tokio::time::interval` 适合固定间隔，无法处理 cron 表达式
- `tokio-cron-scheduler` 等库每个任务一个 tokio task，任务量大会导致大量 task 开销
- 时间轮用单个 tokio task 管理所有任务，内存效率高
- 与现有代码风格一致（无外部调度依赖）
- 可精确控制暂停/恢复/重试逻辑

### 决策 3: 持久化使用 JSONL 而非 SQLite

**方案**：使用 JSONL 文件存储，与现有 `SessionStore` 风格一致。

**理由**：
- 零额外依赖（已有 serde_json）
- 与现有 `src/persist/` 模块风格一致
- 人类可读，便于调试
- 追加写入性能好，适合记录类数据
- 小规模（<10,000 任务）下性能足够
- 未来可迁移到 SQLite 当规模增长

### 决策 4: 内存缓存 + 文件双写

**方案**：`ScheduledTaskStore` 维护 `HashMap` 内存缓存，所有读写先操作缓存，再异步写文件。

**理由**：
- 调度器每秒 tick 需要快速查找任务，文件 IO 不可接受
- 写操作先更新缓存，再异步写文件（不阻塞调度循环）
- 启动时从文件全量加载到缓存
- 崩溃时最多丢失最后一次写入的记录（通过 WAL 可解决，但当前阶段不必要）

### 决策 5: 执行器与调度器分离

**方案**：`Scheduler` 只负责触发，`Executor` 负责执行。

**理由**：
- 调度器可以保持轻量，不阻塞在长时间执行上
- 执行器通过 `tokio::spawn` 异步执行，不阻塞调度 tick
- 重试逻辑在 Executor 层处理，不影响调度器
- 可独立测试调度和执行逻辑

### 决策 6: 通过全局 State 共享 Scheduler

**方案**：使用 `once_cell::sync::Lazy` + `Mutex<Option<Arc<Scheduler>>>` 模式，与现有 `GLOBAL_TASK_MANAGER` 一致。

**理由**：
- 工具 handler 是同步函数，需要通过全局状态访问 Scheduler
- 与现有 `task_tools.rs` 中的 `GLOBAL_TASK_MANAGER` 模式一致
- `Arc<Scheduler>` 允许在多个 tokio task 间共享
- 调度器内部的 `AtomicBool` 用于控制运行/暂停状态

## 7. 与现有模块的集成

### 7.1 在 `App::build()` 中初始化

```rust
// 在 App::build() 末尾添加：
let scheduler = Scheduler::new(
    config.working_dir.join(".dev-assistant-store/scheduled_tasks"),
    llm_client.clone(),
    tools.new_subagent_registry(),
);
scheduler.restore_from_disk()?;  // 加载已有任务到时间轮
set_global_scheduler(Arc::new(scheduler));  // 注册到全局
```

### 7.2 在 `App::run()` 中启动

```rust
// 在 App::run() 中：
let scheduler = get_global_scheduler().unwrap();
tokio::spawn(async move {
    scheduler.start().await;
});
```

### 7.3 注册工具到 ToolRegistry

```rust
// 在 ToolRegistry::register_builtin_tools() 中新增：
"schedule_task", "unschedule_task", "list_scheduled_tasks", "get_scheduled_task_logs"
```

### 7.4 在 `get_global_task_manager` 同级处添加

```rust
// 在 src/tools/task_tools.rs 或新文件 src/scheduler/tools.rs 中
static GLOBAL_SCHEDULER: Lazy<Mutex<Option<Arc<Scheduler>>>> = Lazy::new(|| Mutex::new(None));
pub fn set_global_scheduler(scheduler: Arc<Scheduler>) { ... }
pub fn get_global_scheduler() -> Option<Arc<Scheduler>> { ... }
```

## 8. 非功能性需求

| 指标 | 目标 |
|------|------|
| 调度精度 | ±1 秒（秒级时间轮） |
| 单实例任务容量 | 10,000+ 活跃任务 |
| 内存占用 | 每个任务约 200 字节（10,000 任务约 2MB） |
| 持久化 | JSONL 文件，路径 `.dev-assistant-store/scheduled_tasks/` |
| 启动恢复 | < 100ms（10,000 任务） |
| 并发安全 | `Arc<AtomicBool>` + `RwLock` |
| 可观测 | 执行记录 + 日志工具 |

## 9. 实现优先级

| 优先级 | 模块 | 依赖 |
|--------|------|------|
| P0 | `task.rs` — 数据结构 | 无 |
| P0 | `store.rs` — JSONL 持久化 | task.rs |
| P0 | `wheel.rs` — 时间轮 | task.rs |
| P0 | `scheduler.rs` — 主循环 | wheel, store |
| P0 | `handler.rs` + `executor.rs` — 执行器 | task, store |
| P0 | `tools.rs` — 工具 handler | scheduler, executor |
| P1 | `App::build/run` 集成 | 以上所有 |
| P2 | 执行超时控制 | executor |
| P2 | 标签过滤/分类查询 | store, tools |