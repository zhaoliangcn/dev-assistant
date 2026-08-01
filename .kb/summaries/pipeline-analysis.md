---
type: summary
status: completed
title: Pipeline 功能分析报告
tags: [pipeline, analysis, feature, dev-assistant-rs]
author: dev-assistant
created: 2026-08-01
---

# Pipeline 功能分析报告

## 概述

Pipeline（流水线）是一个**自动化多阶段工作流**功能，用户通过 `/pipeline <任务描述>` 一条命令，即可触发一个由多种类型子代理按顺序执行、自动衔接的完整工作流。

---

## 1. 架构设计

### 1.1 核心组件

| 组件 | 位置 | 职责 |
|------|------|------|
| `PipelineStage` | `src/agent/identity.rs` | 阶段定义：名称、代理类型、任务模板、最大迭代次数 |
| `PipelineContext` | `src/agent/pipeline_context.rs` | 全局/阶段上下文数据结构，含执行状态、摘要、产物清单 |
| `PipelineContextStore` | `src/agent/pipeline_context.rs` | 文件系统持久化：保存/加载上下文、检查点管理 |
| `Agent::run_pipeline()` | `src/agent/mod.rs` | 流水线主循环：阶段遍历、子代理创建、错误处理 |
| `/pipeline` 命令处理 | `src/app.rs` | 用户入口，拦截 `/pipeline` 并调用 `run_pipeline()` |

### 1.2 存储结构

```
.kb/pipeline/
├── context.json              # 全局上下文索引（PipelineContext）
├── checkpoint.json           # 断点信息（用于恢复）
├── stage-0/                  # 架构设计阶段
│   └── summary.json
├── stage-1/                  # 代码实现阶段
│   └── summary.json
├── stage-2/                  # 测试验证阶段
│   └── summary.json
├── stage-3/                  # 代码审查阶段
│   └── summary.json
├── stage-4/                  # 问题修复阶段
│   └── summary.json
└── stage-5/                  # 进度记录阶段
    └── summary.json
```

---

## 2. 6 阶段流水线

实际代码中定义了 **6 个阶段**（相较最初 ADR 设计的 5 个，增加了"🧪 测试验证"阶段）：

| 序号 | 阶段名称 | 代理类型 | 权重 | 迭代分配 | 工具集 |
|------|----------|----------|------|----------|--------|
| 0 | 🏗 架构设计 | Architect | 2/14 | ~17 轮 | read/write/glob/exec/kb |
| 1 | 💻 代码实现 | Implementer | 4/14 | ~34 轮 | read/write/edit/exec/glob/kb |
| 2 | 🧪 测试验证 | Tester | 2/14 | ~17 轮 | read/write/edit/exec/glob/kb |
| 3 | 🔍 代码审查 | Reviewer | 2/14 | ~17 轮 | read/batch-read/glob/kb |
| 4 | 🔧 问题修复 | Debugger | 3/14 | ~26 轮 | read/batch-read/write/edit/exec/glob/kb |
| 5 | 📋 进度记录 | General | 1/14 | ~9 轮 | read/write/exec/glob/kb |

**权重分配**：`2:4:2:2:3:1`，共 14 份。总迭代次数通过环境变量 `MAX_ITERATIONS` 配置（默认 120），每阶段最少 5 轮。

---

## 3. 核心数据流

```
用户输入: /pipeline 实现一个缓存系统
    │
    ▼
app.rs run_interactive()
    │  ┌─ 拦截 /pipeline 前缀
    │  └─ 调用 agent.run_pipeline(&task, verbose, resume=false)
    │
    ▼
Agent::run_pipeline()
    │
    ├── 1. 初始化 PipelineContextStore（.kb/pipeline/）
    ├── 2. 计算迭代权重分配
    ├── 3. 创建或恢复 PipelineContext
    │
    ├── 🔄 阶段循环 (while current_stage < total)
    │   │
    │   ├── 更新 stage.status = InProgress
    │   ├── 渲染进度条
    │   ├── 构建阶段任务模板（替换 {context} 占位符）
    │   ├── 创建子代理 (new_subagent)
    │   ├── 执行子代理 (subagent.run())
    │   ├── 收集子代理输出消息
    │   │
    │   ├── ✅ 成功：
    │   │   ├── status = Completed
    │   │   ├── 记录 summary（来自 finish 工具）
    │   │   ├── git diff 检测修改文件
    │   │   ├── 保存阶段上下文 (save_stage_context)
    │   │   ├── 保存检查点 (save_checkpoint)
    │   │   └── current_stage += 1
    │   │
    │   └── ❌ 失败：
    │       ├── status = Failed
    │       ├── 记录错误信息
    │       ├── 保存检查点
    │       └── 返回错误（提示使用 --resume-pipeline 恢复）
    │
    └── 全部完成：
        ├── 渲染 100% 进度条
        └── 清理检查点 (clear)
```

---

## 4. 上下文传递机制

### 4.1 阶段间上下文

通过 `PipelineContext::build_context_prompt()` 构建上下文提示词，遍历已完成阶段，格式化为 Markdown 摘要。

### 4.2 模板占位符替换

每个阶段的任务模板包含 `{context}` 占位符，在运行时被替换为已完成阶段的上下文摘要。

### 4.3 产物持久化

各阶段子代理通过 `kb_store` 工具将产物保存到 `.kb/pipeline/stage-N/` 目录，后续阶段可通过 `kb_query` 查阅。

---

## 5. 断点续传机制

- **检查点保存**：每个阶段完成后自动保存 `checkpoint.json`
- **检查点恢复**：按 `resume` 参数判断，从已保存的检查点继续
- **完成清理**：流水线全部成功完成后自动清除检查点数据

---

## 6. 关键设计决策

### 6.1 文件系统传递上下文
选择文件系统存储而非纯字符串传递，避免 `finish(summary)` 摘要过长导致 token 浪费，且支持断点续传。

### 6.2 权重分配迭代次数
根据阶段复杂度分配迭代权重：实现阶段（4/14）最高，记录阶段（1/14）最少。

### 6.3 子代理工具限制
- **Architect**: 不允许 `edit_file`（只设计不实现）
- **Reviewer**: 只读工具 + `batch_read_files`
- **Implementer/Debugger**: 完整读写工具集

---

## 7. 潜在问题

1. **CLI 参数缺失**：错误提示中提到了 `--resume-pipeline`，但 CLI 参数中未实现
2. **阶段目录路径不一致**：`stage_dir()` 使用 `stage-{index}`，但任务模板和产物引用使用 `stage-{index}-{name}` 格式
3. **无超时保护**：只有迭代次数限制，没有时间超时保护
4. **产物引用路径不准确**：`artifact_dir` 路径格式与 `stage_dir()` 不一致