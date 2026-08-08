# 小上下文窗口优化设计文档

## 1. 概述

### 1.1 问题背景

当前 dev-assistant-rs 的上下文管理机制假设 LLM 拥有**足够大的上下文窗口**（如 32K+ tokens），但对于小上下文模型（4K-8K tokens），现有机制存在以下问题：

| 问题 | 当前状态 | 影响 |
|------|----------|------|
| **上下文压缩粗暴** | 达到 90% 阈值后直接丢弃旧消息 | 丢失关键信息，模型"失忆" |
| **无预算感知** | Agent 不知道自己的上下文使用情况 | 无法主动管理上下文 |
| **子代理预算不透明** | 子代理统一 `max_tokens=8192` | 小模型容易超出限制 |
| **检查点恢复不重建上下文** | 只恢复任务状态，不恢复对话上下文 | 恢复后 Agent 无法继续之前的工作 |
| **无分层摘要** | 所有历史被同等对待 | 重要信息与噪音一起被丢弃 |

### 1.2 设计目标

| 目标 | 描述 | 优先级 |
|------|------|--------|
| **上下文预算感知** | Agent 能主动查询上下文使用情况，规划自己的行为 | P0 |
| **智能摘要压缩** | 压缩时保留关键信息的语义摘要，而非简单截断 | P1 |
| **子代理预算传递** | 父代理告知子代理可用上下文预算，子代理据此规划 | P1 |
| **检查点上下文重建** | 从检查点恢复时，重建 Agent 的工作上下文 | P2 |
| **分层摘要系统** | 多层级摘要，不同粒度保留不同信息 | P2 |

### 1.3 设计原则

1. **显式而非隐式**：上下文预算信息通过工具调用显式获取，而非隐式塞入 prompt
2. **增量构建**：每一步独立可用，不依赖后续阶段
3. **最小侵入**：尽量复用现有组件（Compressor、Orchestrator、KB），避免大规模重构
4. **LLM 原生**：格式和接口设计符合 LLM 的读写习惯（Markdown > JSON）

---

## 2. 整体架构

### 2.1 核心组件关系

```
┌──────────────────────────────────────────────────────────────────────┐
│                         ContextManager (增强)                        │
│                                                                      │
│  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐   │
│  │ ConversationHist │  │ ContextBudget    │  │ ContextCompressor│   │
│  │ ory              │  │ - 预算跟踪       │  │ (增强版)          │   │
│  │ - messages       │  │ - 使用率报告     │  │ - Truncate       │   │
│  │ - used_tokens    │  │ - 预警阈值       │  │ - Summarize      │   │
│  └──────────────────┘  └──────────────────┘  │ - Hybrid         │   │
│                                                └──────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐    │
│  │                    ContextBudgetReport                        │    │
│  │  - system_tokens: 1200   (系统提示)                           │    │
│  │  - memory_tokens: 800    (KB 注入的记忆)                      │    │
│  │  - history_tokens: 3500  (对话历史)                            │    │
│  │  - total: 5500 / 8192    (使用/总预算)                        │    │
│  │  - utilization: 67%      (使用率)                             │    │
│  │  - estimated_room: 2692  (剩余可用)                           │    │
│  └──────────────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────────────┘
         │ 被 Agent 通过工具查询
         ▼
┌──────────────────────────────────────────────────────────────────────┐
│                         Agent (增强)                                 │
│                                                                      │
│  - 每轮迭代前检查上下文预算                                          │
│  - 接近阈值时主动做摘要压缩                                          │
│  - 创建子代理时传递预算信息                                          │
│  - 调用 kb_store 保存关键信息到外部记忆                              │
└──────────────────────────────────────────────────────────────────────┘
         │
         ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      KnowledgeBase (外部记忆)                        │
│                                                                      │
│  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐   │
│  │ 对话摘要缓存     │  │ 关键决策记录     │  │ 进度追踪         │   │
│  │ session-summary/ │  │ decisions/       │  │ progress/        │   │
│  └──────────────────┘  └──────────────────┘  └──────────────────┘   │
└──────────────────────────────────────────────────────────────────────┘
```

### 2.2 与现有架构的关系

```
现有架构：
  Agent → ContextManager → ConversationHistory → Compressor(truncate only)

新架构：
  Agent → ContextManager → ConversationHistory
              │                    │
              ▼                    ▼
        ContextBudget       Compressor(enhanced)
              │                    │
              ▼                    ▼
        预算报告给Agent      Summarize | Truncate | Hybrid
              │                    │
              └────────┬───────────┘
                       ▼
              KnowledgeBase (摘要存储)
```

关键变化：
- **Compressor 从单一截断模式变为多模式**：新增 Summarize 和 Hybrid 模式
- **新增 ContextBudget 模块**：跟踪和管理上下文预算
- **新增 ContextBudgetReport 工具**：让 Agent 能主动查询上下文状态
- **KnowledgeBase 新增摘要缓存**：存储对话摘要供恢复时使用

