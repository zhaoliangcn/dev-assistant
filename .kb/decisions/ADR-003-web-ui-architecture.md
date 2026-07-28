---
title: Web 界面架构设计方案
type: decision
status: draft
tags: [web-ui, architecture, design, axum, htmx]
---

# Dev-Assistant-RS Web 界面架构设计

## 1. 背景与目标

### 1.1 现状

Dev-Assistant-RS 当前是一个终端（CLI）应用，通过 `ui/` 模块实现 ANSI 终端渲染。核心架构围绕 `Agent` + `LlmClient` + `ToolRegistry` 构建，通过 `repl.rs` 中的交互循环与用户交互。

```
用户输入 → repl.rs:process_user_message → Agent.step() → LLM API → 工具执行 → 结果渲染
```

### 1.2 Web 界面目标

1. **补充而非替代** — CLI 模式完整保留，Web 作为新增前端选项
2. **复用 100% 核心层** — `Agent`、`LlmClient`、`ToolRegistry`、`SecurityPolicy`、`SessionStore` 等核心模块零修改复用
3. **实时流式输出** — LLM 回复和工具执行结果通过 WebSocket/SSE 推送到浏览器
4. **文件管理** — 在浏览器中浏览、编辑项目文件，利用已有的 `file::read`/`file::write` 工具能力
5. **轻量无构建** — 使用 HTMX + Alpine.js，无需 Node.js 构建步骤，所有前端资源嵌入 Rust 二进制

---

## 2. 技术选型

### 2.1 后端

| 组件 | 选择 | 理由 |
|------|------|------|
| HTTP 框架 | **axum 0.8** | 与 tokio 原生集成、支持 WebSocket、提取器体系简洁、社区活跃 |
| HTML 模板 | **minijinja** | Rust 原生 Jinja2 兼容模板，无需外部渲染引擎 |
| 前端资源嵌入 | **rust-embed** | 编译时将 HTML/CSS/JS 打包进二进制，零运行时依赖 |
| WebSocket | **axum 内置** | 原生支持 `ws`，与现有 tokio 运行时无缝集成 |
| 静态资源 | **tower-http** | `ServeDir` 提供开发模式的文件服务 |

### 2.2 前端

| 组件 | 选择 | 理由 |
|------|------|------|
| HTML 超媒体 | **HTMX 2.x** | 通过 HTML 属性完成 AJAX、WebSocket 通信，无需手写 JS |
| 轻量交互 | **Alpine.js 3.x** | 客户端状态管理（折叠、弹窗、表单），无构建步骤 |
| 代码高亮 | **highlight.js** | 通过 CDN 加载，或嵌入精简版 |
| CSS 框架 | **Pico CSS** | 语义化 HTML 样式，极简、轻量、响应式 |

### 2.3 架构决策记录

| 决策 | 选项 | 结论 |
|------|------|------|
| 模板引擎 | Tera / minijinja / maud | **minijinja** — 与 Python 生态兼容，熟悉 Jinja2 语法 |
| 前端框架 | React / Vue / HTMX | **HTMX + Alpine.js** — 零构建、可嵌入、适合以文档为中心的 AI 助手场景 |
| 会话存储 | 内存 / SQLite / 复用现有 JSONL | **复用现有 `persist::SessionStore`** — JSONL 追加写入，与 CLI 模式共享数据 |
| 认证 | 无 / 简单 Token | **无认证** — 本地开发工具，默认绑定 127.0.0.1 |

---

## 3. 整体架构

### 3.1 模块结构

