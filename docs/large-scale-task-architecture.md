# Dev-Assistant 大规模任务架构设计方案

## 1. 概述

### 1.1 目标

将 dev-assistant-rs 从"单轮对话内执行有限步骤"的助手，升级为**能够自主完成大型软件开发任务**的智能体系统。典型场景包括：

- 开发一个完整的游戏引擎
- 构建一个操作系统内核
- 对大型代码库进行全面重构
- 自动化完成跨多模块的复杂功能开发

### 1.2 关键挑战

| 挑战 | 说明 | 应对策略 |
|------|------|---------|
| **上下文窗口有限** | LLM 一次只能看到有限 token | 子代理隔离 + KnowledgeBase 检索式注入 |
| **任务规模巨大** | 数万文件、数百模块、多阶段 | 分层任务分解 + 并行执行 |
| **状态持久化** | 任务跨天/周，崩溃需恢复 | 检查点 + 结构化知识持久化 |
| **信息共享** | 不同阶段/模块间需要共享知识 | KnowledgeBase 作为统一的信息中枢 |
| **System Prompt 膨胀** | 动态 prompt 导致缓存失效和身份漂移 | 静态身份 + 动态上下文分离 |

### 1.3 设计原则

1. **增量构建**：每一步都独立可用，不依赖后续阶段
2. **最小侵入**：尽量复用现有机制，不重构核心 Agent 循环
3. **LLM 原生**：格式和接口设计符合 LLM 的读写习惯（Markdown > JSON）
4. **显式而非隐式**：信息流动通过工具调用显式发生，而非隐式塞入 prompt

---

## 2. 整体架构

```
┌──────────────────────────────────────────────────────────────────────────┐
│                          ProjectOrchestrator                              │
│                                                                            │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────────────┐   │
│  │   TaskPlanner    │  │   TaskScheduler  │  │    CheckpointManager   │   │
│  │  - 任务分解      │  │  - 依赖图遍历    │  │  - 保存/恢复状态      │   │
│  │  - 依赖分析      │  │  - 并行调度      │  │  - 崩溃恢复           │   │
│  │  - 优先级排序    │  │  - 资源分配      │  │  - 版本兼容           │   │
│  └─────────────────┘  └─────────────────┘  └─────────────────────────┘   │
│                                                                            │
│  ┌──────────────────────────────────────────────────────────────────────┐ │
│  │                         Agent 池                                      │ │
│  │                                                                        │ │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────┐  │ │
│  │  │  Architect   │  │ Implementer  │  │   Reviewer   │  │  Tester  │  │ │
│  │  │  Agent       │  │  Agent × N   │  │   Agent      │  │  Agent   │  │ │
│  │  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘  └────┬─────┘  │ │
│  │         │ 递归             │ 并行             │               │        │ │
│  │         ▼                  ▼                  ▼               ▼        │ │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────┐  │ │
│  │  │  Sub-Agent   │  │  Sub-Agent   │  │  Sub-Agent   │  │Sub-Agent │  │ │
│  │  │  (depth=2)   │  │  (depth=2)   │  │  (depth=2)   │  │(depth=2) │  │ │
│  │  └──────────────┘  └──────────────┘  └──────────────┘  └──────────┘  │ │
│  └──────────────────────────────────────────────────────────────────────┘ │
│                                    │ 读写                                  │
│                                    ▼                                      │
│  ┌──────────────────────────────────────────────────────────────────────┐ │
│  │                         KnowledgeBase                                 │ │
│  │                                                                        │ │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────┐  │ │
│  │  │  决策记录    │  │  模块接口    │  │  摘要缓存    │  │ 问题追踪 │  │ │
│  │  │  (ADR)       │  │  (API)       │  │  (Summaries) │  │ (Issues) │  │ │
│  │  └──────────────┘  └──────────────┘  └──────────────┘  └──────────┘  │ │
│  └──────────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────────┘
```

