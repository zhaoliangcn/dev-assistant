---
type: decision
status: accepted
title: 流水线 (/pipeline) 命令设计与实现
tags: [pipeline, slash-command, subagent, workflow]
author: dev-assistant
created: 2026-07-26
---

# ADR-002: 流水线 (/pipeline) 命令设计与实现

## 背景

用户需要一个自动化工作流，将一个大型任务自动分解为多个阶段，由不同类型的子代理依次执行。

## 阶段定义

流水线包含 5 个阶段，按顺序执行，上一个阶段的输出作为下一个阶段的上下文：

| 阶段 | 角色 | 工具集 | 职责 |
|------|------|--------|------|
| 1. 🏗 架构设计 | Architect | read/write/glob/kb | 设计模块结构、接口定义、数据流 |
| 2. 💻 代码实现 | Implementer | read/write/edit/exec/glob/kb | 按架构实现代码和单元测试 |
| 3. 🔍 代码审查 | Reviewer | read/batch-read/glob/kb | 审查代码质量、安全、性能 |
| 4. 🔧 问题修复 | Debugger | read/write/edit/exec/glob/kb | 修复审查发现的问题 |
| 5. 📋 进度记录 | General | read/write/exec/glob/kb | 记录进度、git commit |

## 数据流

```
用户输入: /pipeline <任务描述>
    │
    ▼
Agent::run_pipeline(task, verbose)
    │
    ├── Stage 1: Architect ──→ 输出架构设计文档
    │         │
    │         ▼ (context = Stage1 输出)
    ├── Stage 2: Implementer ──→ 输出代码实现
    │         │
    │         ▼ (context = Stage2 输出)
    ├── Stage 3: Reviewer ──→ 输出审查报告
    │         │
    │         ▼ (context = Stage3 输出)
    ├── Stage 4: Debugger ──→ 输出修复后的代码
    │         │
    │         ▼ (context = Stage4 输出)
    └── Stage 5: General ──→ 记录进度 + git commit
```

## 实现方案

### 修改的文件

| 文件 | 修改内容 |
|------|----------|
| `src/agent/mod.rs` | 新增 `PipelineStage` 结构体 + `run_pipeline()` 异步方法 |
| `src/repl.rs` | 新增 `handle_pipeline_command()` 函数 |
| `src/app.rs` | `run_interactive()` 中拦截 `/pipeline` 命令 |

### 关键设计决策

1. **PipelineStage 结构体**：在 `impl Agent` 块外定义，包含 name、agent_type、task_template、max_iterations
2. **上下文传递**：通过 `{context}` 占位符替换，将上一阶段的 finish 输出注入下一阶段的任务模板
3. **错误处理**：任一阶段失败 → 立即终止流水线，返回错误信息
4. **子代理深度**：`self.depth + 1 = 1`（主 Agent 深度为 0），不超过 MAX_SUBAGENT_DEPTH 限制
5. **最大迭代次数**：各阶段不同（10~20），设计/审查需要的轮次少，实现需要更多

### 命令语法

```
/pipeline <任务描述>
```

示例：
```
/pipeline 实现一个 RBAC 权限控制系统
/pipeline 为项目添加日志系统
```

## 已实现的状态

- [x] `src/agent/mod.rs`: `PipelineStage` 结构体 + `run_pipeline()` 方法
- [x] `src/repl.rs`: `handle_pipeline_command()` 函数
- [x] `src/app.rs`: `run_interactive()` 中 `/pipeline` 命令处理
- [x] 编译验证：148 个测试全部通过，pulldown-cmark API 兼容性已修复

## 实现细节补充

### PipelineStage 结构体位置

`PipelineStage` 结构体已移至 `src/agent/identity.rs`，与 `AgentIdentity` 放在一起，便于统一管理代理相关定义。`src/agent/mod.rs` 通过 `pub use identity::{AgentIdentity, PipelineStage}` 重新导出。

### UI 集成

流水线命令已集成新的 UI 块级渲染系统：
- 启动消息使用 `MessageBlock::System` 渲染
- 错误消息使用 `MessageBlock::Error` 渲染
- 各阶段子代理输出通过 `process_user_message()` 的块级渲染机制显示

### 编译验证结果

| 验证项 | 结果 |
|--------|------|
| `cargo build` | ✅ 成功 |
| `cargo test` | ✅ 148 个测试通过 |
| `cargo clippy` | ✅ 无错误（仅有未使用代码警告） |