# Vue.js 3 Web 前端升级设计方案

## 一、目标与原则

### 1.1 目标

在**保留现有内嵌前端不变**的前提下，新增一个独立的 Node.js 服务，提供基于 Vue.js 3 的现代化 Web 界面。

### 1.2 核心原则

| 原则 | 说明 |
|------|------|
| **零侵入** | 现有 Rust 后端代码、内嵌前端（Alpine.js + HTMX）**一行不改** |
| **独立部署** | Node.js 服务是独立进程，可单独启动/停止，不依赖 Rust 编译 |
| **协议复用** | 完全复用现有 REST API + WebSocket 协议，Rust 后端感知不到前端更换 |
| **渐进增强** | 先实现核心聊天功能，再逐步补全会话管理、文件树、模型配置等 |

### 1.3 运行模式

```
开发模式:
  npm run dev  →  Vite dev server (:5173)  ──proxy──►  Rust backend (:8080)
                                                        │
                                                        ├── /api/*       (REST)
                                                        └── /ws/chat     (WebSocket)

生产模式:
  npm run build  →  Vue 构建产物 → dist/
  npm start      →  Node.js server (:3000)  ──proxy──►  Rust backend (:8080)
                    │                                    │
                    ├── /              静态文件 (dist/)   │
                    ├── /api/*         代理 →             │
                    └── /ws/chat       代理 →             │
```

---

## 二、项目结构

```
dev-assistant/                      # 仓库根目录
├── src/                            # Rust 源码（不改动）
│   └── web/                        # 内嵌前端（保留不变）
├── web-vue/                        # ★ 新增：Vue.js 3 独立前端项目
│   ├── package.json
│   ├── tsconfig.json
│   ├── tsconfig.node.json
│   ├── vite.config.ts
│   ├── index.html
│   ├── server.js                   # 生产模式 Node.js 静态服务 + 代理
│   ├── public/
│   │   └── favicon.svg
│   ├── src/
│   │   ├── main.ts                 # 入口
│   │   ├── App.vue                 # 根组件
│   │   ├── router/
│   │   │   └── index.ts            # Vue Router 配置
│   │   ├── stores/                 # Pinia 状态管理
│   │   │   ├── chat.ts             # 对话消息 + WebSocket 集成
│   │   │   ├── sessions.ts         # 会话列表
│   │   │   ├── models.ts           # 模型配置
│   │   │   ├── files.ts            # 文件树
│   │   │   └── app.ts              # 全局状态（主题、连接、国际化）
│   │   ├── api/                    # HTTP + WebSocket 封装
│   │   │   ├── client.ts           # ofetch 实例（base URL、错误处理）
│   │   │   ├── sessions.ts         # 会话 API
│   │   │   ├── models.ts           # 模型 API
│   │   │   ├── files.ts            # 文件 API
│   │   │   └── ws.ts               # WebSocket 客户端（含自动重连）
│   │   ├── composables/            # 组合式函数
│   │   │   ├── useWebSocket.ts     # WebSocket 连接生命周期
│   │   │   ├── useStreaming.ts     # 流式增量缓冲 + 节流渲染
│   │   │   ├── useMarkdown.ts      # Markdown 渲染（marked + highlight.js）
│   │   │   └── useTheme.ts         # 暗色/亮色主题
│   │   ├── views/                  # 页面级组件
│   │   │   ├── ChatView.vue        # / — 对话主页面
│   │   │   └── FilesView.vue       # /files — 文件浏览页面
│   │   ├── components/             # 可复用组件
│   │   │   ├── chat/
│   │   │   │   ├── ChatMessage.vue       # 单条消息气泡
│   │   │   │   ├── ChatMessageList.vue   # 消息列表（滚动容器）
│   │   │   │   ├── ChatInput.vue         # 输入框 + 发送/停止按钮
│   │   │   │   ├── ToolActivity.vue      # 工具活动面板
│   │   │   │   └── ReasoningPanel.vue    # 思考过程展示
│   │   │   ├── sidebar/
│   │   │   │   ├── AppSidebar.vue        # 侧栏容器（Tab 切换）
│   │   │   │   ├── SessionList.vue       # 会话历史列表
│   │   │   │   └── FileTree.vue          # 文件树浏览器
│   │   │   ├── layout/
│   │   │   │   ├── AppHeader.vue         # 顶部导航栏
│   │   │   │   └── AppLayout.vue         # 主布局（header + sidebar + main）
│   │   │   └── shared/
│   │   │       ├── MarkdownRenderer.vue  # Markdown → HTML 渲染
│   │   │       ├── CodeBlock.vue         # 代码块（语法高亮 + 复制）
│   │   │       ├── DiffView.vue          # Diff 对比视图
│   │   │       └── StatusBadge.vue       # 连接状态指示器
│   │   ├── types/                  # TypeScript 类型定义
│   │   │   ├── api.ts              # API 请求/响应类型
│   │   │   ├── ws.ts               # WebSocket 事件类型
│   │   │   └── chat.ts             # 消息、会话类型
│   │   └── assets/
│   │       └── main.css            # 全局样式 + CSS 变量
│   └── dist/                       # 构建产物（gitignore）
└── .gitignore                      # 追加 web-vue/node_modules/、web-vue/dist/
```