### 2.1 三层架构

| 层 | 职责 | 关键组件 | 状态 |
|-----|------|---------|------|
| **Orchestrator** | 任务分解、调度、恢复 | `TaskPlanner`, `TaskScheduler`, `CheckpointManager` | 全局持久化 |
| **Agent 池** | 执行具体任务 | `Architect Agent`, `Implementer Agent`, `Reviewer Agent`, `Tester Agent` | 无状态，每次新建 |
| **KnowledgeBase** | 结构化知识存储和检索 | `.kb/` 目录，`kb_query` 工具，`kb_store` 工具 | 持久化文件 |

### 2.2 与现有架构的关系

```
现有架构：
  App → Agent(single) → ContextManager → ConversationHistory

新架构：
  App → Orchestrator → Agent Pool → Agent(instance) → ContextManager
                         ↓
                   KnowledgeBase ← → Agent(instance) 读写
```

关键变化：
- **Agent 从单例变为实例池**：每个子任务创建新的 Agent 实例，执行完毕后销毁
- **ConversationHistory 从主存储降级为临时缓存**：永久知识存储在 KnowledgeBase
- **Orchestrator 新增**：接管任务调度职责，Agent 只负责执行

---

## 3. 子代理系统

### 3.1 设计要点

继承自 `docs/subagent-design.md`，但做了以下改进：

| 要点 | 原设计 | 改进后 |
|------|--------|--------|
| **System Prompt** | 动态生成（含任务描述） | 静态身份 + 任务描述作为首条 User Message |
| **信息共享** | 仅返回文本摘要 | 子代理读写 KnowledgeBase，通过 KB 共享 |
| **工具注册** | 白名单过滤 | 基于任务模板的预定义工具集 |
| **结果结构** | 纯文本 `ToolResult.content` | 结构化结果（文件变更、接口定义、问题列表） |

### 3.2 Agent 身份定义

每个 Agent 类型有固定的 System Prompt，只定义**身份和行为规则**，不包含动态内容：

```rust
/// Agent 身份模板。System Prompt 只包含身份定义，不包含任务描述。
enum AgentIdentity {
    /// 架构师：设计模块结构、接口、数据流
    Architect,
    /// 实现者：按规范实现代码
    Implementer,
    /// 审查者：审查代码质量、安全、一致性
    Reviewer,
    /// 测试者：编写测试、运行测试、报告结果
    Tester,
    /// 调试者：分析编译错误、测试失败、修复 bug
    Debugger,
}

impl AgentIdentity {
    /// 返回固定的 System Prompt（不包含任何动态内容）
    fn system_prompt(&self) -> &'static str {
        match self {
            AgentIdentity::Architect => {
                "你是一个软件架构师。你的职责是设计模块结构、接口定义和数据流。\n\
                 规则：\n\
                 1. 设计完成后，将接口定义写入 KnowledgeBase\n\
                 2. 记录关键架构决策（ADR）到 KnowledgeBase\n\
                 3. 使用 finish 工具结束任务"
            }
            AgentIdentity::Implementer => {
                "你是一个实现者。你的职责是按接口规范实现代码。\n\
                 规则：\n\
                 1. 实现前先从 KnowledgeBase 读取接口定义\n\
                 2. 实现后更新 KnowledgeBase 中的模块摘要\n\
                 3. 确保代码编译通过\n\
                 4. 使用 finish 工具结束任务"
            }
            // ... 其他身份
        }
    }
}
```

### 3.3 子代理创建流程

