# Dev-Assistant 长时间任务执行设计文档

## 1. 问题背景

当前 dev-assistant-rs 存在以下限制，无法支持长时间自动执行任务：

| 限制 | 当前状态 | 影响 |
|------|----------|------|
| **执行模式单一** | 交互模式需要人工输入，非交互模式执行单条消息后退出 | 无法自动完成多步骤任务 |
| **迭代次数限制** | `max_iterations` 默认为有限值 | 长时间任务可能中途终止 |
| **上下文压缩简单** | 直接删除旧消息，不保留摘要 | 丢失关键上下文信息 |
| **无状态持久化** | 仅保存重启状态，无检查点机制 | 崩溃后无法恢复进度 |
| **无进度监控** | 无法查询任务状态和进度 | 用户无法了解任务进展 |

## 2. 架构设计

### 2.1 核心概念

```
┌─────────────────────────────────────────────────────────────────┐
│                        Agent (增强版)                           │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐   │
│  │ TaskContext  │  │  TaskState   │  │ CompressionStrategy │   │
│  │ - task_id    │  │ - Running    │  │ - KeepRecent        │   │
│  │ - status     │  │ - Paused     │  │ - KeepByImportance  │   │
│  │ - progress   │  │ - Completed  │  │ - SummarizeOld      │   │
│  │ - start_time │  │ - Failed     │  │ - ArchiveAndSum     │   │
│  │ - iterations │  │ - Interrupted│  └──────────────────────┘   │
│  └──────────────┘  └──────────────┘                              │
│                                                                   │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │                     run_background()                       │  │
│  │  - 后台循环执行                                             │  │
│  │  - 每轮迭代检查退出条件                                      │  │
│  │  - 可恢复错误自动重试                                        │  │
│  │  - 定期保存检查点                                            │  │
│  └────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

### 2.2 数据结构定义

#### TaskState - 任务状态枚举

```rust
pub enum TaskState {
    Running,        // 任务正在执行
    Paused,         // 用户暂停
    Completed,      // 任务完成
    Failed,         // 任务失败
    Interrupted,    // 用户中断
}
```

#### TaskContext - 任务上下文

```rust
pub struct TaskContext {
    pub task_id: String,             // 任务唯一标识
    pub status: TaskState,           // 当前状态
    pub progress: u8,                // 进度百分比 (0-100)
    pub current_step: String,        // 当前步骤描述
    pub start_time: SystemTime,      // 开始时间
    pub iterations: usize,           // 已执行迭代次数
    pub retry_count: usize,          // 重试次数
}
```

#### Checkpoint - 检查点数据

```rust
pub struct Checkpoint {
    pub task_context: TaskContext,
    pub history: ConversationHistory,
    pub timestamp: SystemTime,
    pub version: String,
}
```

#### CompressionStrategy - 压缩策略

```rust
pub enum CompressionStrategy {
    KeepRecent(usize),           // 保留最近 N 轮（当前策略）
    KeepByImportance,            // 按重要性保留
    SummarizeOld,                // 对旧消息生成摘要
    ArchiveAndSummarize,         // 归档 + 摘要
}
```

## 3. 核心功能设计

### 3.1 后台执行模式

**入口方法**：`Agent::run_background()`

```rust
pub async fn run_background(
    &mut self,
    user_message: String,
    output: &mut dyn MessageOutput,
    on_progress: Option<Box<dyn FnMut(&TaskContext) + Send + 'static>>,
) -> Result<AgentResult, AppError>
```

**执行流程**：

```
start_turn() → [循环开始]
  │
  ├─→ 更新 TaskContext
  │
  ├─→ 调用 on_progress 回调（如果有）
  │
  ├─→ 检查退出条件（用户暂停、最大迭代、错误等）
  │     └─→ 如果需要退出 → save_checkpoint() → 退出循环
  │
  ├─→ step() 执行一轮
  │     ├─→ AgentStep::Done → save_checkpoint() → 返回结果
  │     ├─→ AgentStep::Continue → 继续循环
  │     └─→ Err → 判断是否可恢复
  │           ├─→ 可恢复 → 重试计数+1 → 继续循环
  │           └─→ 不可恢复 → save_checkpoint() → 返回错误
  │
  └─→ [循环结束]