---

## 三、技术选型

| 类别 | 选型 | 版本 | 理由 |
|------|------|------|------|
| **框架** | Vue | 3.5+ | Composition API + `<script setup lang="ts">` |
| **语言** | TypeScript | 5.6+ | 类型安全 |
| **构建** | Vite | 6.x | 极速 HMR，Vue 生态首选 |
| **状态管理** | Pinia | 2.x | Vue 官方推荐，TS 友好 |
| **路由** | Vue Router | 4.x | SPA 路由 |
| **HTTP 客户端** | ofetch | 1.x | 轻量，自动 JSON，基于 fetch |
| **Markdown** | marked | 15.x | 成熟稳定，可扩展 |
| **代码高亮** | highlight.js | 11.x | 与现有内嵌前端一致 |
| **CSS** | scoped CSS + CSS variables | — | 无需额外 CSS 框架，保持轻量 |
| **生产服务器** | Express | 4.x | 静态文件 + http-proxy-middleware |
| **开发代理** | Vite proxy | 内置 | 无需额外依赖 |
| **工具库** | @vueuse/core | 11.x | useDark, useToggle, useLocalStorage 等 |

---

## 四、组件树详解

```
App.vue
└── <RouterView>
    └── AppLayout.vue
        ├── AppHeader.vue
        │   ├── StatusBadge.vue          # 在线/离线指示
        │   ├── 模型选择器               # 下拉切换 LLM 模型
        │   ├── TokenUsage.vue           # 累计 Token 消耗
        │   └── 主题切换按钮             # 暗色/亮色
        ├── AppSidebar.vue
        │   ├── [Tab: 会话]
        │   │   └── SessionList.vue
        │   │       ├── 新建会话按钮
        │   │       └── SessionItem.vue (v-for)
        │   │           ├── 标题 / 时间 / 消息数
        │   │           ├── 重命名按钮
        │   │           └── 删除按钮
        │   └── [Tab: 文件]
        │       └── FileTree.vue
        │           ├── 面包屑导航
        │           └── FileEntry.vue (v-for)
        └── <RouterView>
            ├── ChatView.vue             # 路由 /
            │   ├── MessageSearch.vue    # Ctrl+F 搜索面板（可折叠）
            │   ├── ReasoningPanel.vue   # 思考流展示（可折叠）
            │   ├── ChatMessageList.vue  # 消息滚动区
            │   │   └── ChatMessage.vue (v-for)
            │   │       ├── 用户消息气泡
            │   │       ├── 助手消息气泡
            │   │       │   └── MarkdownRenderer.vue
            │   │       │       └── CodeBlock.vue (v-for)
            │   │       ├── 系统消息
            │   │       └── 错误消息
            │   ├── ToolActivity.vue     # 工具调用侧栏（可折叠）
            │   └── ChatInput.vue        # 底部输入区
            │       ├── textarea（自动增高）
            │       ├── 发送按钮 / 停止按钮
            │       └── 拖拽上传区域
            └── FilesView.vue            # 路由 /files
                └── 文件内容查看 + 编辑
```

---

## 五、数据流与状态管理

### 5.1 Pinia Store 职责划分