```
Orchestrator 或父 Agent 决定创建子代理
  │
  ├─ 1. 选择 Agent 身份 (Architect / Implementer / ...)
  │
  ├─ 2. 构建 System Prompt：固定身份文本
  │
  ├─ 3. 构建首条消息：任务描述 + 相关 KB 引用
  │     "任务目标：实现物理引擎的碰撞检测模块
  │      参考接口定义：.kb/interfaces/physics-api.md
  │      相关决策：.kb/decisions/ADR-001-use-ecs.md"
  │
  ├─ 4. 选择工具集：基于身份预定义
  │     Architect → read_file, write_file, kb_store, kb_query, glob, finish
  │     Implementer → read_file, write_file, edit_file, exec_command, kb_query, finish
  │     Reviewer → read_file, batch_read_files, kb_store, kb_query, finish
  │
  ├─ 5. 创建 Agent 实例，分配 token 预算
  │
  └─ 6. 执行，完成后返回结构化的结果摘要
```

### 3.4 深度限制

```
MAX_SUBAGENT_DEPTH = 3

深度 0: Orchestrator（非 Agent，不消耗深度）
深度 1: 专业 Agent（Architect / Implementer / ...）
深度 2: 子 Agent（由专业 Agent 通过 spawn_subagent 创建）
深度 3: 孙 Agent（极少需要，用于极小粒度的子任务）
```

超过深度限制时，返回 `AppError::SubagentDepthLimit`，强制父 Agent 自行完成任务。

---

## 4. KnowledgeBase 设计

### 4.1 存储格式

**主存储：Markdown 文件 + YAML Frontmatter**

复用 `skills/` 目录的现有模式：

```markdown
---
type: decision                    # 条目类型
id: ADR-001                       # 唯一标识
title: 使用 ECS 架构              # 标题
status: accepted                  # 状态: proposed / accepted / deprecated / superseded
tags: [architecture, rendering]   # 标签
created: 2026-07-19T10:00:00Z     # 创建时间
updated: 2026-07-19T11:00:00Z     # 更新时间
author: architect                 # 创建者角色
relates_to: [ADR-002, interface-renderer-api]  # 关联条目
depends_on: [ADR-003]             # 依赖的条目
supersedes: []                    # 替代的条目
---
# 使用 ECS 架构

## 背景
渲染引擎需要支持多种实体类型...

## 决策
采用 Entity-Component-System 模式...

## 理由
- 更好的数据局部性（缓存友好）
- 更灵活的实体组合
```

### 4.2 目录结构

```
.kb/                                    # KnowledgeBase 根目录
├── index.json                          # 索引文件（程序化更新）
│
├── decisions/                          # 架构决策记录 (ADR)
│   ├── ADR-001-use-ecs.md
│   └── ADR-002-choose-wgpu.md
│
├── interfaces/                         # 模块接口定义
│   ├── renderer-api.md
│   └── physics-api.md
│
├── summaries/                          # 模块摘要（自动生成并更新）
│   ├── src-renderer-vulkan.md
│   └── src-physics-rigidbody.md
│
├── issues/                             # 问题追踪
│   ├── BUG-001-descriptor-leak.md
│   └── TODO-002-texture-compression.md
│
├── progress/                           # 任务进度
│   └── current-task.md
│
└── templates/                          # 任务模板
    ├── architect-task.md
    └── implementer-task.md
```

### 4.3 索引文件

```json
{
  "version": 1,
  "updated": "2026-07-19T11:00:00Z",
  "entries": {
    "ADR-001": {
      "path": "decisions/ADR-001-use-ecs.md",
      "type": "decision",
      "title": "使用 ECS 架构",
      "tags": ["architecture", "rendering"],
      "status": "accepted",
      "relates_to": ["ADR-002", "interface-renderer-api"]
    },
    "interface-renderer-api": {
      "path": "interfaces/renderer-api.md",
      "type": "interface",
      "title": "渲染引擎接口定义",
      "tags": ["rendering", "api"],
      "status": "draft"
    }
  }
}
```

### 4.4 工具定义

#### kb_store — 创建/更新 KB 条目