```

### 3.2 智能上下文压缩

**改进 `Compressor`**：

```rust
pub fn compress_with_strategy(
    &mut self,
    strategy: CompressionStrategy,
) -> CompressionInfo
```

**压缩策略对比**：

| 策略 | 行为 | 适用场景 |
|------|------|----------|
| `KeepRecent(N)` | 保留最近 N 轮，删除更早的 | 当前策略，简单高效 |
| `KeepByImportance` | 根据消息类型和内容重要性判断 | 需要保留关键上下文 |
| `SummarizeOld` | 对旧消息生成 LLM 摘要 | 需要保留语义信息 |
| `ArchiveAndSummarize` | 归档到持久化存储 + 生成摘要 | 长时间任务，需完整历史 |

### 3.3 检查点机制

**保存检查点**：

```rust
pub fn save_checkpoint(&self, path: &Path) -> Result<(), AppError>
```

**从检查点恢复**：

```rust
pub fn load_checkpoint(path: &Path) -> Result<Self, AppError>
```

**检查点文件格式**：

```json
{
  "task_context": {
    "task_id": "task-abc123",
    "status": "Running",
    "progress": 45,
    "current_step": "正在分析代码",
    "start_time": "2024-01-15T10:00:00Z",
    "iterations": 23,
    "retry_count": 1
  },
  "history": { ... },
  "timestamp": "2024-01-15T10:15:00Z",
  "version": "0.2.0"
}
```

### 3.4 错误恢复机制

**扩展 LLM 客户端重试逻辑**：

```rust
// 当前：仅处理 rate limit
// 扩展后：支持多种可恢复错误

