# OCR 对 `src/` Rust 代码的扫描审查报告

- 工具：`D:\opencodereview-windows-amd64.exe` v1.12.4
- 命令：`ocr scan --repo D:\dev-assistant --path src/...`
- LLM：`deepseek-v4-flash` @ `https://token.sensenova.cn`
- 扫描范围：`src/` 下 110 个 Rust 文件（排除 `src/web/static/**`、`src/web/templates/**`、`src/web/tests/**`）
- 状态：**部分完成**（受限于模型 rpm/tpm 配额）

## 环境限制

1. **git 版本过旧**（2.37.0 < 2.41.0）：`code_search` 工具调用 `git grep --max-count` 全部失败，导致 OCR 无法做跨文件搜索，只能靠 `file_read`/`file_find`。
2. **模型限流**：`token.sensenova.cn` 端点频繁返回 429（rpm/tpm exhausted），大批次扫描会因所有子任务失败而中止。
3. 因此本次仅成功完成两个批次的审查：**`src/llm/provider/openai.rs`** 与 **`src/llm/provider/ollama.rs`**（`anthropic.rs`/`common.rs`/`client.rs`/`models.rs` 因 429 失败）。

---

## 批次 1：`src/llm/provider/openai.rs`（3–4 条）

| 行号 | 严重度 | 类别 | 问题 |
|------|--------|------|------|
| 273 | **high** | bug | `while let Ok(Some(..))` 静默吞掉两种情况：① `next_line()` 返回 `Err`（连接重置）时循环退出，`try_stream!` 仍成功结束，消费者拿到截断响应而无错误/无 `Done`；② provider 未发送 `data: [DONE]` 时也不 emit `Done`，且累积的 tool_calls 未 flush。建议显式 match `Result`：`Err` 走 `return Err(...)`，`Ok(None)`/EOF 时 flush 并 emit `Done`（防重复），仅 `[DONE]` 时 break。 |
| 282 | low | bug | `strip_prefix("data: ")` 要求冒号后恰好一个空格。SSE 规范允许 `data:{json}`（无空格），多个 OpenAI 兼容 provider 就是这么发的，这些行会被静默丢弃。建议 `line.strip_prefix("data:").map(str::trim_start)`。 |
| 48 | low | bug | `serde_json::to_value(tools).unwrap_or(Value::Null)` 序列化失败时写入 `"tools": null`，很多 OpenAI 兼容服务端会 400 拒绝。建议 `?`/`map_err` 传播错误，或失败时省略该字段。 |
| 103–109 | low | maintainability | 状态码→`AppError` 映射（429→`RateLimited`+`retry_after`，≥500→`ServerError`，其他→`Llm`）与 `RETRY_AFTER` 解析在 `chat` 和 `chat_stream` 中重复。抽取共享 helper 保持一致。 |

---

## 批次 2：`src/llm/provider/ollama.rs`（6 条）

| 行号 | 严重度 | 类别 | 问题 |
|------|--------|------|------|
| 212–220 | **high** | bug | Ollama 流式返回的 `message.tool_calls[].function.arguments` 是分片 JSON 字符串，跨多个 `done:false` chunk 累积。当前代码对每个 chunk 都新建 `ToolCall` + `Uuid::new_v4()`，导致单个 tool call 被切成多个不稳定 ID 的 delta，下游无法重组。应在 chunk 间累积 name/arguments，仅当 `done:true` 时 emit 最终 `ToolCall`。 |
| 245–246 | medium | bug | Ollama `/api/chat` 的 `tool_calls` 项不含 `id` 字段，`tc["id"].as_str().unwrap_or_default()` 恒为空字符串。若下游依赖唯一 id（OpenAI 风格），所有 tool call 都会是空/重复 id。缺失时应回退生成 UUID，与流式路径一致。 |
| 175 | medium | bug | Ollama 可能在 `done:true` 消息里带最后一截文本内容，此分支只 yield Usage/ToolCall/Done，从不 flush `message.content`，尾部文本被静默丢弃。应在 yield `Done` 前 emit 剩余 content 与待处理的 tool-call 状态。 |
| 161 | medium | bug | 连接中断或 `next_line()` 返回 `Err` 时循环静默退出，返回的 stream 不 emit `Done`。等待 `Done` 的消费者会把中断当成功完成。EOF 时应 emit 终态错误/`Done`，或至少文档化此行为。 |
| 52 | low | bug | 同 openai.rs:48 —— `unwrap_or(Value::Null)` 吞掉序列化错误，发送 `"tools": null`。 |
| 45–47 | low | bug | `request.temperature` 为 `None` 时 `json!` 序列化为 `null`，Ollama options schema 期望数字，某些版本会拒绝。`None` 时应省略该 option。 |

---

## 关键共性发现（跨 provider）

1. **流式终止处理不健壮**：openai 与 ollama 都存在"连接中断/EOF 时不 emit `Done`、不 flush 累积状态"的问题，属于同一类缺陷，建议统一封装一个 stream-finalizer。
2. **`serde_json::to_value(...).unwrap_or(Value::Null)` 反模式**：openai.rs:48 与 ollama.rs:52 都存在，会向服务端发送显式 `null`，建议统一改为 `?` 传播或省略字段。
3. **SSE `data:` 前缀解析过严**：openai.rs:282 要求 `data: `（含空格），不兼容无空格变体。
4. **Tool call ID 生成不一致**：ollama 流式路径用 `Uuid::new_v4()`，非流式路径用 `unwrap_or_default()` 得到空字符串，两条路径行为不一致。

## 建议后续动作

1. 升级 git 到 ≥ 2.41.0，恢复 `code_search` 工具能力。
2. 换用更高 rpm/tpm 配额的模型（或降低 `--concurrency` 到 1）以完成剩余 100+ 文件的扫描。
3. 优先修复上述 2 条 high 严重度问题（openai.rs:273、ollama.rs:212–220），它们会导致流式响应静默截断/工具调用丢失。
4. 抽取公共 stream-finalizer 与 error-mapping helper，消除 openai/ollama 之间的重复与不一致。

## 遗留

- 本次仅覆盖 `src/llm/provider/{openai,ollama}.rs`（2/110 文件）。
- 剩余 108 个文件（agent、tools、security、orchestrator、scheduler、dream、hooks、utils、config、web 后端、prompt、app、main、repl、restart、session、skills、persist 等）因限流未完成，需换模型或分批重试。