```json
{
  "name": "kb_store",
  "description": "创建或更新 KnowledgeBase 条目。用于记录架构决策、模块接口定义、问题追踪等。",
  "parameters": {
    "type": "object",
    "properties": {
      "path": {
        "type": "string",
        "description": "条目路径，如 'decisions/ADR-001-use-ecs.md'"
      },
      "content": {
        "type": "string",
        "description": "完整的 Markdown 内容（含 YAML frontmatter）"
      },
      "update_index": {
        "type": "boolean",
        "description": "是否自动更新 index.json",
        "default": true
      }
    },
    "required": ["path", "content"]
  }
}
```

#### kb_query — 检索 KB 条目

```json
{
  "name": "kb_query",
  "description": "检索 KnowledgeBase 条目。支持按标签、类型、关键词过滤。",
  "parameters": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "搜索关键词"
      },
      "type": {
        "type": "string",
        "enum": ["decision", "interface", "summary", "issue", "any"],
        "description": "条目类型过滤",
        "default": "any"
      },
      "tags": {
        "type": "array",
        "items": { "type": "string" },
        "description": "标签过滤（满足任一即匹配）"
      },
      "max_results": {
        "type": "integer",
        "description": "最大返回结果数",
        "default": 5
      },
      "include_content": {
        "type": "boolean",
        "description": "是否包含条目内容",
        "default": false
      }
    },
    "required": []
  }
}
```

### 4.5 检索算法

```rust
/// KnowledgeBase 检索算法。
/// 不依赖外部向量数据库，使用简单的标签 + 关键词匹配。
fn kb_search(
    index: &Index,
    query: &str,
    type_filter: Option<&str>,
    tag_filter: Option<&[String]>,
    max_results: usize,
) -> Vec<Entry> {
    let query_lower = query.to_lowercase();
    let mut scored: Vec<(i32, &Entry)> = Vec::new();

    for entry in index.entries.values() {
        let mut score = 0i32;

        // 1. 类型过滤
        if let Some(t) = type_filter {
            if t != "any" && entry.type_name != t {
                continue;
            }
        }

        // 2. 标签匹配
        if let Some(tags) = tag_filter {
            if !tags.iter().any(|t| entry.tags.contains(t)) {
                continue;
            }
            score += 10; // 标签匹配是高权重信号
        }

        // 3. 关键词匹配
        if !query_lower.is_empty() {
            if entry.title.to_lowercase().contains(&query_lower) {
                score += 5; // 标题命中
            }
            if entry.id.to_lowercase().contains(&query_lower) {
                score += 3; // ID 命中
            }
            // 如果 include_content 为 true，还会搜索内容
        }

        // 4. 状态优先级
        if entry.status == "accepted" || entry.status == "completed" {
            score += 2; // 已接受/已完成的状态优先
        }

        scored.push((score, entry));
    }

    // 按分数降序，取 top N
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter()
        .take(max_results)
        .map(|(_, entry)| entry.clone())
        .collect()
}
```

### 4.6 与现有模式的复用关系

| KB 组件 | 复用的现有代码 | 改动量 |
|---------|--------------|--------|
| YAML frontmatter 解析 | `skills/mod.rs::parse_frontmatter()` | 提取为公共函数 |
| 目录遍历发现 | `skills/mod.rs::discover_skills()` | 通用化 |
| 文件读写 | 现有 `read_file`/`write_file` 工具 | 0 |
| JSON 序列化 | 现有 `serde_json` | 0 |
| 上下文注入 | `agent/mod.rs` skill activation 机制 | 小改动 |

---

## 5. System Prompt 策略

### 5.1 原则：分离身份和上下文

```
System Prompt 只放"整个会话期间不变"的东西
```