---

## 3. 详细设计

### 3.1 上下文预算感知 (P0)

#### 3.1.1 ContextBudget 数据结构

```rust
/// 上下文预算报告。
///
/// 告诉 Agent 当前上下文的使用情况，供其主动管理。
#[derive(Debug, Clone, Serialize)]
pub struct ContextBudget {
    /// 系统提示词占用的 tokens
    pub system_prompt_tokens: usize,
    /// 从 KB 注入的记忆占用的 tokens
    pub memory_tokens: usize,
    /// 对话历史占用的 tokens
    pub history_tokens: usize,
    /// 总使用量
    pub total_tokens: usize,
    /// 最大允许 tokens
    pub max_tokens: usize,
    /// 使用率百分比 (0.0 ~ 1.0)
    pub utilization: f64,
    /// 估算剩余可用 tokens
    pub estimated_room: usize,
    /// 上下文压力等级
    pub pressure: ContextPressure,
}

/// 上下文压力等级。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum ContextPressure {
    /// 充足 (> 40% 剩余)
    Normal,
    /// 注意 (20% ~ 40% 剩余)
    Warning,
    /// 紧张 (10% ~ 20% 剩余)
    Critical,
    /// 即将溢出 (< 10% 剩余)
    Exhausted,
}
```

#### 3.1.2 ContextBudgetManager

```rust
/// 上下文预算管理器。
///
/// 跟踪和管理上下文预算，提供报告生成和预警功能。
pub struct ContextBudgetManager {
    max_tokens: usize,
    system_prompt_tokens: usize,
    memory_tokens: usize,
    /// 预警阈值：当达到此使用率时触发预警
    warning_threshold: f64,   // 默认 0.60
    /// 关键阈值：当达到此使用率时触发关键预警
    critical_threshold: f64,  // 默认 0.80
}

impl ContextBudgetManager {
    /// 生成当前上下文预算报告
    pub fn report(&self, history: &ConversationHistory) -> ContextBudget {
        let total = history.used_tokens;
        // system_prompt_tokens 和 memory_tokens 在构建时记录
        // history_tokens = total - system_prompt_tokens - memory_tokens
        let history_tokens = total.saturating_sub(
            self.system_prompt_tokens + self.memory_tokens
        );
        let utilization = total as f64 / self.max_tokens as f64;
        let estimated_room = self.max_tokens.saturating_sub(total);

        let pressure = if utilization > self.critical_threshold {
            ContextPressure::Critical
        } else if utilization > self.warning_threshold {
            ContextPressure::Warning
        } else if utilization > 0.90 {
            ContextPressure::Exhausted
        } else {
            ContextPressure::Normal
        };

        ContextBudget {
            system_prompt_tokens: self.system_prompt_tokens,
            memory_tokens: self.memory_tokens,
            history_tokens,
            total_tokens: total,
            max_tokens: self.max_tokens,
            utilization,
            estimated_room,
            pressure,
        }
    }

    /// 检查是否需要压缩（基于压力等级）
    pub fn should_compress(&self, history: &ConversationHistory) -> bool {
        let report = self.report(history);
        matches!(report.pressure, 
            ContextPressure::Critical | ContextPressure::Exhausted
        )
    }
}
```

#### 3.1.3 新增工具：`context_budget`

```json
{
  "name": "context_budget",
  "description": "查询当前上下文预算使用情况。返回系统提示、记忆、历史各占用的 token 数，以及使用率、压力等级和剩余可用空间。",
  "parameters": {}
}
```

**返回示例**：
```json
{
  "system_prompt_tokens": 1200,
  "memory_tokens": 800,
  "history_tokens": 3500,
  "total_tokens": 5500,
  "max_tokens": 8192,
  "utilization": 0.67,
  "estimated_room": 2692,
  "pressure": "Warning"
}
```

#### 3.1.4 系统提示词增强

在系统提示词末尾添加上下文预算指引：

```
## 上下文预算管理

你的上下文窗口为 {max_tokens} tokens。请遵循以下原则：
- 使用 `context_budget` 工具查看当前上下文使用情况
- 当压力等级为 Warning 时，考虑压缩不必要的输出
- 当压力等级为 Critical 时，优先使用 kb_store 保存关键信息
- 当压力等级为 Exhausted 时，使用 finish 工具结束当前任务
- 优先使用 kb_store/kb_query 在外部存储信息，而非依赖上下文
```

---

### 3.2 智能摘要压缩 (P1)

#### 3.2.1 压缩策略枚举

