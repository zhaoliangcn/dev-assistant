---
type: decision
title: 定时任务模块架构设计
tags: [scheduler, architecture, design]
---

# 定时任务模块架构

## 模块结构
```
src/scheduler/
  mod.rs      - 模块入口，重新导出
  task.rs     - 已有：核心数据结构（ScheduledTask, ExecutionRecord等）
  tools.rs    - 新增：cron 解析、ID 生成等工具函数
  store.rs    - 新增：任务持久化存储（JSON文件）
  engine.rs   - 新增：调度引擎（后台循环执行任务）
```

## 设计决策
1. **调度引擎**：使用 tokio 后台任务，每秒 tick 一次检查到期任务
2. **持久化**：任务和执行记录存储在 `.kb/scheduler/` 目录下的 JSON 文件中
3. **执行方式**：Agent 模式通过子代理执行，Command 模式通过 exec_command 执行
4. **集成**：通过 REPL 的 `/schedule` 命令管理，App 启动时自动初始化引擎
5. **Cron 解析**：支持标准 5 字段 cron 表达式，使用 chrono 库计算