| 内容 | 归属 | 示例 |
|------|------|------|
| Agent 身份 | ✅ System Prompt（固定） | "你是一个架构师..." |
| 安全规则 | ✅ System Prompt（固定） | "遵守父代理的安全策略" |
| 行为规则 | ✅ System Prompt（固定） | "完成后使用 finish 工具" |
| 任务描述 | ❌ 首条 User Message（动态） | "任务目标：实现物理引擎" |
| 项目上下文 | ❌ 检索后注入的消息（动态） | "接口定义见 KB-123" |
| 进度信息 | ❌ 不作为 prompt 内容 | 存储在 KB progress 中 |
| 工具 schema | ❌ API `tools` 参数 | 不占用 prompt 空间 |

### 5.2 子 Agent 的 System Prompt

```rust
/// 子 Agent 的 System Prompt 只包含身份定义。
/// 任务描述和上下文通过首条 User Message 传递。
fn build_subagent_system_prompt(identity: &AgentIdentity) -> String {
    // 固定文本，不包含任何动态内容
    format!(
        "{} \n\n通用规则：\n\
         1. 专注完成分配的任务\n\
         2. 完成后必须使用 finish 工具结束\n\
         3. 不要调用 spawn_subagent 工具（除非确实需要分解子任务）\n\
         4. 重要信息写入 KnowledgeBase\n\
         5. 遵守安全策略",
        identity.system_prompt()
    )
}

/// 首条 User Message 包含所有动态内容。
fn build_initial_message(task: &str, kb_refs: &[String]) -> LlmMessage {
    let mut content = format!("任务目标：{}\n\n", task);

    if !kb_refs.is_empty() {
        content.push_str("参考资料：\n");
        for ref_ in kb_refs {
            content.push_str(&format!("- {}\n", ref_));
        }
    }

    LlmMessage {
        role: "user".to_string(),
        content: Some(content),
        tool_calls: None,
        tool_call_id: None,
    }
}
```

### 5.3 优势

1. **缓存命中率高**：System Prompt 不变，LLM 提供商的前缀缓存可以复用
2. **身份稳定**：LLM 不会因 prompt 变化而"迷失自我"
3. **审计清晰**：System Prompt 可追溯，首条 User Message 记录了任务来源
4. **工具 schema 独立**：通过 API `tools` 参数传递，不占用 prompt 空间

---

## 6. 长期运行与检查点

### 6.1 执行模式

```rust
/// Orchestrator 的任务执行入口。
/// 管理长时间运行的任务，支持检查点、恢复、中断。
pub struct TaskOrchestrator {
    /// 任务队列（含依赖图）
    task_queue: TaskQueue,
    /// 项目知识库
    kb: KnowledgeBase,
    /// 当前执行的 Agent 实例
    active_agents: Vec<RunningAgent>,
    /// 检查点管理器
    checkpoint: CheckpointManager,
}

impl TaskOrchestrator {
    /// 启动一个大规模任务。
    ///
    /// 1. 将顶层任务分解为子任务（使用 Architect Agent）
    /// 2. 构建依赖图
    /// 3. 按依赖顺序调度执行
    /// 4. 支持并行执行独立子任务
    pub async fn execute(&mut self, goal: &str) -> Result<ProjectResult, AppError> {
        // Phase 1: 任务分解
        let tasks = self.decompose_goal(goal).await?;

        // Phase 2: 构建依赖图
        let graph = DependencyGraph::build(&tasks);

        // Phase 3: 按依赖图调度
        while let Some(ready) = graph.next_ready() {
            // 并行执行无依赖的任务
            let results: Vec<_> = ready.into_iter()
                .map(|task| self.execute_task(task))
                .collect();

            for result in results {
                // 更新 KnowledgeBase
                self.kb.record_result(&result)?;
                // 保存检查点
                self.checkpoint.save(&self.kb, &graph)?;
                // 标记任务完成
                graph.complete(result.task_id);
            }
        }

        // Phase 4: 生成总结
        self.generate_summary().await
    }
}
```

### 6.2 检查点策略

检查点 = KnowledgeBase（完整知识状态）+ 任务队列（进度状态）