```rust
/// 上下文压缩策略。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionStrategy {
    /// 截断模式：保留最近 N 轮对话（当前行为）
    Truncate { keep_rounds: usize },
    
    /// 摘要模式：将旧消息压缩为 LLM 生成的摘要
    Summarize {
        /// 保留的完整对话轮数（最近的 N 轮保留完整）
        keep_rounds: usize,
        /// 摘要最大 token 数
        max_summary_tokens: usize,
        /// 摘要存储路径（KB 中的路径，用于恢复时重建）
        summary_path: Option<String>,
    },
    
    /// 混合模式：先摘要，再截断摘要
    Hybrid {
        /// 保留的完整对话轮数
        keep_rounds: usize,
        /// 摘要最大 token 数
        max_summary_tokens: usize,
        /// 摘要后的保留轮数（摘要本身也占用空间，超出时丢弃最旧摘要）
        max_summary_rounds: usize,
    },
}
```

#### 3.2.2 摘要压缩流程

```
┌──────────────────────────────────────────────────┐
│              压缩触发条件                         │
│  - 使用率 > 90% (当前 compressor 的行为)           │
│  - 或 Agent 主动调用 compress 工具                 │
│  - 或 每 N 轮自动触发                              │
└──────────────────────┬───────────────────────────┘
                       ▼
┌──────────────────────────────────────────────────┐
│              选择压缩策略                         │
│                                                    │
│  ContextBudget.pressure 决定策略：                  │
│  - Normal → 不压缩                                 │
│  - Warning → Summarize (保留最近 3 轮，摘要旧消息)  │
│  - Critical → Hybrid (保留最近 2 轮，摘要+截断)    │
│  - Exhausted → Truncate (保留最近 2 轮，紧急截断)  │
└──────────────────────┬───────────────────────────┘
                       ▼
┌──────────────────────────────────────────────────┐
│          Summarize 模式执行流程                    │
│                                                    │
│  1. 分离消息：                                     │
│     - 旧消息（需要摘要的部分）                      │
│     - 新消息（保留完整）                            │
│                                                    │
│  2. 构建摘要 prompt：                               │
│     "请对以下对话生成一个简洁的摘要，               │
│      保留关键决策、发现的问题、已完成的步骤。       │
│      限制在 {max_summary_tokens} tokens 内。"       │
│                                                    │
│  3. 调用 LLM 生成摘要                               │
│                                                    │
│  4. 将摘要替换旧消息                                │
│     - 移除旧消息                                    │
│     - 插入一条 system 消息作为摘要                  │
│     - 保留最近 N 轮完整对话                         │
│                                                    │
│  5. 将摘要存储到 KB（可选）                         │
│     - 路径: .kb/summaries/{session_id}/round-{n}.md │
│     - 用于恢复时重建上下文                          │
└──────────────────────┬───────────────────────────┘
                       ▼
┌──────────────────────────────────────────────────┐
│          压缩后上下文结构                          │
│                                                    │
│  [system] 系统提示词                               │
│  [system] 【对话摘要】已完成的步骤: ...            │  ← 新增
│           关键决策: ...                            │
│           待处理: ...                              │
│  [user]   最近第 N-2 轮的用户消息                  │
│  [assistant] 最近第 N-2 轮的助手回复               │
│  [user]   最近第 N-1 轮的用户消息                  │
│  [assistant] 最近第 N-1 轮的助手回复               │
│  [user]   当前轮的用户消息                          │
└──────────────────────────────────────────────────┘
```

#### 3.2.3 摘要压缩的核心代码设计

```rust
impl ContextCompressor {
    /// 智能压缩：根据预算压力选择策略
    pub fn smart_compress(
        history: &mut ConversationHistory,
        budget: &ContextBudget,
        llm: Option<&LlmClient>,  // 摘要模式需要 LLM
        config: &CompressionConfig,
    ) -> Result<CompressionInfo, AppError> {
        let strategy = match budget.pressure {
            ContextPressure::Normal => return Ok(CompressionInfo::no_op(history)),
            ContextPressure::Warning => {
                CompressionStrategy::Summarize {
                    keep_rounds: 3,
                    max_summary_tokens: 500,
                    summary_path: None,
                }
            }
            ContextPressure::Critical => {
                CompressionStrategy::Hybrid {
                    keep_rounds: 2,
                    max_summary_tokens: 300,
                    max_summary_rounds: 3,
                }
            }
            ContextPressure::Exhausted => {
                CompressionStrategy::Truncate {
                    keep_rounds: 2,
                }
            }
        };

        Self::compress_with_strategy(history, strategy, llm, config)
    }

    /// 按指定策略压缩
    pub fn compress_with_strategy(
        history: &mut ConversationHistory,
        strategy: CompressionStrategy,
        llm: Option<&LlmClient>,
        config: &CompressionConfig,
    ) -> Result<CompressionInfo, AppError> {
        match strategy {
            CompressionStrategy::Truncate { keep_rounds } => {
                Self::truncate(history, keep_rounds)
            }
            CompressionStrategy::Summarize { keep_rounds, max_summary_tokens, summary_path } => {
                Self::summarize(history, keep_rounds, max_summary_tokens, llm, summary_path, config)
            }
            CompressionStrategy::Hybrid { keep_rounds, max_summary_tokens, max_summary_rounds } => {
                Self::hybrid(history, keep_rounds, max_summary_tokens, max_summary_rounds, llm, config)
            }
        }
    }

    /// 摘要模式：将旧消息压缩为 LLM 生成的摘要
    fn summarize(
        history: &mut ConversationHistory,
        keep_rounds: usize,
        max_summary_tokens: usize,
        llm: Option<&LlmClient>,
        summary_path: Option<String>,
        config: &CompressionConfig,
    ) -> Result<CompressionInfo, AppError> {
        // 1. 分离旧消息和新消息
        // 2. 构建摘要 prompt
        // 3. 调用 LLM 生成摘要
        // 4. 用摘要替换旧消息
        // 5. 可选：存储摘要到 KB
        todo!()
    }
}
```