enum RetryableError {
    RateLimited,
    NetworkTimeout,
    ServiceUnavailable,
    ServerError(u16),
}
```

**重试策略**：

| 错误类型 | 最大重试次数 | 退避策略 |
|----------|-------------|----------|
| RateLimited | 5 | 指数退避 |
| NetworkTimeout | 3 | 线性退避 |
| ServiceUnavailable | 3 | 线性退避 |
| ServerError(5xx) | 2 | 固定延迟 |

### 3.5 任务管理工具

#### task_status - 查询任务状态

```json
{
  "name": "task_status",
  "description": "查询当前任务状态和进度",
  "parameters": {}
}
```

**返回示例**：

```
任务状态: Running
进度: 45%
当前步骤: 正在分析代码文件
已执行轮数: 23
开始时间: 2024-01-15 10:00:00
```

#### pause_task - 暂停任务

```json
{
  "name": "pause_task",
  "description": "暂停当前运行的任务，保存检查点",
  "parameters": {}
}
```

#### resume_task - 恢复任务

```json
{
  "name": "resume_task",
  "description": "从检查点恢复已暂停的任务",
  "parameters": {}
}
```

#### cancel_task - 取消任务

```json
{
  "name": "cancel_task",
  "description": "取消当前任务",
  "parameters": {}
}
```

## 4. 模块改动清单

### 4.1 agent/mod.rs

- 新增 `TaskState` 枚举
- 新增 `TaskContext` 结构体
- 新增 `run_background()` 方法
- 新增 `save_checkpoint()` 方法
- 新增 `load_checkpoint()` 方法

### 4.2 agent/compressor.rs

- 新增 `CompressionStrategy` 枚举
- 新增 `compress_with_strategy()` 方法
- 新增 `generate_summary()` 方法（调用 LLM 生成摘要）

### 4.3 llm/client.rs

- 扩展 `RetryableError` 枚举
- 扩展重试逻辑，支持多种错误类型
- 新增 `is_retryable()` 方法

### 4.4 persist/mod.rs

- 新增 `Checkpoint` 结构体
- 新增 `save_checkpoint()` 方法
- 新增 `load_checkpoint()` 方法
- 支持检查点文件的版本兼容

### 4.5 tools/mod.rs

- 注册新工具：`task_status`, `pause_task`, `resume_task`, `cancel_task`

### 4.6 app.rs

- 新增 `run_background()` 方法（后台模式入口）

### 4.7 repl.rs

- 新增 `/background` slash 命令
- 支持 `/status` 命令查询任务状态

## 5. 实现步骤

| 步骤 | 模块 | 内容 | 优先级 |
|------|------|------|--------|
| 1 | agent/mod.rs | 定义 `TaskState`、`TaskContext` | 高 |
| 2 | agent/mod.rs | 实现 `run_background()` 核心循环 | 高 |
| 3 | persist/mod.rs | 实现检查点保存和恢复 | 高 |
| 4 | llm/client.rs | 扩展错误重试机制 | 中 |
| 5 | agent/compressor.rs | 实现智能压缩策略 | 中 |
| 6 | tools/mod.rs | 新增任务管理工具 | 中 |
| 7 | app.rs | 新增后台模式入口 | 中 |
| 8 | repl.rs | 新增 slash 命令 | 低 |

## 6. 测试计划

### 6.1 单元测试

| 测试用例 | 模块 | 目的 |
|----------|------|------|
| `run_background_completes_task` | agent/mod.rs | 验证后台模式能完成任务 |
| `run_background_saves_checkpoint` | agent/mod.rs | 验证检查点保存功能 |
| `load_checkpoint_resumes_state` | agent/mod.rs | 验证从检查点恢复 |
| `retry_on_rate_limit` | llm/client.rs | 验证 rate limit 重试 |
| `retry_on_timeout` | llm/client.rs | 验证超时重试 |
| `compress_with_summarize` | compressor.rs | 验证摘要压缩 |

### 6.2 集成测试

| 测试用例 | 目的 |
|----------|------|
| `long_running_task_stability` | 运行超过 100 轮迭代的任务，验证稳定性 |
| `crash_recovery` | 模拟崩溃，验证从检查点恢复 |
| `pause_resume` | 验证暂停/恢复功能 |

## 7. 向后兼容性

- **配置兼容**：新增配置项，不影响现有配置
- **状态兼容**：检查点文件支持版本字段，支持向后兼容
- **API 兼容**：保持现有 `run()`、`step()` 方法不变，新增 `run_background()`

## 8. 性能考虑

| 关注点 | 策略 |
|--------|------|
| **内存使用** | 智能压缩，定期清理不再需要的上下文 |
| **磁盘 I/O** | 检查点保存使用 debounce，避免频繁写入 |
| **网络开销** | LLM 调用自动重试，指数退避 |
| **CPU 占用** | 非活跃时进入休眠，减少轮询 |

## 9. 具体场景：大型代码库自动分析

### 9.1 场景描述

用户希望分析一个包含数千个文件的大型代码库，LLM 一次只能处理有限数量的文件。需要设计一套机制让 LLM 能够：

1. 自动发现代码库中的所有相关文件
2. 分批读取和分析文件
3. 追踪分析进度
4. 汇总分析结果
5. 无需人工干预，持续运行直到完成

### 9.2 工作流程设计

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        代码库分析任务流程                                     │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  Step 1: 代码库探索                                                         │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │  list_directory → 获取项目结构                                         │   │
│  │  glob("**/*.rs") → 获取所有 Rust 文件                                   │   │
│  │  分类文件：按模块/目录分组                                              │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                              ↓                                              │
│  Step 2: 分批分析                                                           │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │  batch_read_files(files[0..N]) → 读取一批文件                          │   │
│  │  LLM 分析 → 提取关键信息（架构、依赖、设计模式）                         │   │
│  │  record_analysis() → 将分析结果写入持久化存储                           │   │
│  │  compress() → 压缩已分析的文件内容，保留摘要                            │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                              ↓                                              │
│  Step 3: 进度追踪                                                           │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │  更新 TaskContext.progress                                            │   │
│  │  更新 TaskContext.current_step                                        │   │
│  │  保存检查点（每批分析后）                                               │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                              ↓                                              │
│  Step 4: 循环或完成                                                         │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │  还有未分析文件？                                                       │   │
│  │  ├─→ Yes → 回到 Step 2，处理下一批                                      │   │
│  │  └─→ No → 进入 Step 5                                                  │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                              ↓                                              │
│  Step 5: 结果汇总                                                           │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │  读取所有分析记录                                                        │   │
│  │  LLM 生成综合报告                                                       │   │
│  │  输出最终分析结果                                                        │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 9.3 新增工具设计

#### 9.3.1 analyze_codebase - 启动代码库分析任务

```json
{
  "name": "analyze_codebase",
  "description": "启动大型代码库的自动分析任务。该任务会遍历代码库，分批分析文件，最终生成综合分析报告。",
  "parameters": {
    "type": "object",
    "properties": {
      "include_patterns": {
        "type": "array",
        "items": { "type": "string" },
        "description": "要包含的文件模式（glob），如 ['src/**/*.rs', 'tests/**/*.rs']",
        "default": ["**/*.rs"]
      },
      "exclude_patterns": {
        "type": "array",
        "items": { "type": "string" },
        "description": "要排除的文件模式，如 ['target/**', '.git/**']",
        "default": ["target/**", ".git/**", "node_modules/**"]
      },
      "batch_size": {
        "type": "integer",
        "description": "每批分析的文件数量",
        "default": 5
      },
      "analysis_depth": {
        "type": "string",
        "enum": ["quick", "standard", "deep"],
        "description": "分析深度：quick=仅文件结构，standard=结构+核心逻辑，deep=完整分析",
        "default": "standard"
      }
    }
  }
}
```

#### 9.3.2 record_analysis - 记录分析结果

```json
{
  "name": "record_analysis",
  "description": "将代码分析结果记录到持久化存储，便于后续汇总和查询。",
  "parameters": {
    "type": "object",
    "properties": {
      "file_path": {
        "type": "string",
        "description": "被分析的文件路径"
      },
      "analysis_type": {
        "type": "string",
        "enum": ["structure", "logic", "security", "performance", "design"],
        "description": "分析类型"
      },
      "summary": {
        "type": "string",
        "description": "分析摘要"
      },
      "details": {
        "type": "object",
        "description": "详细分析结果（JSON）"
      },
      "issues": {
        "type": "array",
        "items": {
          "type": "object",
          "properties": {
            "severity": { "type": "string", "enum": ["critical", "high", "medium", "low"] },
            "description": { "type": "string" },
            "location": { "type": "string" }
          }
        },
        "description": "发现的问题列表"
      }
    },
    "required": ["file_path", "analysis_type", "summary"]
  }
}
```

#### 9.3.3 get_analysis_summary - 获取分析汇总

```json
{
  "name": "get_analysis_summary",
  "description": "获取当前代码库分析的汇总信息，包括已分析文件数、发现的问题统计等。",
  "parameters": {
    "type": "object",
    "properties": {
      "group_by": {
        "type": "string",
        "enum": ["file", "type", "severity", "directory"],
        "description": "按什么维度分组汇总",
        "default": "type"
      }
    }
  }
}
```

#### 9.3.4 finish_analysis - 完成分析并生成报告

```json
{
  "name": "finish_analysis",
  "description": "完成代码库分析任务，汇总所有分析结果并生成综合报告。",
  "parameters": {
    "type": "object",
    "properties": {
      "report_type": {
        "type": "string",
        "enum": ["summary", "detailed", "security", "performance", "architecture"],
        "description": "报告类型",
        "default": "detailed"
      },
      "output_file": {
        "type": "string",
        "description": "报告输出文件路径（可选）"
      }
    }
  }
}
```

### 9.4 文件分批策略

#### 9.4.1 基于目录的分组

```rust
pub struct FileBatch {
    pub files: Vec<String>,           // 本批文件列表
    pub batch_number: usize,          // 批次号
    pub total_batches: usize,         // 总批次数
    pub directory: String,            // 所属目录（用于上下文关联）
    pub estimated_tokens: usize,      // 预估 token 数
}

