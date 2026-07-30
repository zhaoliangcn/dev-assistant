## UI 优化计划

### 核心问题

尽管所有 Phase 1-4 的功能已实现，但最大的 UX 问题仍未解决：**LLM 响应时没有实时流式输出**。用户发送消息后，需要等待 10-30 秒才能看到任何反馈。`is_streaming` 字段存在但从未使用（`#[allow(dead_code)]`）。消息仍在 `UIMessageOutput` 中缓冲，然后批量渲染。

---

### Phase 1: LLM 响应流式输出 (P0)

**目标**: 让用户实时看到 LLM 生成的文本，消除等待空白期

**改动文件** (8 个文件):

| 文件 | 改动 |
|------|------|
| `src/llm/provider/mod.rs` | 新增 `chat_stream()` 方法到 `LlmProvider` trait |
| `src/llm/models.rs` | 新增 `LlmResponse::StreamingChunk` 变体，包含增量文本 |
| `src/llm/client.rs` | 新增 `call_streaming()` 方法，返回流式响应 |
| `src/llm/provider/ollama.rs` | 实现流式：`"stream": true`，解析 NDJSON 行 |
| `src/llm/provider/openai.rs` | 实现流式：SSE 解析，`stream_options: {include_usage: true}` |
| `src/llm/provider/anthropic.rs` | 实现流式：SSE 解析，`content_block_delta` 事件 |
| `src/agent/mod.rs` | 修改 `step()` 支持流式：循环读取流式块，实时更新 `MessageBlock` |
| `src/ui/blocks.rs` | 激活 `is_streaming` 字段，渲染时显示闪烁光标/`...` 指示器 |
| `src/repl.rs` | 主循环处理流式 `MessageBlock::Assistant { is_streaming: true }` |

**核心设计思路:**

```rust
// LlmProvider trait 新增方法
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat(&self, http_client: &Client, request: &LlmRequest) -> Result<LlmResponse, AppError>;
    async fn chat_stream(
        &self,
        http_client: &Client,
        request: &LlmRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmStreamEvent, AppError>> + Send>>, AppError>;
}

// 流式事件
pub enum LlmStreamEvent {
    Chunk(String),      // 增量文本
    ToolCallDelta(ToolCall),  // 工具调用（首批 chunk 就包含完整 tool_calls）
    Done,               // 完成
}

// LlmClient 新增方法
pub async fn call_streaming(
    &self,
    messages: Vec<LlmMessage>,
    tools: Vec<ToolSchema>,
) -> Result<Pin<Box<dyn Stream<Item = Result<LlmStreamEvent, AppError>> + Send>>, AppError>
```

**agent/mod.rs 改动:**
```rust
// 当前: 等待完整响应后渲染
let response = self.llm.call(messages, tool_schemas).await?;
// 改为: 使用流式，实时更新
let mut stream = self.llm.call_streaming(messages, tool_schemas).await?;
let mut assistant_content = String::new();
while let Some(event) = stream.next().await {
    match event? {
        LlmStreamEvent::Chunk(text) => {
            assistant_content.push_str(&text);
            // 渲染辅助消息块（带 is_streaming: true）
            output.streaming_assistant(&assistant_content);
        }
        LlmStreamEvent::ToolCallDelta(tc) => {
            // 收集工具调用信息
            tool_calls.push(tc);
        }
        LlmStreamEvent::Done => break,
    }
}
```

**Assistant 块渲染:**
- 流式输出时，在内容末尾添加 `▊` 闪烁光标（或 `...` 动画）
- 完成时移除光标，设置 `is_streaming: false`
- 使用 `\x1b[5m▊\x1b[0m` (ANSI 闪烁) 或定时更新

**测试策略:**
- 为 `LlmClient::call_streaming()` 编写 mock provider 测试
- 使用 `tokio::sync::mpsc` 模拟流式事件
- 验证 `MessageBlock::Assistant { is_streaming: true }` 的渲染输出

---

### Phase 2: 工具执行可视化 (P1)