#### 3.2.4 摘要 prompt 模板

```
请对以下 AI 编程助手的对话生成一个简洁的摘要。

## 摘要要求
- 保留以下关键信息：
  * 已完成的步骤和任务
  * 关键的代码分析/设计决策
  * 发现的 Bug 或问题
  * 待处理的事项
  * 引用的文件路径和关键代码位置
- 忽略：问候语、格式调整、测试运行日志等次要信息
- 限制在 {max_tokens} tokens 以内
- 使用 Markdown 格式

## 需要摘要的对话

{old_messages}

## 摘要
```

#### 3.2.5 摘要存储格式（KB 中）

文件路径：`.kb/summaries/{session_id}/round-{round_number}.md`

```markdown
---
type: context-summary
session: {session_id}
round: {round_number}
compressed_at: {timestamp}
original_messages: 12
original_tokens: 4500
summary_tokens: 380
---

# 对话摘要 (Round {round_number})

## 已完成的步骤
1. 分析了 `src/main.rs` 的入口逻辑
2. 审查了 `src/agent/compressor.rs` 的压缩策略
3. 发现了 compressor 的截断模式丢失关键信息的问题

## 关键决策
- 决定将 Compressor 从单一截断模式改为多模式（Truncate/Summarize/Hybrid）
- 决定新增 ContextBudget 模块跟踪上下文预算

## 待处理
- 实现 Summarize 模式的 LLM 摘要生成
- 编写 ContextBudget 的单元测试

## 引用的文件
- `src/agent/compressor.rs`
- `src/agent/context.rs`
```

---

### 3.3 子代理预算传递 (P1)

#### 3.3.1 传递机制

父代理创建子代理时，将上下文预算信息传递给子代理：

```
父代理上下文（已用 6500/8192 tokens）
  │
  ├─ 创建子代理时传递预算信息
  │    budget: {
  │      parent_utilization: 0.79,      // 父代理使用率
  │      recommended_max_tokens: 4096,   // 建议子代理的 max_tokens
  │      parent_pressure: "Critical",    // 父代理压力等级
  │      advice: "子代理请尽快完成，父代理上下文紧张"
  │    }
  │
  ▼
子代理上下文（max_tokens=4096）
  ├─ 系统提示中包含预算指引
  ├─ 子代理可以使用 context_budget 工具
  └─ 子代理知道父代理的紧张状态，会更快完成
```

#### 3.3.2 SubagentConfig 扩展

```rust
/// 子代理创建参数（扩展版）。
pub struct SubagentConfig {
    // ... 现有字段
    pub llm: Arc<LlmClient>,
    pub tools: ToolRegistry,
    pub depth: usize,
    pub task: String,
    pub context: String,
    pub max_iterations: usize,
    pub max_tokens: usize,
    pub agent_type: Option<AgentIdentity>,
    
    // 新增字段
    /// 父代理的上下文预算信息（供子代理参考）
    pub parent_budget: Option<ContextBudget>,
}
```

#### 3.3.3 子代理系统提示词增强

当子代理收到父代理的预算信息时，系统提示词中增加：

```
## 上下文预算约束

父代理的上下文状态：{parent_pressure}（使用率 {parent_utilization}%）
你的上下文窗口：{max_tokens} tokens
建议：{advice}

请尽快完成你的任务，使用 `context_budget` 工具监控你的上下文使用情况。
关键信息请使用 `kb_store` 保存到知识库，不要依赖上下文。
```

---

### 3.4 检查点上下文重建 (P2)

#### 3.4.1 重建流程

```
Agent 崩溃/重启
  │
  ├─ 1. 加载检查点
  │     ├─ 任务状态恢复 ✅（已有实现）
  │     └─ 检查点中是否包含对话摘要？
  │           ├─ 有 → 直接使用
  │           └─ 无 → 从 KB 重建
  │
  ├─ 2. 从 KB 重建上下文
  │     ├─ 查询 .kb/summaries/{session_id}/ 下的所有摘要
  │     ├─ 按 round 排序
  │     ├─ 从最近的摘要开始，依次加载
  │     └─ 直到达到上下文预算限制
  │
  ├─ 3. 构建恢复上下文
  │     ├─ system prompt (固定)
  │     ├─ 【恢复通知】"你之前的工作已被中断，以下是你之前的工作摘要..."
  │     ├─ 摘要消息（从 KB 加载）
  │     ├─ 【继续指令】"请从以下任务继续..."
  │     └─ 当前任务描述
  │
  └─ 4. 继续执行
```