```
src/
├── main.rs                    # CLI 入口（保持不变）
├── app.rs                     # CLI App（保持不变）
├── repl.rs                    # CLI REPL（保持不变）
│
├── web/                       # [新增] Web 模块
│   ├── mod.rs                 # 模块根：路由注册、服务启动
│   ├── router.rs              # axum Router 定义、中间件链
│   ├── handlers/
│   │   ├── mod.rs
│   │   ├── chat.rs            # 对话相关：POST /chat, WS /ws/chat
│   │   ├── files.rs           # 文件浏览：GET /files, GET /files/{path}
│   │   ├── session.rs         # 会话管理：GET /sessions, POST /sessions/:id/resume
│   │   └── status.rs          # 状态/信息：GET /status, GET /models
│   ├── templates/             # minijinja 模板
│   │   ├── base.html          # 布局骨架
│   │   ├── index.html         # 主页面（对话界面）
│   │   ├── chat/
│   │   │   ├── message.html   # 单条消息（用户/助手/工具结果）
│   │   │   ├── thinking.html  # 思考中指示器
│   │   │   └── toolbar.html   # 输入工具栏
│   │   ├── files/
│   │   │   ├── explorer.html  # 文件浏览器侧栏
│   │   │   └── editor.html    # 文件编辑器
│   │   └── session_list.html  # 历史会话列表
│   ├── static/                # 静态资源（rust-embed）
│   │   ├── css/
│   │   │   └── app.css        # 自定义样式 + Pico CSS 覆盖
│   │   └── js/
│   │       └── app.js         # Alpine.js 组件 + HTMX 扩展
│   └── ws/
│       ├── mod.rs             # WebSocket 连接管理
│       ├── session.rs         # 单连接会话状态
│       └── events.rs          # 事件类型定义
│
├── agent/                     # [复用] 核心层不变
├── llm/                       # [复用]
├── tools/                     # [复用]
├── security/                  # [复用]
├── persist/                   # [复用]
├── session/                   # [复用]
└── utils/                     # [复用]
```

### 3.2 依赖新增

```toml
[dependencies]
axum = { version = "0.8", features = ["ws", "macros"] }
minijinja = { version = "2.0", features = ["autoescape"] }
rust-embed = "8.0"
tower-http = { version = "0.6", features = ["fs", "cors"] }
futures = "0.3"               # Stream/WS 工具
tokio-stream = "0.1"          # 流式转换

[build-dependencies]
rust-embed = "8.0"            # 编译时嵌入静态资源
```

### 3.3 数据流

```
┌─────────────────────────────────────────────────────────────────────┐
│                      浏览器 (HTMX + Alpine.js)                       │
│  ┌─────────┐  ┌────────────┐  ┌──────────┐  ┌──────────────────┐  │
│  │ 对话面板 │  │ 文件浏览器 │  │ 模型选择 │  │ 会话历史侧栏    │  │
│  │ HTMX ws  │  │ HTMX GET  │  │ Alpine   │  │ HTMX GET        │  │
│  └────┬─────┘  └─────┬──────┘  └────┬─────┘  └───────┬──────────┘  │
│       │              │              │                │              │
└───────┼──────────────┼──────────────┼────────────────┼──────────────┘
        │              │              │                │
   ┌────▼──────────────▼──────────────▼────────────────▼──────────┐
   │                    axum API 网关 (web/router.rs)              │
   │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐ │
   │  │ WS /chat │  │REST /api │  │REST/file │  │REST/sessions │ │
   │  └────┬─────┘  └────┬─────┘  └────┬─────┘  └──────┬───────┘ │
   └───────┼──────────────┼────────────┼────────────────┼─────────┘
           │              │            │                │
           ▼              │            │                │
   ┌───────────────┐      │            │                │
   │ WebSession    │      │            │                │
   │ (连接管理)    │      │            │                │
   └───────┬───────┘      │            │                │
           │ Agent         │            │                │
           ▼ 实例化        ▼            ▼                ▼
   ┌─────────────────────────────────────────────────────────────┐
   │                   现有核心层 (完全复用)                       │
   │                                                             │
   │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌─────────────┐ │
   │  │  Agent   │  │LlmClient │  │ToolReg.  │  │SessionStore│ │
   │  │ .step()  │→ │ .call()  │  │.execute()│  │ .record()   │ │
   │  └──────────┘  └──────────┘  └──────────┘  └─────────────┘ │
   └─────────────────────────────────────────────────────────────┘
```

---

## 4. 核心接口设计