```
┌──────────────────────────────────────────────────────────────┐
│  useChatStore                                                │
│  ┌──────────────────────────────────────────────────────────┐│
│  │ state:                                                   ││
│  │   messages: ChatMessage[]    // 对话消息列表              ││
│  │   toolMessages: ToolMessage[]// 工具活动消息              ││
│  │   sessionId: string | null   // 当前 WS 会话 ID          ││
│  │   busy: boolean              // 是否正在生成              ││
│  │   reasoningText: string      // 思考流文本               ││
│  │   reasoningActive: boolean   // 思考流是否活跃            ││
│  │   tokenUsage: TokenUsage     // 本轮 Token 消耗           ││
│  │                                                          ││
│  │ actions:                                                 ││
│  │   sendMessage(content)       // 发送用户消息              ││
│  │   stopGeneration()           // 取消生成                  ││
│  │   addMessage(msg)            // 追加消息（含流式）        ││
│  │   loadSession(id)            // 加载历史会话              ││
│  │   newChat()                  // 新建对话                  ││
│  │   handleServerEvent(ev)      // 分发 WS 事件              ││
│  └──────────────────────────────────────────────────────────┘│
├──────────────────────────────────────────────────────────────┤
│  useSessionsStore                                            │
│  │ state: sessions[], activeId, loading                      │
│  │ actions: load(), remove(id), rename(id, title)            │
├──────────────────────────────────────────────────────────────┤
│  useModelsStore                                              │
│  │ state: models[], activeModel, configPath                  │
│  │ actions: load(), switch(name), save(cfg), remove(name)    │
├──────────────────────────────────────────────────────────────┤
│  useFilesStore                                               │
│  │ state: currentPath, entries[], loading                    │
│  │ actions: listDir(path), getContent(path), saveFile(...)   │
├──────────────────────────────────────────────────────────────┤
│  useAppStore                                                 │
│  │ state: connected, theme, sidebarOpen, locale              │
│  │ actions: toggleTheme(), toggleSidebar(), setLocale()      │
└──────────────────────────────────────────────────────────────┘
```

### 5.2 WebSocket 集成架构

```typescript
// api/ws.ts — WebSocket 客户端
class ChatWebSocket {
  private ws: WebSocket | null = null
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private reconnectAttempts = 0
  private maxReconnectAttempts = 10

  // 连接建立 → 自动注册 onmessage → 分发到 chatStore.handleServerEvent()
  connect(projectDir?: string): void
  disconnect(): void
  send(msg: ClientMessage): void
  
  // 指数退避重连：1s → 2s → 4s → ... → 30s max
  private scheduleReconnect(): void
}
```

### 5.3 流式渲染流程

```
Rust Backend                    WebSocket                 Vue Frontend
     │                             │                          │
     │  assistant_stream_delta     │                          │
     │  { delta: "Hel",            │                          │
     │    is_final: false }  ────► │  onmessage ──────────►  │
     │                             │          │               │
     │                             │          ▼               │
     │                             │   chatStore.handleServerEvent()
     │                             │          │               │
     │                             │          ▼               │
     │                             │   _streamBuf += "Hel"    │
     │                             │   启动 50ms 节流定时器    │
     │                             │                          │
     │  assistant_stream_delta     │                          │
     │  { delta: "lo",             │                          │
     │    is_final: false }  ────► │  _streamBuf += "lo"      │
     │                             │                          │
     │  assistant_stream_delta     │                          │
     │  { delta: " World",         │                          │
     │    is_final: true }   ────► │  50ms 到期               │
     │                             │  flush: 消息 content     │
     │                             │  += "Hello World"        │
     │                             │  streaming = false       │
     │                             │  → 触发 Markdown 重渲染   │
```

---

## 六、Node.js 服务设计（server.js）

### 6.1 职责

生产模式下作为轻量级静态文件服务 + API/WebSocket 反向代理：