#### 3.4.2 恢复上下文结构

```rust
/// 从检查点重建 Agent 上下文。
pub fn rebuild_context_from_checkpoint(
    checkpoint: &Checkpoint,
    kb_root: &Path,
    system_prompt: &str,
    max_tokens: usize,
    task_description: &str,
) -> Result<ContextManager, AppError> {
    // 1. 加载 KB 中的对话摘要
    let summaries = load_summaries_from_kb(kb_root, &checkpoint.session_id)?;
    
    // 2. 构建恢复消息
    let mut messages = Vec::new();
    
    // 恢复通知
    messages.push(LlmMessage {
        role: "system".to_string(),
        content: Some(format!(
            "【恢复通知】你之前的工作在 {} 被中断。\n\
             已完成 {}/{} 个任务。\n\
             以下是之前的工作摘要，请仔细阅读后继续。",
            checkpoint.timestamp,
            checkpoint.completed_tasks.len(),
            checkpoint.task_graph.tasks.len(),
        )),
        tool_calls: None,
        tool_call_id: None,
    });
    
    // 加载摘要（从最近的开始，不超过预算）
    let mut budget_remaining = max_tokens 
        - TokenCounter::estimate(system_prompt) 
        - TokenCounter::estimate(task_description);
    
    for summary in summaries.iter().rev() {
        let summary_tokens = TokenCounter::estimate(&summary.content);
        if summary_tokens > budget_remaining {
            break;
        }
        messages.push(summary.to_llm_message());
        budget_remaining -= summary_tokens;
    }
    
    // 3. 构建 ContextManager
    let mut context = ContextManager::new(system_prompt.to_string(), max_tokens);
    context.history.messages = messages;
    context.history.recount_tokens();
    
    Ok(context)
}
```

#### 3.4.3 检查点数据扩展

```rust
/// 检查点数据（扩展版）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    // ... 现有字段
    pub version: String,
    pub timestamp: SystemTime,
    pub task_graph: TaskSnapshot,
    pub completed_tasks: Vec<TaskId>,
    pub in_progress: Vec<RunningTask>,
    pub progress_summary: String,
    pub metadata: Option<serde_json::Value>,
    
    // 新增字段
    /// 会话 ID（用于查找 KB 摘要）
    pub session_id: Option<String>,
    /// 最后一次压缩的摘要索引
    pub last_summary_round: Option<usize>,
    /// 关键上下文的 KB 查询路径列表（用于恢复时加载）
    pub key_context_paths: Vec<String>,
}
```

---

### 3.5 分层摘要系统 (P2)

#### 3.5.1 摘要层级

```
层级 0: 原始对话（完整保留在上下文中）
  → 最近 N 轮完整对话
  
层级 1: 轮次摘要（每轮对话的摘要）
  → 存储: .kb/summaries/{session_id}/round-{n}.md
  → 大小: ~300 tokens/条
  
层级 2: 阶段摘要（每 5 轮摘要的聚合）
  → 存储: .kb/summaries/{session_id}/phase-{n}.md
  → 大小: ~500 tokens/条
  
层级 3: 会话摘要（整个会话的最终摘要）
  → 存储: .kb/summaries/{session_id}/final.md
  → 大小: ~1000 tokens/条
```

#### 3.5.2 摘要聚合流程

```
Round 1-5 的摘要
  │
  ├─ 合并为阶段摘要
  │    prompt: "请综合以下 5 轮对话摘要，生成一份阶段摘要..."
  │    输出: phase-1.md (~500 tokens)
  │
Round 6-10 的摘要
  │
  ├─ 合并为阶段摘要
  │    输出: phase-2.md (~500 tokens)
  │
Phase 1-4 的阶段摘要
  │
  ├─ 合并为会话摘要
  │    prompt: "请综合以下 4 个阶段的摘要，生成一份会话摘要..."
  │    输出: final.md (~1000 tokens)
  │
  ▼
恢复时：从 final.md 开始，根据需要回溯到 phase 或 round 级别
```

---

## 4. 新增工具

### 4.1 context_budget

```json
{
  "name": "context_budget",
  "description": "查询当前上下文预算使用情况。返回系统提示、记忆、历史各占用的 token 数，以及使用率、压力等级和剩余可用空间。",
  "parameters": {}
}
```

### 4.2 compress_context

```json
{
  "name": "compress_context",
  "description": "主动压缩上下文。使用智能策略（根据当前压力等级选择最佳压缩方式）。调用后旧的对话会被摘要替换。",
  "parameters": {
    "strategy": {
      "type": "string",
      "enum": ["auto", "summarize", "truncate"],
      "description": "压缩策略：auto=自动选择（推荐），summarize=摘要压缩，truncate=截断压缩"
    }
  }
}
```