```rust
pub struct Checkpoint {
    /// 知识库索引快照
    pub kb_index: KbIndex,
    /// 任务依赖图状态
    pub task_graph: TaskGraph,
    /// 已完成的子任务列表
    pub completed_tasks: Vec<TaskId>,
    /// 进行中的子任务
    pub in_progress: Vec<RunningTask>,
    /// 检查点创建时间
    pub timestamp: SystemTime,
    /// 版本号，用于向后兼容
    pub version: String,
}
```

**恢复流程**：

```
1. 加载检查点
2. 恢复 KnowledgeBase 索引
3. 恢复任务依赖图
4. 标记 in_progress 任务为 pending（需要重新执行）
5. 从 pending 任务继续调度
```

### 6.3 错误恢复

| 错误类型 | 处理方式 |
|---------|---------|
| 子 Agent 执行失败 | 重试 3 次，仍然失败则标记为 failed，继续执行其他任务 |
| LLM API 超时 | 指数退避重试，最多 5 次 |
| 程序崩溃 | 下次启动时从最近的检查点恢复 |
| 用户中断 | 保存检查点后退出，下次可恢复 |

---

## 7. 实现路径

### Phase 1：子代理机制（最小可用）

**目标**：LLM 能够通过 `spawn_subagent` 工具创建子 Agent 执行独立任务。

| 步骤 | 内容 | 涉及文件 | 工作量 |
|------|------|---------|--------|
| 1.1 | Agent 添加 `depth` 字段，`llm` 改为 `Arc<LlmClient>` | `src/agent/mod.rs` | 小 |
| 1.2 | 实现 `new_subagent()` 构造函数（静态 system prompt + 任务描述作为首条消息） | `src/agent/mod.rs` | 中 |
| 1.3 | 在 `process_tool_calls` 中拦截 `spawn_subagent` | `src/agent/mod.rs` | 小 |
| 1.4 | 新增 `spawn_subagent` 工具定义 | `src/tools/subagent.rs` | 小 |
| 1.5 | `ToolRegistry` 新增 `new_subagent_registry()` 方法 | `src/tools/mod.rs` | 小 |
| 1.6 | 注册 `spawn_subagent` 工具 | `src/tools/mod.rs` | 小 |
| 1.7 | 添加 `SubagentDepthLimit` 错误类型 | `src/utils/error.rs` | 小 |
| 1.8 | 更新系统提示词 | `src/prompt.rs` | 小 |
| 1.9 | 测试：子代理创建、深度限制、工具过滤、上下文隔离 | 多个文件 | 中 |

**交付物**：LLM 可以调用 `spawn_subagent` 创建子 Agent，子 Agent 独立执行任务后返回摘要。

### Phase 2：KnowledgeBase 基础

**目标**：LLM 能够通过工具读写结构化知识，实现信息共享。

| 步骤 | 内容 | 涉及文件 | 工作量 |
|------|------|---------|--------|
| 2.1 | 将 frontmatter 解析从 `skills/mod.rs` 提取为公共函数 | `src/utils/frontmatter.rs` | 小 |
| 2.2 | 实现 `kb_store` 工具（创建/更新 KB 条目 + 维护 index.json） | `src/tools/kb.rs` | 中 |
| 2.3 | 实现 `kb_query` 工具（标签 + 关键词检索） | `src/tools/kb.rs` | 中 |
| 2.4 | 在 `ToolRegistry` 中注册 `kb_store` 和 `kb_query` | `src/tools/mod.rs` | 小 |
| 2.5 | 更新系统提示词，告知 LLM 如何使用 KB | `src/prompt.rs` | 小 |
| 2.6 | 测试：KB 条目创建、检索、索引更新 | 多个文件 | 中 |

**交付物**：LLM 可以创建、查询、更新 KnowledgeBase 条目。子 Agent 可以通过 KB 共享信息。

### Phase 3：Orchestrator + 长期运行