pub fn create_file_batches(
    files: Vec<String>,
    batch_size: usize,
    max_tokens_per_batch: usize,
) -> Vec<FileBatch>
```

#### 9.4.2 分组算法

```
1. 按目录深度排序文件（深度优先，保持模块内聚）
2. 遍历文件列表，累计 token 估计值
3. 当达到 batch_size 或 max_tokens_per_batch 时，创建一批
4. 保持同一目录的文件在同一批（除非超出 token 限制）
5. 最后一批可能包含少于 batch_size 的文件
```

### 9.5 分析结果持久化

#### 9.5.1 AnalysisRecord - 分析记录

```rust
pub struct AnalysisRecord {
    pub file_path: String,
    pub analysis_type: AnalysisType,
    pub summary: String,
    pub details: serde_json::Value,
    pub issues: Vec<Issue>,
    pub timestamp: SystemTime,
    pub batch_number: usize,
}

pub struct Issue {
    pub severity: Severity,
    pub description: String,
    pub location: String,
    pub suggestion: Option<String>,
}

pub enum AnalysisType {
    Structure,       // 文件结构分析
    Logic,           // 业务逻辑分析
    Security,        // 安全分析
    Performance,     // 性能分析
    Design,          // 设计模式分析
}

pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}
```

#### 9.5.2 分析结果存储格式

```json
{
  "analysis_records": [
    {
      "file_path": "src/agent/mod.rs",
      "analysis_type": "structure",
      "summary": "Agent 模块包含核心执行逻辑，使用 ContextManager 管理状态",
      "details": {
        "lines_of_code": 420,
        "functions": ["run", "step", "process_tool_calls"],
        "structs": ["Agent", "AgentConfig", "AgentResult"],
        "dependencies": ["llm", "tools", "persist"]
      },
      "issues": [
        {
          "severity": "medium",
          "description": "max_iterations 限制可能导致长时间任务中断",
          "location": "line 310",
          "suggestion": "考虑增加后台执行模式支持"
        }
      ],
      "timestamp": "2024-01-15T10:30:00Z",
      "batch_number": 1
    }
  ],
  "task_summary": {
    "total_files": 150,
    "analyzed_files": 45,
    "issues_found": {
      "critical": 2,
      "high": 5,
      "medium": 15,
      "low": 23
    },
    "start_time": "2024-01-15T10:00:00Z",
    "end_time": null
  }
}
```

### 9.6 提示词设计

#### 9.6.1 分析提示词模板

```
你是一个资深代码分析师。请分析以下文件，按照指定的分析深度进行。