```javascript
// server.js — 生产模式入口
const express = require('express')
const { createProxyMiddleware } = require('http-proxy-middleware')

const RUST_BACKEND = process.env.RUST_BACKEND || 'http://127.0.0.1:8080'
const PORT = process.env.PORT || 3000

const app = express()

// 1. 静态文件：Vue 构建产物
app.use(express.static('dist'))

// 2. REST API 代理 → Rust 后端
app.use('/api', createProxyMiddleware({ target: RUST_BACKEND, changeOrigin: true }))

// 3. WebSocket 代理 → Rust 后端
//    http-proxy-middleware 自动处理 Upgrade 头

// 4. SPA fallback：所有非 /api 非 /ws 路径 → index.html
app.get('*', (req, res) => {
  res.sendFile('index.html', { root: 'dist' })
})

app.listen(PORT, () => {
  console.log(`Vue Web UI: http://127.0.0.1:${PORT}`)
  console.log(`Backend: ${RUST_BACKEND}`)
})
```

### 6.2 开发模式代理配置（vite.config.ts）

```typescript
// vite.config.ts
export default defineConfig({
  plugins: [vue()],
  server: {
    port: 5173,
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8080',
        changeOrigin: true,
      },
      '/ws': {
        target: 'ws://127.0.0.1:8080',
        ws: true,
      },
    },
  },
})
```

---

## 七、与现有内嵌前端的对比

| 维度 | 现有内嵌前端 | 新 Vue.js 前端 |
|------|-------------|---------------|
| **运行方式** | 嵌入 Rust 二进制 | Node.js 独立进程 |
| **端口** | :8080（与后端同端口） | :5173（开发）/ :3000（生产） |
| **框架** | Alpine.js + Pico.css | Vue 3 + scoped CSS |
| **类型安全** | 无（纯 JS） | TypeScript |
| **状态管理** | Alpine Store | Pinia（DevTools 可调试） |
| **路由** | 服务端模板渲染 | Vue Router（客户端 SPA） |
| **HMR** | 需重新编译 Rust | Vite 毫秒级热更新 |
| **组件化** | x-data 函数 | .vue 单文件组件 |
| **调试体验** | 浏览器 console | Vue DevTools + TS 类型提示 |
| **包管理** | 无（CDN） | npm + node_modules |
| **代码高亮** | highlight.js CDN | highlight.js npm 包 |
| **Markdown** | 自研 JS 解析器 | marked 库 |

---

## 八、Rust 后端需要的适配

### 8.1 现有代码：零改动

Rust 后端的 `src/web/` 模块**完全不动**，内嵌前端继续正常工作在 `:8080`。

### 8.2 可选增强（后续优化）

| 改动 | 优先级 | 说明 |
|------|--------|------|
| CORS 响应头 | 低 | Vite proxy 已解决跨域，生产模式同源不需要 CORS |
| `/health` 端点 | 低 | Node.js 服务启动时检测 Rust 后端是否就绪 |
| API 文档（OpenAPI） | 低 | 自动生成 API 文档便于前端对接 |

---

## 九、实施计划

### Phase 1：项目脚手架 + 类型定义 + API 层

| 任务 | 文件 | 说明 |
|------|------|------|
| 初始化项目 | `package.json`, `vite.config.ts`, `tsconfig.json` | Vite + Vue 3 + TS + Pinia + Router |
| HTML 入口 | `index.html` | SPA 挂载点 |
| 类型定义 | `src/types/api.ts`, `ws.ts`, `chat.ts` | 对齐 Rust 后端数据结构 |
| API 封装 | `src/api/client.ts`, `sessions.ts`, `models.ts`, `files.ts` | ofetch 实例 + 各模块 API 函数 |
| WebSocket 客户端 | `src/api/ws.ts` | 连接/重连/收发 |
| App 入口 | `src/main.ts`, `App.vue` | 挂载 Pinia + Router |
| 路由 | `src/router/index.ts` | `/` → ChatView, `/files` → FilesView |

**里程碑**：`npm run dev` 可启动，API 调用可连通 Rust 后端

### Phase 2：核心聊天功能

| 任务 | 文件 | 说明 |
|------|------|------|
| chatStore | `src/stores/chat.ts` | 消息管理 + WS 事件分发 |
| useWebSocket | `src/composables/useWebSocket.ts` | 连接生命周期 |
| useStreaming | `src/composables/useStreaming.ts` | 流式缓冲 + 节流 |
| ChatMessage | `src/components/chat/ChatMessage.vue` | 消息气泡（user/assistant/system/error） |
| ChatMessageList | `src/components/chat/ChatMessageList.vue` | 滚动容器 + 自动滚底 |
| ChatInput | `src/components/chat/ChatInput.vue` | 输入 + 发送/停止 |
| ChatView | `src/views/ChatView.vue` | 组装聊天页面 |

**里程碑**：可发送消息、接收流式回复、取消生成

### Phase 3：Markdown 渲染 + 代码高亮

| 任务 | 文件 | 说明 |
|------|------|------|
| useMarkdown | `src/composables/useMarkdown.ts` | marked 配置 + highlight.js 集成 |
| MarkdownRenderer | `src/components/shared/MarkdownRenderer.vue` | 渲染 Markdown → HTML |
| CodeBlock | `src/components/shared/CodeBlock.vue` | 代码块 + 语言标签 + 复制按钮 |
| DiffView | `src/components/shared/DiffView.vue` | diff 对比 |

**里程碑**：助手回复正确渲染 Markdown、代码块有语法高亮

### Phase 4：侧栏（会话管理 + 文件树）

| 任务 | 文件 | 说明 |
|------|------|------|
| sessionsStore | `src/stores/sessions.ts` | 会话 CRUD |
| filesStore | `src/stores/files.ts` | 文件树浏览 |
| AppSidebar | `src/components/sidebar/AppSidebar.vue` | Tab 切换容器 |
| SessionList | `src/components/sidebar/SessionList.vue` | 会话列表 + 重命名 + 删除 |
| FileTree | `src/components/sidebar/FileTree.vue` | 文件树 + 导航 |

**里程碑**：侧栏可切换会话、浏览文件树

### Phase 5：布局 + 模型管理 + 主题

| 任务 | 文件 | 说明 |
|------|------|------|
| AppLayout | `src/components/layout/AppLayout.vue` | 响应式布局 |
| AppHeader | `src/components/layout/AppHeader.vue` | 状态栏 + 模型选择 |
| modelsStore | `src/stores/models.ts` | 模型列表 + 切换 |
| appStore + useTheme | `src/stores/app.ts`, `src/composables/useTheme.ts` | 主题/连接/国际化 |
| ToolActivity | `src/components/chat/ToolActivity.vue` | 工具调用面板 |
| ReasoningPanel | `src/components/chat/ReasoningPanel.vue` | 思考流展示 |
| StatusBadge | `src/components/shared/StatusBadge.vue` | 连接状态 |
| TokenUsage | `src/components/shared/TokenUsage.vue` | Token 计数 |

**里程碑**：界面完整，含暗色模式

### Phase 6：生产部署 + 服务脚本

| 任务 | 文件 | 说明 |
|------|------|------|
| 生产服务器 | `server.js` | Express 静态 + 代理 |
| npm scripts | `package.json` | `dev` / `build` / `start` |
| 全局样式 | `src/assets/main.css` | CSS 变量 + 响应式 |

**里程碑**：`npm run build && npm start` 可生产运行

---

## 十、风险与缓解

| 风险 | 缓解措施 |
|------|----------|
| WebSocket 代理断连 | `ws.ts` 内置指数退避自动重连（最多 10 次） |
| Rust 后端未启动时 Node.js 启动 | `server.js` 启动时检测后端健康状态，提示用户先启动 Rust 服务 |
| 大会话消息渲染性能 | 消息列表使用 `v-memo` 优化 + 虚拟滚动（后续） |
| 与内嵌前端端口冲突 | Vue 服务使用独立端口（5173/3000），不与 8080 冲突 |
| TypeScript 类型与 Rust 结构体不同步 | 手动维护 `src/types/`，后续可考虑从 Rust 结构体自动生成 TS 类型 |

---

## 十一、npm scripts

```jsonc
{
  "scripts": {
    "dev": "vite",                           // 开发服务器 :5173 + HMR
    "build": "vue-tsc -b && vite build",     // 类型检查 + 构建
    "preview": "vite preview",               // 预览构建产物
    "start": "node server.js",               // 生产模式 :3000
    "typecheck": "vue-tsc --noEmit"          // 仅类型检查
  }
}
```

---

## 十二、package.json 核心依赖

```jsonc
{
  "dependencies": {
    "vue": "^3.5",
    "vue-router": "^4.4",
    "pinia": "^2.2",
    "marked": "^15.0",
    "highlight.js": "^11.10",
    "ofetch": "^1.4",
    "@vueuse/core": "^11.0"
  },
  "devDependencies": {
    "@vitejs/plugin-vue": "^5.1",
    "typescript": "~5.6",
    "vite": "^6.0",
    "vue-tsc": "^2.1",
    "@types/node": "^22.0",
    "express": "^4.21",
    "http-proxy-middleware": "^3.0"
  }
}
```