### 4.3 save_summary

```json
{
  "name": "save_summary",
  "description": "将当前对话的关键信息保存为摘要到知识库。用于在上下文紧张时，先保存关键信息再压缩。",
  "parameters": {
    "content": {
      "type": "string",
      "description": "需要保存的关键信息摘要（Markdown 格式）"
    },
    "tags": {
      "type": "array",
      "items": {"type": "string"},
      "description": "标签列表，便于后续检索"
    }
  }
}
```

---

## 5. 系统提示词增强

### 5.1 预算管理指引

在系统提示词末尾增加以下内容：

```
## 上下文预算管理

你的上下文窗口为 {max_tokens} tokens。当前使用情况可通过 `context_budget` 工具查看。

### 预算使用指引

| 压力等级 | 使用率 | 你应该怎么做 |
|---------|--------|------------|
| ✅ Normal | < 60% | 正常执行任务 |
| ⚠️ Warning | 60-80% | 开始考虑压缩输出，使用 `kb_store` 保存关键信息 |
| 🔴 Critical | 80-90% | 主动调用 `compress_context` 压缩，优先完成核心任务 |
| 🚨 Exhausted | > 90% | 立即使用 `save_summary` 保存关键信息，然后调用 `finish` 结束 |

### 最佳实践

1. **优先使用外部记忆**：关键信息使用 `kb_store` 保存，而非依赖上下文
2. **定期检查预算**：每 3-5 轮工具调用后检查一次 `context_budget`
3. **主动压缩**：在压力到达 Critical 前主动调用 `compress_context`
4. **子代理预算**：创建子代理时，考虑父代理的上下文压力，让子代理尽快完成
```

### 5.2 子代理预算指引（补充）

当子代理收到父代理的预算信息时，额外增加：

```
### 父代理上下文状态

父代理的上下文压力：{parent_pressure}
父代理使用率：{parent_utilization}%

请尽快完成你的任务。父代理的上下文空间有限，
你的结果需要能容纳回父代理的上下文中。
```

---

## 6. 数据流

### 6.1 正常执行流程

```
Agent 开始执行任务
  │
  ├─ 每轮迭代前：
  │     ├─ 可调用 context_budget 查看预算
  │     └─ 根据预算调整行为
  │
  ├─ 发现关键信息：
  │     └─ 调用 kb_store 保存到外部记忆
  │
  ├─ 预算压力达到 Warning：
  │     └─ 开始压缩输出，减少不必要的日志
  │
  ├─ 预算压力达到 Critical：
  │     ├─ 调用 save_summary 保存关键信息
  │     ├─ 调用 compress_context 压缩
  │     └─ 继续执行核心任务
  │
  ├─ 预算压力达到 Exhausted：
  │     ├─ 调用 save_summary 保存最终状态
  │     └─ 调用 finish 结束
  │
  └─ 任务完成：
        ├─ 调用 kb_store 记录最终结果
        └─ 调用 finish 结束
```

### 6.2 崩溃恢复流程

```
系统重启
  │
  ├─ Orchestrator 检测到检查点
  │
  ├─ 加载检查点
  │     ├─ 恢复任务状态 ✅
  │     └─ 读取 session_id 和 last_summary_round
  │
  ├─ 从 KB 重建上下文
  │     ├─ 加载 .kb/summaries/{session_id}/ 下的摘要
  │     ├─ 从 final.md 开始（如果有）
  │     └─ 否则从最近的 phase 摘要开始
  │
  ├─ 构建恢复 ContextManager
  │     ├─ 系统提示词
  │     ├─ 恢复通知消息
  │     ├─ 摘要消息
  │     └─ 当前任务消息
  │
  └─ 继续执行任务
```

---

## 7. 与现有模块的集成

### 7.1 修改的文件

| 文件 | 修改内容 | 影响范围 |
|------|---------|---------|
| `src/agent/compressor.rs` | 新增 Summarize 压缩模式，CompressionStrategy 枚举，truncate/summarize/no_op 方法 | 核心修改 |
| `src/agent/context.rs` | 新增 ContextPressure/ContextBudget/ContextBudgetManager，集成到 ContextManager | 核心修改 |
| `src/agent/mod.rs` | 新增 handle_context_tool() 拦截处理 context_budget/compress_context/save_summary | Agent 能力增强 |
| `src/agent/history.rs` | 新增 split_old_messages() 和 replace_old_with_summary() | 辅助方法 |
| `src/tools/context_budget.rs` | **新建** — 定义 3 个工具的 ToolDefinition（拦截模式） | 新文件 |
| `src/tools/mod.rs` | 注册 context_budget/compress_context/save_summary；子代理注册表包含 | 工具注册 |
| `src/orchestrator/checkpoint.rs` | 新增 context_summary 字段（崩溃恢复时重建上下文用） | 检查点扩展 |
| `src/orchestrator/mod.rs` | SubagentConfig 调用处增加 parent_budget: None | 保持兼容 |
| `src/prompt.rs` | 系统提示词增加「上下文预算管理」章节 | 提示词增强 |

