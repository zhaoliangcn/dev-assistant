//! 定时任务模块。
//!
//! 提供基于时间轮 (TimingWheel) 的后台定时任务调度能力。
//! 支持 cron 表达式、固定间隔、一次性延迟三种调度方式。
//! 支持 Agent 子代理执行和 Shell 命令执行两种执行模式。
//!
//! # 模块结构
//!
//! - `task.rs` — 核心数据结构 (ScheduledTask, ExecutionRecord)
//! - `tools.rs` — 工具函数 (cron 解析, ID 生成)
//! - `wheel.rs` — 时间轮 (TimingWheel)
//! - `store.rs` — 持久化存储 (JSONL)
//! - `handler.rs` — 任务处理器 trait + 内置实现
//! - `scheduler.rs` — 调度器主循环
//! - `executor.rs` — 执行器 (派发到 Handler)
//! - `tools_handlers.rs` — 工具 handler（CRUD + 日志查询）

// 模块尚未在 App 中初始化，允许 dead_code 避免未使用类型/方法的警告
#![allow(dead_code)]

pub mod task;
pub mod tools;
pub mod wheel;
pub mod store;
pub mod handler;
pub mod engine;
pub mod executor;
pub mod tools_handlers;