分析深度：{{analysis_depth}}

分析要求：
1. 文件结构：列出主要的结构体、函数、模块依赖
2. 业务逻辑：解释核心功能的实现机制
3. 设计模式：识别使用的设计模式和架构风格
4. 潜在问题：发现可能的 bug、性能问题、安全隐患
5. 改进建议：提供具体的优化建议

请使用 record_analysis 工具记录你的分析结果。

如果还有未分析的文件，请继续调用 batch_read_files 获取下一批文件。

当所有文件分析完成后，请调用 finish_analysis 生成综合报告。

待分析文件：
{{file_list}}
```

#### 9.6.2 汇总报告提示词

```
你是一个资深技术架构师。请基于以下分析记录，生成一份综合的代码库分析报告。

报告类型：{{report_type}}

报告结构要求：
1. 项目概览：代码库规模、技术栈、模块结构
2. 架构分析：整体设计思路、核心模块职责
3. 代码质量：发现的问题统计、严重程度分布
4. 改进建议：按优先级排序的优化建议
5. 总结：项目亮点和待改进项

分析记录：
{{analysis_records}}
```

### 9.7 进度计算算法

```rust
pub fn calculate_progress(
    analyzed_count: usize,
    total_count: usize,
    current_phase: AnalysisPhase,
) -> u8 {
    let phase_weight = match current_phase {
        AnalysisPhase::Exploration => 10,    // 探索阶段占 10%
        AnalysisPhase::Analyzing => 70,      // 分析阶段占 70%
        AnalysisPhase::Summarizing => 20,    // 汇总阶段占 20%
    };
    
    let base_progress = match current_phase {
        AnalysisPhase::Exploration => 0,
        AnalysisPhase::Analyzing => 10,
        AnalysisPhase::Summarizing => 80,
    };
    
    let phase_progress = match current_phase {
        AnalysisPhase::Exploration => 100,   // 探索完成即 100%
        AnalysisPhase::Analyzing => {
            if total_count == 0 { 0 } else { (analyzed_count * 100) / total_count }
        },
        AnalysisPhase::Summarizing => 100,   // 汇总完成即 100%
    };
    
    (base_progress + (phase_weight * phase_progress) / 100) as u8
}
```

### 9.8 防止重复分析

#### 9.8.1 已分析文件追踪

```rust
pub struct AnalyzedFileTracker {
    analyzed: HashSet<String>,        // 已分析的文件路径
    in_progress: HashSet<String>,     // 当前正在分析的文件
    failed: HashMap<String, usize>,   // 分析失败的文件及重试次数
}

