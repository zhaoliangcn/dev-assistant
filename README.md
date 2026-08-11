# Dev-Assistant-RS

<div align="center">

**Rust 原生 AI 编程助手 — 代码库级别的 AI 结对编程伙伴**

[![Rust](https://img.shields.io/badge/Rust-1.85%2B-orange)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue)](LICENSE)
[![CI](https://github.com/your-username/dev-assistant-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/your-username/dev-assistant-rs/actions/workflows/ci.yml)

</div>

---

## 简介

**Dev-Assistant-RS** 是一个用 Rust 编写的 AI 编程助手，专注于 **代码库级别的 AI 辅助**。与普通聊天式 AI 不同，它能：

- 🔍 **阅读并理解整个代码库** — 批量分析项目文件，理解代码结构和依赖
- 🔧 **自动修改代码** — 编辑文件、重构代码、编写测试
- 🔄 **流水线执行** — 架构设计 → 代码实现 → 审查 → 修复
- 🛡 **安全可控** — 危险操作审批、路径遍历防护、gitignore 感知
- 🧩 **子代理递归** — 多层子代理并行处理复杂任务（深度 ≤ 3）
- 🌐 **双形态运行** — 终端 REPL 或 Web UI（axum + HTMX + Alpine.js）
- 🧠 **记忆系统** — 自动整理经验，模拟人脑记忆巩固与遗忘

## 快速开始

### 前置条件

- Rust 1.85+
- 一个 LLM API Key（OpenAI / DeepSeek / Anthropic / Ollama / 商汤 等）

### 安装

```bash
git clone https://github.com/zhaoliangcn/dev-assistant.git
cd dev-assistant
cargo build --release
```

### 配置

```bash
# 1. 复制环境变量模板
cp .env.example .env

# 2. 编辑 .env，填入你的 API Key
# LLM_PROVIDER=openai
# LLM_API_KEY=sk-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
# LLM_MODEL=gpt-4o

# 3. （可选）多模型配置
# 复制示例到可执行文件所在目录（如 target/release/）：
cp .dev-assistant-models_example.toml target/release/.dev-assistant-models.toml
# 编辑 target/release/.dev-assistant-models.toml，配置多个模型的 API Key
# （程序从可执行文件所在目录查找该文件，未找到时回退到环境变量）

# 4. （可选）用 --config 指定模型配置文件位置（任意路径，优先级最高）
cargo run --release -- --config /path/to/.dev-assistant-models.toml
```

### 运行

```bash
# 终端交互模式（REPL）
cargo run --release

# 单次执行模式
cargo run --release -- --message "帮我审查一下代码"

# Web UI 模式（浏览器访问 http://127.0.0.1:8080）
cargo run --release -- --web

# 查看完整选项
cargo run --release -- --help
```

## 使用示例

### REPL 交互

```
🚀 Dev-Assistant Rust CLI
Project: /path/to/your-project
Type '/exit' or '/quit' to quit.

> 这个项目有哪些安全风险？

── 第 1 轮 ──
👤 你 │ 这个项目有哪些安全风险？

💭 思考 │ 正在分析代码库...

✅ 结果 │ 工具 batch_read_files 执行成功：读取完成 50/50 文件

🤖 助手 │ 安全审查结果如下：
        │ 1. **命令注入风险**: src/tools/bash.rs:42 未过滤输入...
        │ 2. **路径遍历**: src/tools/file/io.rs:88 缺少规范化...
```

### Web UI

Web UI 模式基于 **axum + HTMX + Alpine.js** 构建，提供与 CLI 同样完整的功能：

```bash
# 启动 Web 服务
cargo run --release -- --web --port 8080
```

- 对话式 AI 辅助（WebSocket 实时流式输出）
- Markdown 渲染 + 代码语法高亮
- Diff 对比可视化
- 文件浏览与管理
- 会话历史管理

### 内建命令

```
/help      — 显示帮助
/history   — 显示对话历史
/clear     — 清屏
/expand    — 展开折叠的内容
/grep      — 搜索文件内容（支持正则）
/model     — 查看/切换 LLM 模型
/pipeline  — 执行流水线任务
/dream     — 手动触发记忆整理（支持 --dry-run 预演）
/verbose   — 切换到详细模式
/quiet     — 切换到安静模式
```

## 架构

### 数据流

```
用户输入 → repl.rs → Agent.step() → LLM API → 工具执行 → 结果渲染
                                                      ↓
Web UI ← axum Router ← WebSocket/REST ← 复用 Agent/LlmClient/ToolRegistry
```

### 核心组件

| 组件 | 职责 |
|------|------|
| **Agent** | 核心引擎，管理对话上下文、工具调度、子代理、流水线、上下文压缩 |
| **LlmClient** | LLM API 客户端，支持多 provider、自动故障转移、429/5xx 指数退避重试 |
| **ToolRegistry** | 工具注册中心（文件操作、搜索、代码分析、符号提取、知识库等） |
| **AsyncToolRegistry** | 异步工具注册表（大文件读写、批量读取，支持进度回调） |
| **SecurityPolicy** | 安全策略层（危险命令拦截、路径规范化验证、gitignore 感知） |
| **Orchestrator** | 任务编排引擎（依赖图、断点续跑、并发控制、后台执行） |
| **Scheduler** | 定时调度引擎（时间轮、持久化任务存储、执行器池） |
| **Dream** | 记忆系统自我整理（采集 → 巩固 → 去重 → 遗忘 → 报告） |
| **HookManager** | Shell hook 注入机制（session-start 等事件） |
| **SessionStore** | 会话持久化（JSONL 格式，append-only） |
| **Subagent** | 递归子代理（深度 ≤ 3），自动排除 spawn 工具防无限递归 |

## 功能特性

| 特性 | CLI | Web UI |
|------|-----|--------|
| 对话式 AI 辅助 | ✅ | ✅ |
| Markdown 渲染 + 代码语法高亮 | ✅ | ✅ |
| Diff 对比（绿/红高亮） | ✅ | ✅ |
| 工具调用智能摘要 | ✅ | ✅ |
| 文件浏览/编辑 | ❌ | ✅ |
| 会话历史管理 | ✅ | ✅ |
| 流水线管理（6 阶段加权进度） | ✅ | ✅ |
| 递归子代理（深度 ≤ 3） | ✅ | ✅ |
| 任务编排（依赖图 + 断点续跑） | ✅ | ✅ |
| 定时调度（时间轮 + 持久化存储） | ✅ | ✅ |
| 记忆系统（自动整理/巩固/遗忘） | ✅ | ❌ |
| Shell hook 注入 | ✅ | ❌ |
| Provider 多模型故障转移 | ✅ | ✅ |
| 429/5xx 自动重试（指数退避+抖动） | ✅ | ✅ |
| 上下文压缩（小窗口优化） | ✅ | ✅ |
| 半透明终端界面 | ✅ | ❌ |
| 技能（Skill）安装与发现 | ✅ | ❌ |

## 开发

```bash
# 运行测试
cargo test

# 代码检查
cargo clippy

# 以详细模式运行
cargo run -- --verbose
```

## 项目结构

```
src/
├── agent/          # 核心引擎（Agent / Subagent / Pipeline / 上下文压缩 / 摘要）
├── llm/            # LLM 客户端（多 provider / 故障转移 / 重试）
├── tools/          # 工具注册表（文件读写 / 搜索 / 符号分析 / 知识库 / 缓存）
├── security/       # 安全策略（路径验证 / 危险操作审批）
├── orchestrator/   # 任务编排（依赖图 / 断点 / 并发控制）
├── scheduler/      # 定时调度（时间轮 / 任务存储 / 执行器）
├── dream/          # 记忆系统（采集 → 巩固 → 去重 → 遗忘 → 报告）
├── hooks/          # Shell hook 机制
├── web/            # axum Web 服务（REST / WebSocket / 模板 / 静态资源）
├── ui/             # 终端 UI（markdown / diff / 主题 / 半透明）
├── session/        # 会话持久化（JSONL）
├── config/         # 配置加载（模型 / 环境变量）
├── skills/         # 技能安装与管理
├── utils/          # 工具函数（错误 / git / 消息输出）
├── app.rs          # 应用入口（build / run / 交互循环）
├── main.rs         # CLI 入口（clap 解析 / 模式切换）
├── repl.rs         # REPL 交互循环
├── prompt.rs       # 系统提示词构建
└── restart.rs      # 重启机制
```

## 致谢

- 架构参考 [grok-build](https://github.com/xai-org/grok-build) 的 `xai-grok-pager` crate

## 许可证

[MIT](LICENSE)