### 7.2 无需修改的文件

| 文件 | 原因 |
|------|------|
| `src/llm/` | LLM 客户端无需修改，摘要压缩直接调用现有 `chat()` 方法 |
| `src/security/` | 安全策略不变，新工具继承现有安全机制 |
| `src/persist/` | 会话持久化逻辑不变 |
| `src/tools/kb.rs` | KnowledgeBase 接口不变，摘要作为普通条目存储 |
| `src/scheduler/` | 调度器逻辑不变 |

---

## 7.3 实现进度快照（2026-08-08）

| 功能 | 状态 | 说明 |
|------|------|------|
| ContextBudget / ContextPressure / ContextBudgetManager | ✅ 已实现 | `src/agent/context.rs` |
| context_budget / compress_context / save_summary 工具 | ✅ 已实现 | `src/tools/context_budget.rs`（新建）+ `src/agent/mod.rs` 拦截 |
| 多策略压缩器（Truncate/Summarize） | ✅ 已实现 | `src/agent/compressor.rs`，Summarize 失败自动回退 Truncate |
| 摘要辅助方法 | ✅ 已实现 | `src/agent/history.rs`：`split_old_messages`/`replace_old_with_summary` |
| 系统提示词预算管理章节 | ✅ 已实现 | `src/prompt.rs` |
| 子代理预算传递 | ✅ 已实现 | `SubagentConfig.parent_budget`，子代理任务描述注入父代理压力 |
| 检查点上下文重建 | ✅ 已实现 | `rebuild_context_from_checkpoint()` 消费 `SummaryStore::load_all` 重建 Agent 上下文，3 个集成测试 |
| 检查点扩展字段 | ✅ 已实现 | `Checkpoint.context_summary` + `session_id`，`ContextManager.session_id` |
| 分层摘要系统 | ✅ 已实现 | `src/agent/summary.rs`：三层摘要（round/phase/final），`save_summary` 集成分层存储+聚合，9 个单元测试 |
| 自动预算检查（每 N 轮） | ⏳ 待实现 | 当前依赖 Agent 主动调用 `context_budget` |
| Hybrid 压缩模式 | ⏳ 待实现 | 当前 Summarize 已保留最近轮次，近似 Hybrid |

**验证状态**：`cargo check` 通过（0 errors）；349 单元测试通过，6 个失败均为 `tools/file/symbol` 预存失败（已用 `git stash` 确认与本次改动无关）。

---

## 8. 实施路线图

### 阶段 1：上下文预算感知 (P0) — ✅ 已完成

```
✅ 实现 ContextBudget 数据结构
✅ 实现 ContextBudgetManager
✅ 实现 context_budget 工具（src/tools/context_budget.rs）
✅ 集成到 ContextManager（get_budget_report / budget_report_json）
✅ 增强系统提示词（上下文预算管理章节）
✅ 实现 compress_context / save_summary 工具
✅ 注册到 ToolRegistry（register_builtin_tools + create_tool_by_name + 子代理注册表）
```

### 阶段 2：智能摘要压缩 (P1) — ✅ 已完成

```
✅ 实现 Summarize 压缩模式（ContextCompressor::summarize）
✅ 实现摘要 prompt 模板（中文，保留步骤/决策/问题/待办）
✅ 实现 LLM 调用生成摘要（llm.call 纯文本；失败自动回退截断）
✅ 实现 Truncate 截断模式（保留最近 6 轮）
✅ 实现 split_old_messages / replace_old_with_summary
✅ compress_context 支持 auto/summarize/truncate 策略选择
⏳ Hybrid 压缩模式（当前 Summarize 已保留最近轮次，近似实现）
```

### 阶段 3：子代理预算传递 + 检查点扩展 (P1-P2) — ✅ 已完成

```
✅ SubagentConfig 扩展（新增 parent_budget 字段）
✅ 子代理预算信息传递（父代理压力/使用率注入子代理任务描述）
✅ rebuild_context_from_checkpoint() 实现（src/orchestrator/checkpoint.rs，消费 SummaryStore::load_all 重建 Agent 上下文）
✅ 摘要加载和预算计算（从 final.md → phase-* → round-* 分层加载，按 token 预算限制加载量）
✅ 端到端恢复测试（3 个集成测试：无 session_id 返回 None、有 final 摘要重建、有分层摘要完整重建）
✅ Checkpoint 新增 context_summary 和 session_id 字段（持久化，用于定位摘要目录）
✅ ContextManager 新增 session_id 字段（持久化，定位 .kb/summaries/{session_id}/）
```

### 阶段 4：分层摘要系统 (P2) — 已基本完成