### 4.1 REST API

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/` | 主页面（SPA 入口） |
| `GET` | `/api/status` | 系统状态（模型、项目目录、运行模式） |
| `GET` | `/api/models` | 可用模型列表 |
| `POST` | `/api/models/switch` | 切换模型 |
| `GET` | `/api/sessions` | 历史会话列表 |
| `GET` | `/api/sessions/:id` | 单条会话详情（消息记录） |
| `DELETE` | `/api/sessions/:id` | 删除会话 |
| `GET` | `/api/files?path=` | 文件/目录列表 |
| `GET` | `/api/files/content?path=` | 文件内容（支持 offset/limit） |
| `POST` | `/api/files/save` | 保存文件内容 |

### 4.2 WebSocket API

WebSocket 连接路径：`GET /ws/chat`

消息格式（JSON，双工流）：

```json
// 浏览器 → 服务端
{
  "type": "user_message",
  "content": "帮我审查一下代码",
  "id": "msg_001"
}

// 服务端 → 浏览器（事件流）
{
  "type": "thinking",
  "content": "正在分析代码库...",
  "id": "evt_001"
}

{
  "type": "tool_call",
  "tool_name": "batch_read_files",
  "args": { "files": ["src/main.rs"] }
}

{
  "type": "tool_result",
  "tool_name": "batch_read_files",
  "success": true,
  "content": "✅ 读取完成: 50/50 文件成功",
  "preview": "..."
}

{
  "type": "assistant_message",
  "content": "代码审查结果如下：\n1. ...",
  "streaming": false
}

{
  "type": "error",
  "content": "LLM API 错误: connection timeout"
}

{
  "type": "done",
  "message_id": "msg_001"
}

{
  "type": "status",
  "content": "等待 LLM 响应..."
}
```

### 4.3 核心数据结构（复用）

```rust
// 复用现有的 Agent 和上下文，Web 会话只需包装 Agent 实例
pub struct WebSession {
    pub id: String,
    pub agent: Agent,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub message_count: usize,
}

// 复用现有的 SessionStore 和 SessionEvent 做持久化
// 复用现有的 MessageLevel / MessageOutput trait
```

---

## 5. 分阶段实施计划

### Phase 1: 基础 Web 服务 + 对话界面 (预计 5-7 天)

**目标**: 启动 axum Web 服务，完成基础对话界面

| 步骤 | 文件 | 说明 |
|------|------|------|
| 1.1 | `Cargo.toml` | 添加 axum、minijinja、rust-embed 等依赖 |
| 1.2 | `src/web/mod.rs` | Web 模块根，`serve()` 函数启动 axum 服务 |
| 1.3 | `src/web/router.rs` | 路由+中间件（日志、错误处理、静态文件） |
| 1.4 | `src/web/ws/mod.rs` | WebSocket 连接管理器（连接池，消息路由） |
| 1.5 | `src/web/ws/events.rs` | 事件类型枚举（输入/输出） |
| 1.6 | `src/web/ws/session.rs` | `WebSession` 结构体，包装 `Agent` 实例 |
| 1.7 | `src/web/handlers/chat.rs` | WebSocket 处理：接收用户消息 → 调用 Agent → 发送事件 |
| 1.8 | `src/web/templates/` | base.html + index.html + chat/message.html |
| 1.9 | `src/web/static/` | app.css + app.js（HTMX ws 扩展） |
| 1.10 | `main.rs` | 新增 `--web` 标志，启动 Web 模式 |

**Phase 1 完成后的效果**: 用户在浏览器打开 `http://localhost:8080`，进入对话界面，发送消息后看到流式输出的助手回复和工具结果。

### Phase 2: 文件管理 + 代码预览 (预计 3-4 天)

**目标**: 在 Web 界面中浏览和编辑项目文件

| 步骤 | 文件 | 说明 |
|------|------|------|
| 2.1 | `src/web/handlers/files.rs` | 目录列表、文件读取、文件保存 API |
| 2.2 | `src/web/templates/files/explorer.html` | 文件树侧栏（HTMX 懒加载） |
| 2.3 | `src/web/templates/files/editor.html` | 文件编辑器（Alpine.js 管理状态） |
| 2.4 | `src/web/static/js/app.js` | 代码高亮集成（highlight.js） |

### Phase 3: 会话管理 + 历史记录 (预计 2-3 天)

**目标**: 支持查看、恢复、删除历史会话

