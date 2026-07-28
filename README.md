# Dev-Assistant-RS

<div align="center">

**Rust 原生 AI 编程助手 — 终端中的 AI 结对编程伙伴**

[![Rust](https://img.shields.io/badge/Rust-1.85%2B-orange)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue)](LICENSE)
[![CI](https://github.com/your-username/dev-assistant-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/your-username/dev-assistant-rs/actions/workflows/ci.yml)

</div>

---

## 简介

**Dev-Assistant-RS** 是一个用 Rust 编写的 AI 编程助手，专注于 **代码库级别的AI辅助**。与聊天式 AI 不同，它能：

- 🔍 **阅读并理解整个代码库** — 批量分析项目文件
- 🔧 **自动修改代码** — 编辑文件、重构代码、编写测试
- 🔄 **流水线执行** — 架构设计 → 代码实现 → 审查 → 修复
- 🛡 **安全可控** — 危险操作需审批，路径遍历防护

## 快速开始

### 前置条件

- Rust 1.85+
- 一个 LLM API Key（OpenAI / DeepSeek / Anthropic / 商汤 等）

### 安装

```bash
git clone https://github.com/your-username/dev-assistant-rs.git
cd dev-assistant-rs
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
cp .dev-assistant-models_example.toml .dev-assistant-models.toml
# 编辑 .dev-assistant-models.toml，配置多个模型的 API Key
```

### 运行

```bash
# 交互模式（REPL）
cargo run --release

# 单次执行模式
cargo run --release -- --message "帮我审查一下代码"

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

### 内建命令

```
/help      — 显示帮助
/history   — 显示对话历史
/clear     — 清屏
/expand    — 展开折叠的内容
/grep      — 搜索文件内容（支持正则）
/model     — 查看/切换 LLM 模型
/pipeline  — 执行流水线任务
/verbose   — 切换到详细模式
/quiet     — 切换到安静模式
```

## 架构

```
用户输入 → repl.rs → Agent.step() → LLM API → 工具执行 → 结果渲染
```

- **Agent** — 核心引擎，管理对话上下文和工具调度
- **LlmClient** — LLM API 客户端，支持多 provider、自动故障转移、429/5xx 重试
- **ToolRegistry** — 工具注册中心（文件操作、搜索、代码分析等）
- **SecurityPolicy** — 安全策略层（危险命令拦截、路径验证）
- **SessionStore** — 会话持久化（JSONL 格式，append-only）

## 功能特性

| 特性 | CLI | Web (规划中) |
|------|-----|-------------|
| 对话式 AI 辅助 | ✅ | Phase 1 |
| Markdown 渲染 + 代码高亮 | ✅ | Phase 1 |
| Diff 渲染（绿/红对比） | ✅ | Phase 1 |
| 工具调用智能摘要 | ✅ | Phase 1 |
| 文件浏览/编辑 | ❌ | Phase 2 |
| 会话历史管理 | ✅ | Phase 3 |
| 流水线管理 | ✅ | Phase 4 |
| Provider 故障转移 | ✅ | ✅ |
| 429/5xx 自动重试 | ✅ | ✅ |

## 开发

```bash
# 运行测试
cargo test

# 代码检查
cargo clippy

# 以详细模式运行
cargo run -- --verbose
```

## 许可证

[MIT](LICENSE)

## 致谢

- 架构参考 [grok-build](https://github.com/xai-org/grok-build) 的 `xai-grok-pager` crate