**目标**：支持大规模任务的分解、调度、检查点恢复。

| 步骤 | 内容 | 涉及文件 | 工作量 |
|------|------|---------|--------|
| 3.1 | 实现 `TaskQueue` 和 `DependencyGraph` 数据结构 | `src/orchestrator/task.rs` | 中 |
| 3.2 | 实现 `TaskOrchestrator` 核心调度循环 | `src/orchestrator/mod.rs` | 大 |
| 3.3 | 实现 `CheckpointManager`（保存/恢复） | `src/orchestrator/checkpoint.rs` | 中 |
| 3.4 | 实现 `run_background()` 入口 | `src/orchestrator/mod.rs` | 中 |
| 3.5 | 实现 `task_status` / `pause_task` / `cancel_task` 工具 | `src/tools/task_tools.rs` | 中 |
| 3.6 | 更新 `app.rs`，新增后台模式入口 | `src/app.rs` | 小 |
| 3.7 | 更新 `repl.rs`，新增 `/background` 等 slash 命令 | `src/repl.rs` | 小 |
| 3.8 | 测试：任务分解、依赖调度、检查点恢复、中断恢复 | 多个文件 | 大 |

**交付物**：完整的长时间任务执行能力，Orchestrator 自动分解、调度、恢复。

### Phase 4：专业 Agent 模板

**目标**：预定义的专业 Agent 类型，提高任务执行质量。

| 步骤 | 内容 | 涉及文件 | 工作量 |
|------|------|---------|--------|
| 4.1 | 实现 `AgentIdentity` 枚举和固定 System Prompt | `src/agent/identity.rs` | 小 |
| 4.2 | 定义各专业 Agent 的默认工具集 | `src/agent/identity.rs` | 小 |
| 4.3 | 任务模板系统（从 KB templates/ 加载） | `src/agent/template.rs` | 中 |
| 4.4 | 增量编译和测试集成 | `src/orchestrator/build.rs` | 中 |
| 4.5 | 端到端测试：完整游戏引擎开发流程模拟 | 测试文件 | 大 |

**交付物**：完整的专业 Agent 系统，支持自动化大型软件开发。

---

## 8. 与现有代码的兼容性

| 现有组件 | 变化 | 兼容性 |
|---------|------|--------|
| `Agent::run()` | 保持不变，仍然可用 | ✅ 完全向后兼容 |
| `Agent::step()` | 保持不变 | ✅ 完全向后兼容 |
| `ContextManager` | 保持不变 | ✅ 完全向后兼容 |
| `ConversationHistory` | 保持不变 | ✅ 完全向后兼容 |
| `SessionStore` | 保持不变（可作为 KB 事件的日志后端） | ✅ 完全向后兼容 |
| `ToolRegistry` | 新增方法，不修改现有 | ✅ 完全向后兼容 |
| `ToolHandler` 签名 | 保持不变 | ✅ 完全向后兼容 |
| 工具定义模式 | 保持不变 | ✅ 完全向后兼容 |
| 配置格式 | 保持不变 | ✅ 完全向后兼容 |
| 状态文件 | 新检查点文件，不影响现有 `.dev-assistant-state.json` | ✅ 完全向后兼容 |

---

## 9. 附录：与现有设计文档的关系

| 文档 | 与本方案的关系 | 采纳的内容 | 改进的内容 |
|------|--------------|-----------|-----------|
| `docs/subagent-design.md` | 子代理部分的直接基础 | `spawn_subagent` 工具、递归深度限制、上下文隔离 | System Prompt 策略（静态身份 + 动态任务描述） |
| `docs/long-running-tasks-design.md` | Orchestrator 部分的参考 | 后台执行、检查点、进度追踪、任务管理工具 | 增加了 KnowledgeBase 作为共享信息中枢 |
| 本方案 | 综合设计 | - | 三层架构、KnowledgeBase 设计、System Prompt 策略、分阶段实现路径 |