impl AnalyzedFileTracker {
    pub fn mark_analyzed(&mut self, file: &str) {
        self.analyzed.insert(file.to_string());
        self.in_progress.remove(file);
    }
    
    pub fn mark_in_progress(&mut self, file: &str) {
        self.in_progress.insert(file.to_string());
    }
    
    pub fn is_analyzed(&self, file: &str) -> bool {
        self.analyzed.contains(file)
    }
    
    pub fn get_pending_files(&self, all_files: &[String]) -> Vec<String> {
        all_files
            .iter()
            .filter(|f| !self.is_analyzed(f) && !self.in_progress.contains(f))
            .cloned()
            .collect()
    }
}
```

### 9.9 错误处理与恢复

#### 9.9.1 分析失败处理

```
1. 单文件分析失败：记录到 failed 集合，继续分析其他文件
2. 批量分析失败：重试当前批次（最多 3 次）
3. 连续失败：保存检查点，暂停任务，通知用户
4. 恢复时：跳过已分析文件，从失败点继续
```

#### 9.9.2 检查点恢复流程

```
1. 加载检查点
2. 恢复 TaskContext（进度、当前步骤）
3. 恢复 AnalyzedFileTracker（已分析文件列表）
4. 从当前批次的下一个文件开始继续分析
5. 如果在汇总阶段失败，重新生成报告
```

### 9.10 测试用例

| 测试用例 | 目的 | 输入 | 预期输出 |
|----------|------|------|----------|
| `analyze_small_codebase` | 验证小代码库完整分析 | 10 个文件 | 分析完成，生成报告 |
| `analyze_large_codebase` | 验证大代码库分批分析 | 100+ 个文件 | 分批处理，正确计算进度 |
| `analysis_progress_tracking` | 验证进度追踪 | 20 个文件 | 进度从 0% 递增到 100% |
| `analysis_error_recovery` | 验证失败恢复 | 模拟网络错误 | 重试后继续，不丢失进度 |
| `analysis_checkpoint_resume` | 验证检查点恢复 | 中途暂停 | 从暂停点继续分析 |
| `analysis_report_generation` | 验证报告生成 | 已完成分析 | 生成完整综合报告 |

### 9.11 模块改动清单（补充）

| 模块 | 新增内容 |
|------|----------|
| `tools/file/search.rs` | 增强 `glob_tool`，支持 exclude_patterns |
| `tools/file/read.rs` | 增强 `batch_read_files`，支持按目录分组 |
| `tools/meta_tools.rs` | 新增 `analyze_codebase`, `record_analysis`, `get_analysis_summary`, `finish_analysis` |
| `persist/mod.rs` | 新增 `AnalysisRecord`, `Issue` 存储 |
| `agent/mod.rs` | 新增 `AnalyzedFileTracker`, `calculate_progress` |