**目标**: 在工具执行期间提供实时反馈，减少视觉噪音

**改动文件** (3 个文件):

| 文件 | 改动 |
|------|------|
| `src/agent/mod.rs` | 工具调用时输出进度信息，显示工具名称和参数摘要 |
| `src/repl.rs` | 优化 `derive_thinking_status()` 显示更精确的状态 |
| `src/ui/mod.rs` | 新增 `render_tool_progress()` 显示工具执行进度条 |

**具体实现:**
- 工具调用时显示 `🔧 ReadFile("src/main.rs")` 并立即渲染
- 工具完成后，在同一行更新为 `✅ ReadFile("src/main.rs")` (或覆盖)
- 长耗时工具（>3秒）显示进度动画
- 批量工具（如搜索多个文件）显示 `📂 搜索中... (3/15)` 计数

**渲染方式:**
```rust
// 工具执行开始
render_block(&MessageBlock::ToolCall { tool_name, args }, md)?;
// 工具执行完成——更新状态
// 使用 \x1b[1A 回到上一行，覆盖 ToolCall 行
write!(stdout, "\x1b[1A\x1b[2K\r")?;
render_block(&MessageBlock::ToolResult { tool_name, success, content }, md)?;
```

---

### Phase 3: 状态栏与输入面板增强 (P1)

**目标**: 提供更丰富、更美观的状态信息

**改动文件** (2 个文件):

| 文件 | 改动 |
|------|------|
| `src/ui/mod.rs` | 增强 `render_input_panel()` 显示更多信息 |
| `src/repl.rs` | 传递更丰富的状态信息（运行时间、当前阶段） |

**具体实现:**
- 状态栏显示格式: `⏳ 正在执行工具调用... [0:12]`
- 在 verbose 模式下显示 `🔊 详细模式`
- Pipeline 运行时显示 `🚀 流水线阶段 3/5: 代码审查`
- 不在状态栏显示 `⏳`/`⌛` 交替，使用固定图标 + 有意义的状态文本

---

### Phase 4: 消息节流与视觉优化 (P2)

**目标**: 减少快速消息爆发时的视觉噪音，提升整体美观度

**改动文件** (3 个文件):

| 文件 | 改动 |
|------|------|
| `src/ui/mod.rs` | 添加消息去重和节流逻辑 |
| `src/repl.rs` | 合并连续相同类型的消息 |
| `src/ui/blocks.rs` | 优化分隔线样式，添加消息计数 |

**具体实现:**
- 50ms 内的连续相同类型消息合并为一条
- `🔧 ReadFile` 连续调用 x15 显示为 `🔧 ReadFile (×15)`
- 分隔线使用更细的 `╌` 而非 `─`，减少视觉突兀
- 添加 `render_blocks_to_string()` 的批量渲染优化

---

### 实施优先级与估算

| 优先级 | 阶段 | 改动文件数 | 预计工作量 | 用户感知收益 |
|--------|------|-----------|-----------|------------|
| **P0** | Phase 1: LLM 流式输出 | 8 | 2-3 天 | 极高 — 消除等待空白期 |
| **P1** | Phase 2: 工具可视化 | 3 | 1 天 | 高 — 实时反馈 |
| **P1** | Phase 3: 状态栏增强 | 2 | 0.5 天 | 中 — 更好的信息展示 |
| **P2** | Phase 4: 消息节流 | 3 | 0.5 天 | 中 — 减少视觉噪音 |

**总计**: 4 phases, 4-5 天工作量

---

### 测试策略

- **Phase 1**: Mock LLM provider 返回流式事件，验证 `MessageBlock` 实时更新
- **Phase 2**: 验证工具进度渲染输出，测试 `render_tool_progress()` 格式
- **Phase 3**: 测试 `render_input_panel()` 不同状态下的输出
- **Phase 4**: 测试消息去重逻辑，验证节流行为

所有渲染测试使用 `render_blocks_to_string()` 纯函数，无需 mock stdout。