| 步骤 | 文件 | 说明 |
|------|------|------|
| 3.1 | `src/web/handlers/session.rs` | 会话 CRUD API （复用 `persist::SessionStore`） |
| 3.2 | `src/web/templates/session_list.html` | 会话历史侧栏（HTMX 分页） |
| 3.3 | 集成 | 从会话恢复 Agent 状态（`App::load_state_or_fresh`） |

### Phase 4: 增强功能 (预计 3-5 天)

**目标**: 流水线管理、后台任务监控

| 步骤 | 说明 |
|------|------|
| 4.1 | 流水线执行界面（进度条、阶段展示） |
| 4.2 | 后台任务监控仪表盘 |
| 4.3 | 设置页面（模型配置、安全策略） |
| 4.4 | 导出/导入对话记录 |

---

## 6. 关键设计决策

### 6.1 Agent 实例生命周期

每个 Web 会话持有独立的 `Agent` 实例（含 `ContextManager`）。Agent 是 `!Send` 的（因为它持有 `Arc<LlmClient>` 等共享资源），所以每个 WebSocket 连接需要在独立的任务中运行。

```rust
// WebSocket 连接时创建新的 Agent
let agent = Agent::new(AgentConfig {
    llm: self.llm.clone(),          // Arc<LlmClient> — 共享
    tools: self.tools.clone(),       // Arc<ToolRegistry> — 共享
    session_store: Some(store),      // 每个会话独立
    ..config
})?;

// 每条消息在独立 task 中执行，通过 mpsc channel 发送结果
let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<WsEvent>();
tokio::spawn(async move {
    process_message(agent, input, tx).await;
});
```

### 6.2 UI 渲染策略

不同于 CLI 模式的块级 ANSI 渲染，Web 模式使用服务器端渲染的 HTML 片段（HTMX）：

| CLI 概念 | Web 对应 |
|----------|----------|
| `MessageBlock::User` | `chat/message.html` 模板 + `user` class |
| `MessageBlock::Assistant` | `chat/message.html` + `assistant` class + Markdown 渲染 |
| `MessageBlock::Thinking` | `chat/thinking.html` 旋转动画 + 文字 |
| `MessageBlock::ToolResult` | `chat/message.html` + `tool-result` class + 可折叠 |
| `MessageBlock::Diff` | 代码 diff 高亮（green/red 行） |
| `render_input_panel()` | 固定底部输入框（Alpine.js 管理） |
| 进度条 | 进度条 HTML 元素（HTMX 替换） |

HTMX 的 `hx-swap-oob` 机制用于在 WebSocket 消息到达时原地更新页面元素。

### 6.3 Markdown 渲染

CLI 模式使用 `pulldown-cmark` + `syntect` 在服务端渲染为 ANSI。Web 模式转而在**服务端**使用 `pulldown-cmark` 将 Markdown 渲染为 HTML，代码块使用 `syntect` 生成 `<pre><code class="language-xxx">` 加类名的 HTML，**客户端**由 highlight.js 完成高亮。

```
服务端: pulldown-cmark → HTML + 代码块 <pre><code> → 客户端: highlight.js 着色
```

### 6.4 与 CLI 模式的共存

```
┌──────────────────────────────────────────┐
│               main.rs                     │
│                                           │
│  if cli.web {                             │
│      web::serve(config).await             │
│  } else {                                 │
│      App::build(config).run().await       │
│  }                                        │
└──────────────────────────────────────────┘
```

两侧共享:
- `.dev-assistant-store/` — 会话持久化目录
- `.dev-assistant-state.json` — 状态文件
- `config.toml` — 配置（可选）

---

## 7. 安全考虑

| 风险 | 缓解措施 |
|------|----------|
| Web 服务暴露到外网 | 默认绑定 `127.0.0.1`，通过 `--host` 可选暴露 |
| XSS（用户消息含 HTML） | minijinja 自动转义 + 服务端 Markdown 渲染过滤危险标签 |
| 文件系统越权 | 复用 `SecurityPolicy::validate_path` 限制路径 |
| WebSocket 未授权 | 本地绑定 + 无认证（开发工具假设） |
| CSRF | 同源策略（无跨域 API）+ 可选 Origin 检查 |

---

## 8. 与 CLI 模式的功能对比