```
✅ 分层摘要存储结构（src/agent/summary.rs：SummaryStore / RoundSummary / PhaseSummary / LayeredSummaries）
✅ 轮次摘要生成（save_summary 工具保存 round-{n}.md，层级 1）
✅ 阶段摘要聚合（每 5 轮自动聚合为 phase-{n}.md，层级 2）
✅ 会话摘要生成（≥2 个阶段时聚合为 final.md，层级 3）
✅ 摘要回溯机制（load_rounds / load_phases / load_final / load_all 按需加载）
✅ 集成到 save_summary 工具（自动分层存储 + 聚合）
✅ 单元测试（9 个：phase 分组、frontmatter 剥离、三层读写、聚合空列表等）
✅ ContextManager 新增 session_id 字段（持久化，定位摘要目录）
✅ 摘要回溯机制（load_all 加载全部层级，按需从 final → phase → round 回溯）
✅ 恢复时分层加载（rebuild_context_from_checkpoint 在 src/orchestrator/checkpoint.rs 中实现，消费 SummaryStore::load_all 从 KB 重建 Agent 上下文）
✅ 集成测试（3 个：无摘要时的空上下文、有 final 时的重建、有分层摘要时的完整重建）
```

### 阶段 4 遗留项（低优先级）
- 性能测试：摘要聚合延迟、额外 token 开销

---

## 9. 测试计划

### 9.1 单元测试

| 测试 | 描述 | 覆盖文件 |
|------|------|---------|
| `test_context_budget_report` | 验证预算报告的正确性 | `context.rs` |
| `test_context_pressure_levels` | 验证各压力等级的计算 | `context.rs` |
| `test_summarize_compression` | 验证摘要压缩的正确性 | `compressor.rs` |
| `test_hybrid_compression` | 验证混合压缩的正确性 | `compressor.rs` |
| `test_compression_strategy_selection` | 验证策略选择逻辑 | `compressor.rs` |
| `test_subagent_budget_propagation` | 验证子代理预算传递 | `agent/mod.rs` |
| `test_checkpoint_rebuild_context` | 验证上下文重建 | `orchestrator/` |
| `test_summary_storage_and_retrieval` | 验证摘要存储和检索 | `compressor.rs` |

### 9.2 集成测试

| 测试 | 描述 |
|------|------|
| `test_long_running_with_small_context` | 模拟 4K 上下文模型运行 50 轮迭代 |
| `test_crash_recovery_with_context` | 模拟崩溃后恢复，验证上下文重建 |
| `test_subagent_budget_awareness` | 子代理在预算紧张时主动压缩 |
| `test_summary_quality` | 验证摘要是否保留关键信息 |

### 9.3 性能测试

| 测试 | 指标 | 目标 |
|------|------|------|
| 摘要压缩延迟 | 每次摘要调用的耗时 | < 2 秒 |
| 上下文恢复延迟 | 从检查点恢复的耗时 | < 1 秒 |
| 预算查询延迟 | context_budget 工具调用耗时 | < 10ms |
| 额外 token 开销 | 预算管理带来的额外 token 消耗 | < 总 token 的 5% |

---

## 10. 风险评估

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|---------|
| 摘要压缩丢失关键信息 | 中 | 高 | 保留最近 N 轮完整对话；摘要存入 KB 可回溯 |
| LLM 调用摘要增加延迟和成本 | 高 | 中 | 仅在 Critical 压力下触发，限制摘要频率 |
| 预算管理增加系统提示词体积 | 中 | 低 | 指引部分约 200 tokens，影响可忽略 |
| 模型不理解预算指引 | 高 | 中 | 通过系统提示词训练；提供 context_budget 工具 |
| 子代理预算传递增加复杂度 | 低 | 中 | 简单的 Option 字段，不影响现有调用 |
| 分层摘要存储膨胀 | 中 | 低 | 限制摘要保留数量，旧摘要自动清理 |

---

## 11. 附录

### 11.1 与现有设计文档的关系

| 文档 | 关系 |
|------|------|
| `docs/subagent-design.md` | 子代理架构基础，本设计在此基础上增加预算传递 |
| `docs/long-running-tasks-design.md` | 长时间任务执行基础，本设计优化其中的上下文管理 |
| `docs/large-scale-task-architecture.md` | 大规模任务架构，本设计聚焦于小上下文场景的优化 |
| 本设计 | 填补上述文档在"小上下文模型"场景下的空白 |

### 11.2 关键决策记录

| 决策 | 方案 | 理由 |
|------|------|------|
| 预算信息通过工具获取而非自动注入 | 工具调用 | 避免不必要的 token 消耗，按需查询 |
| 摘要压缩使用 LLM 而非本地算法 | LLM 生成 | 保证摘要质量，保留语义信息 |
| 压缩策略根据压力等级自动选择 | 自动选择 | 减少 Agent 的决策负担，行为可预测 |
| 摘要存储到 KB 而非独立存储 | KB 复用 | 利用现有 KB 的检索和存储机制 |
| 保留最近 N 轮完整对话 | 混合策略 | 保证当前任务的完整上下文不丢失 |