| 功能 | CLI | Web Phase 1 | Phase 2 | Phase 3 | Phase 4 |
|------|-----|------------|---------|---------|---------|
| 对话/消息 | ✅ | ✅ | ✅ | ✅ | ✅ |
| Markdown 渲染 | ✅ | ✅ | ✅ | ✅ | ✅ |
| 代码高亮 | ✅ | ✅ | ✅ | ✅ | ✅ |
| 工具调用展示 | ✅ | ✅ | ✅ | ✅ | ✅ |
| 文件浏览 | ❌ | ❌ | ✅ | ✅ | ✅ |
| 文件编辑 | ❌ | ❌ | ✅ | ✅ | ✅ |
| 会话历史 | ✅ 日志文件 | ❌ | ❌ | ✅ | ✅ |
| 会话恢复 | ✅ | ❌ | ❌ | ✅ | ✅ |
| 流水线管理 | ✅ | ❌ | ❌ | ❌ | ✅ |
| 后台任务 | ✅ | ❌ | ❌ | ❌ | ✅ |
| 行编辑/历史 | ✅ | ✅ | ✅ | ✅ | ✅ |
| 进度条 | ✅ | ❌ | ❌ | ❌ | ✅ |

---

## 9. 测试策略

| 层级 | 工具 | 覆盖范围 |
|------|------|----------|
| 单元测试 | `#[cfg(test)]` | `web::ws::events`、`web::handlers::chat` 逻辑 |
| 集成测试 | `axum::test` | REST API 端点、WebSocket 握手 |
| E2E 测试 | Playwright (可选) | 浏览器自动化——Phase 4 追加 |

```
// 集成测试示例
#[tokio::test]
async fn test_websocket_chat() {
    let app = test_app().await;
    let mut ws = app.connect_ws("/ws/chat").await;
    ws.send_text(r#"{"type":"user_message","content":"hello"}"#).await;
    let msg = ws.recv_text().await;
    assert!(msg.contains("assistant_message"));
}
```

---

## 10. 附录

### 10.1 HTML 模板示例（Phase 1）

**`chat/message.html`** — 单条消息模板（HTMX WebSocket 交换）:

```html
<div id="message-{{ msg.id }}" class="message {{ msg.role }}{% if msg.streaming %} streaming{% endif %}">
  <div class="message-avatar">
    {% if msg.role == "user" %} 👤 {% else %} 🤖 {% endif %}
  </div>
  <div class="message-body">
    <div class="message-header">
      <span class="message-role">{{ msg.role_label }}</span>
      <span class="message-time">{{ msg.timestamp }}</span>
    </div>
    <div class="message-content markdown-body">
      {{ msg.html_content|safe }}
    </div>
    {% if msg.collapsible %}
    <button class="collapsible-trigger"
            x-data="{ collapsed: true }"
            x-on:click="collapsed = !collapsed">
      <span x-show="collapsed">展开详情 ▸</span>
      <span x-show="!collapsed">收起详情 ▾</span>
    </button>
    {% endif %}
  </div>
</div>
```

### 10.2 WebSocket 事件处理流程

```
浏览器                         axum                         Agent
  │                              │                              │
  │── WS connect /ws/chat ──────>│                              │
  │                              │── 创建 WebSession ──────────>│
  │<─── {type:"status",...} ─────│                              │
  │                              │                              │
  │── {type:"user_message"} ────>│                              │
  │                              │── Agent.step() ────────────>│
  │                              │   loop {                     │
  │<── {type:"thinking",...} ────│<── output.info() ───────────│
  │<── {type:"tool_call",...} ───│<── output.info() ───────────│
  │<── {type:"tool_result",...} ─│<── output.success() ────────│
  │                              │   }                          │
  │<── {type:"assistant_msg"} ───│<── AgentStep::Done ─────────│
  │<── {type:"done"} ───────────│                              │
```

### 10.3 启动方式

```bash
# CLI 模式（不变）
dev-assistant --project ./my-project

# Web 模式（新增）
dev-assistant --web --project ./my-project
# → 打开 http://localhost:8080

# 指定端口和 host
dev-assistant --web --port 9090 --host 0.0.0.0
```
