use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde_json::Value;
use std::pin::Pin;
use tracing::debug;
use tracing::warn;

use super::super::models::*;
use super::{parse_arguments, LlmProvider};
use crate::utils::error::AppError;

/// OpenAI / OpenAI-compatible provider（Ollama /v1/chat/completions 等）
pub struct OpenAIProvider {
    config: ProviderConfig,
}

impl OpenAIProvider {
    pub fn new(config: &ProviderConfig) -> Result<Self, AppError> {
        Ok(Self {
            config: config.clone(),
        })
    }

    fn build_request_body(&self, request: &LlmRequest, stream: bool) -> Value {
        let mut body = serde_json::json!({
            "model": request.model,
            "messages": request.messages,
            "temperature": request.temperature,
            "stream": stream,
        });
        // 输出上限：未设置（None）则省略该字段，由 provider 用自身默认；wire key 仍为 max_tokens。
        if let Some(n) = request.max_output_tokens {
            body["max_tokens"] = serde_json::json!(n);
        }
        // 推理力度：未配置则省略，由服务端默认（多数为 medium/high）。
        // 商汤 SenseNova、DeepSeek、Kimi、GLM 等 OpenAI 兼容端点均支持顶层该字段。
        if let Some(ref effort) = request.reasoning_effort {
            if !effort.trim().is_empty() {
                body["reasoning_effort"] = serde_json::json!(effort.trim());
            }
        }
        if stream {
            body["stream_options"] = serde_json::json!({"include_usage": true});
        }
        if let Some(tools) = &request.tools {
            if !tools.is_empty() {
                // 序列化失败时省略 tools 字段，避免写入 null 触发 provider 400
                if let Ok(value) = serde_json::to_value(tools) {
                    body["tools"] = value;
                } else {
                    warn!("Failed to serialize tools, omitting 'tools' field");
                }
            }
        }
        body
    }

    fn api_url(&self) -> String {
        let api_url = self.config.api_url.trim_end_matches('/').to_string();
        if api_url.ends_with("/chat/completions") {
            api_url
        } else {
            format!("{}/chat/completions", api_url)
        }
    }

    fn build_request<'a>(
        &self,
        http_client: &'a Client,
        body: &'a Value,
        stream: bool,
    ) -> reqwest::RequestBuilder {
        let mut req = http_client
            .post(self.api_url())
            .header("Content-Type", "application/json")
            .json(body);

        if let Some(ref key) = self.config.api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }

        // 非流式请求设总超时兜底；流式请求不设总超时，由 EOF/[DONE]/读错误
        // 自然终止——总超时会覆盖整个响应体读取，长对话流式输出会被
        // reqwest 以 "error decoding response body" 中止。
        if !stream {
            req = req.timeout(super::common::non_stream_timeout());
        }

        req
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    async fn chat(
        &self,
        http_client: &Client,
        request: &LlmRequest,
    ) -> Result<LlmResponse, AppError> {
        let body = self.build_request_body(request, false);

        debug!(url = %self.api_url(), model = %request.model, "OpenAI chat request");

        let response = self.build_request(http_client, &body, false).send().await?;
        let status = response.status();

        if !status.is_success() {
            let headers = response.headers().clone();
            let body_text = response.text().await.unwrap_or_default();
            return Err(map_http_error(status, &headers, &body_text, "LLM API"));
        }

        let data: Value = response.json().await?;
        normalize_chat_response(data)
    }

    async fn chat_stream(
        &self,
        http_client: &Client,
        request: &LlmRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmStreamEvent, AppError>> + Send>>, AppError>
    {
        let body = self.build_request_body(request, true);

        debug!(url = %self.api_url(), model = %request.model, "OpenAI chat stream request");

        let response = self.build_request(http_client, &body, true).send().await?;
        let status = response.status();

        if !status.is_success() {
            let headers = response.headers().clone();
            let body_text = response.text().await.unwrap_or_default();
            return Err(map_http_error(status, &headers, &body_text, "LLM API"));
        }

        // SSE 流式解析
        let stream = response.bytes_stream();
        let mapped = parse_openai_sse_stream(stream);
        Ok(Box::pin(mapped))
    }
}

/// 标准化 OpenAI 兼容的 chat completion 响应
fn normalize_chat_response(data: Value) -> Result<LlmResponse, AppError> {
    let choices = data["choices"].as_array().ok_or_else(|| {
        AppError::Llm(format!(
            "LLM response missing 'choices' array: {}",
            serde_json::to_string(&data).unwrap_or_default()
        ))
    })?;

    if choices.is_empty() {
        return Err(AppError::Llm(format!(
            "LLM response has empty 'choices' array: {}",
            serde_json::to_string(&data).unwrap_or_default()
        )));
    }

    let message = &choices[0]["message"];
    let content_val = &message["content"];
    let content = content_val.as_str().unwrap_or("");
    let tool_calls = message["tool_calls"].as_array();

    if let Some(tcs) = tool_calls {
        let mut calls = Vec::new();
        for tc in tcs {
            let args = parse_arguments(tc["function"]["arguments"].clone())?;
            calls.push(ToolCall {
                id: tc["id"].as_str().unwrap_or_default().to_string(),
                function: ToolCallFunction {
                    name: tc["function"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    arguments: args,
                },
            });
        }
        Ok(LlmResponse::ToolCalls(calls))
    } else {
        Ok(LlmResponse::Text(content.to_string()))
    }
}

/// 解析 OpenAI SSE (Server-Sent Events) 流式响应。
///
/// SSE 格式示例：
/// ```
/// data: {"choices":[{"delta":{"content":"Hello"}}],"id":"chatcmpl-123"}
///
/// data: {"choices":[{"delta":{"content":" World"}}],"id":"chatcmpl-123"}
///
/// data: [DONE]
/// ```
///
/// 处理要点：
/// - 消息可能被分割到多个 TCP 包（需要缓冲）
/// - 提取 `choices[0].delta.content` 作为文本块
/// - 提取 `choices[0].delta.tool_calls` 作为工具调用增量
/// - 使用 `IndexMap` 重建工具调用索引（因为 tool_calls 是数组，索引可能变化）
fn parse_openai_sse_stream<S, E>(
    stream: S,
) -> Pin<Box<dyn Stream<Item = Result<LlmStreamEvent, AppError>> + Send>>
where
    S: Stream<Item = Result<tokio_util::bytes::Bytes, E>> + Send + 'static,
    E: std::fmt::Display + 'static,
{
    // 使用 tokio_util::io::StreamReader 将 Stream 转换为 AsyncRead
    use tokio::io::{AsyncBufReadExt, BufReader};

    let stream = stream.map(|result| {
        result.map_err(|e| std::io::Error::other(e.to_string()))
    });
    let reader = tokio_util::io::StreamReader::new(stream);
    let reader = BufReader::new(reader);
    // 使用 Box::pin 确保 BufReader<StreamReader<...>> 满足 Unpin
    let mut reader = Box::pin(reader);

    /// 按 index 累积工具调用增量
    struct AccToolCall {
        id: String,
        name: String,
        arguments: String,
    }

    Box::pin(async_stream::try_stream! {
        let mut lines = reader.as_mut().lines();
        // 使用 Vec 保持插入顺序（按 index 升序）
        let mut acc_tool_calls: Vec<(usize, AccToolCall)> = Vec::new();

        let mut done_emitted = false;
        // 计数已产出的有效内容事件（文本/思考/工具调用）。
        // 不计 Usage 帧：仅带 token 统计的空响应同样属于失败，不能算作有效输出。
        let mut event_count: usize = 0;

        /// 将累积的工具调用逐个 yield 出去，然后清空缓冲区。
        /// 返回的 Vec 保留已 yield 的 index，用于去重。
        macro_rules! flush_tool_calls {
            () => {{
                let mut yielded_indices = Vec::new();
                // 按 index 排序确保输出顺序稳定
                acc_tool_calls.sort_by_key(|(idx, _)| *idx);
                for (idx, acc) in &acc_tool_calls {
                    if !acc.name.is_empty() {
                        match parse_arguments(serde_json::Value::String(acc.arguments.clone())) {
                            Ok(parsed_args) => {
                                event_count += 1;
                                yield LlmStreamEvent::ToolCallDelta(ToolCall {
                                    id: acc.id.clone(),
                                    function: ToolCallFunction {
                                        name: acc.name.clone(),
                                        arguments: parsed_args,
                                    },
                                });
                                yielded_indices.push(*idx);
                            }
                            Err(e) => {
                                warn!(error = %e, tool = %acc.name, "Failed to parse accumulated tool arguments");
                            }
                        }
                    }
                }
                // 只移除已 yield 的条目，保留只有 arguments 碎片的（可能名称在后续事件中才到）
                acc_tool_calls.retain(|(idx, _)| !yielded_indices.contains(idx));
                yielded_indices
            }};
        }

        loop {
            match lines.next_line().await {
                Ok(Some(line_result)) => {
                    let line = line_result.trim();

                    // 跳过空行
                    if line.is_empty() {
                        continue;
                    }

                    // SSE 格式: "data: {...}"（容忍 "data:" 后无空格或多余空格）
                    let Some(rest) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data_str = rest.trim_start();

                    // 处理结束信号
                    if data_str == "[DONE]" {
                        // 在结束前先 flush 所有累积的工具调用
                        flush_tool_calls!();
                        yield LlmStreamEvent::Done;
                        done_emitted = true;
                        break;
                    }

                    // 解析 JSON
                    let data: Value = match serde_json::from_str(data_str) {
                        Ok(v) => v,
                        Err(e) => {
                            warn!(error = %e, "Failed to parse SSE data line, skipping");
                            continue;
                        }
                    };

                    // 流内错误帧：OpenRouter 等网关在已返回 HTTP 200 后，
                    // 会以单条 `data: {"error":{...}}` 帧上报上游故障/限流，
                    // 此时 choices 为空且不会有后续 [DONE]。
                    // 必须上抛为错误，否则会被当成"无内容的正常结束"，
                    // 上层误判为"LLM 返回空响应"而反复重试同一请求。
                    if let Some(err) = data.get("error").filter(|e| !e.is_null()) {
                        yield Err(stream_error_to_app_error(err))?;
                        break;
                    }

                    // 提取 token 用量（OpenAI 在最后一个带 usage 的 chunk 中返回）
                    // 即使 choices 为空，usage 也可能存在
                    if let Some(usage) = data.get("usage") {
                        if !usage.is_null() {
                            let prompt_tokens = usage["prompt_tokens"].as_u64().unwrap_or(0) as usize;
                            let completion_tokens = usage["completion_tokens"].as_u64().unwrap_or(0) as usize;
                            let total_tokens = usage["total_tokens"].as_u64().unwrap_or(0) as usize;
                            if total_tokens > 0 {
                                yield LlmStreamEvent::Usage(TokenUsage {
                                    prompt_tokens,
                                    completion_tokens,
                                    total_tokens,
                                });
                            }
                        }
                    }

                    // 提取 delta 内容
                    if let Some(delta) = data["choices"].as_array()
                        .and_then(|arr| arr.first())
                        .and_then(|c| c["delta"].as_object())
                    {
                        // 处理思考模型推理增量：reasoning_content（OpenAI o系列/商汤/GLM/DeepSeek-flash 风格）
                        // 与 thinking_content（DeepSeek-v4-pro/Kimi 风格）。仅用于 UI 展示，不进历史。
                        let reasoning = delta
                            .get("reasoning_content")
                            .or_else(|| delta.get("thinking_content"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !reasoning.is_empty() {
                            event_count += 1;
                            yield LlmStreamEvent::Reasoning(reasoning.to_string());
                        }

                        // 处理文本增量
                        if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
                            if !content.is_empty() {
                                event_count += 1;
                                yield LlmStreamEvent::Chunk(content.to_string());
                            }
                        }

                        // 处理工具调用增量：按 index 累积
                        if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                            for tc in tool_calls {
                                let index = tc["index"].as_i64().unwrap_or(0) as usize;

                                // 查找或创建该 index 的累积条目
                                let pos = acc_tool_calls.iter().position(|(i, _)| *i == index);
                                if let Some(pos) = pos {
                                    let acc = &mut acc_tool_calls[pos].1;
                                    // 合并增量
                                    if let Some(id) = tc["id"].as_str() {
                                        if !id.is_empty() {
                                            acc.id = id.to_string();
                                        }
                                    }
                                    if let Some(func) = tc["function"].as_object() {
                                        if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                                            if !name.is_empty() {
                                                acc.name = name.to_string();
                                            }
                                        }
                                        if let Some(args) = func.get("arguments").and_then(|v| v.as_str()) {
                                            acc.arguments.push_str(args);
                                        }
                                    }
                                } else {
                                    // 创建新的累积条目
                                    let id = tc["id"].as_str().unwrap_or_default().to_string();
                                    let func_obj = tc["function"].as_object().cloned().unwrap_or_default();
                                    let name = func_obj.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                                    let arguments = func_obj.get("arguments").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                                    acc_tool_calls.push((index, AccToolCall { id, name, arguments }));
                                }
                            }
                        }
                    }
                }
                Ok(None) => {
                    // EOF：连接在 [DONE] 之前关闭。
                    // 若整条流一个事件都没产出，说明这次请求实际上失败了
                    // （网关空响应、连接被重置等），必须上抛错误让上层重试/故障转移，
                    // 否则会被当作"LLM 返回空响应"，白等 2s+4s 后以同一方式失败。
                    if event_count == 0 {
                        yield Err(AppError::Llm(
                            "OpenAI SSE stream closed without any event".to_string(),
                        ))?;
                        break;
                    }
                    // 已有内容但缺 [DONE]：属于可容忍的收尾异常（如上游截断前发完最后一块），
                    // flush 剩余 tool calls 并补发 Done，保证已产出的内容不丢失。
                    warn!("OpenAI SSE stream ended without [DONE] sentinel; flushing remaining tool calls");
                    flush_tool_calls!();
                    if !done_emitted {
                        yield LlmStreamEvent::Done;
                    }
                    break;
                }
                Err(e) => {
                    // 读取错误：向上传播，避免消费者拿到截断响应却无错误信号
                    warn!(error = %e, "OpenAI SSE stream read error");
                    yield Err(AppError::Llm(format!("OpenAI SSE stream read error: {}", e)))?;
                    break;
                }
            }
        }
    })
}

/// 将 SSE 流内 `error` 帧映射为 AppError。
///
/// 网关（OpenRouter 等）在已发出 HTTP 200 之后，用单条
/// `data: {"error":{"code":...,"message":...}}` 帧上报失败。
/// 映射规则与 [`map_http_error`] 对齐：429 → RateLimited（走退避重试并读 Retry-After），
/// 5xx → ServerError（走退避重试），其余 → Llm（立即故障转移）。
/// 这样上层 `retry_class` 能正确识别流内瞬时故障，而不是当成空响应原地重试。
fn stream_error_to_app_error(err: &Value) -> AppError {
    let code = err.get("code").and_then(|c| c.as_u64()).unwrap_or(0) as u16;
    let message = err
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("upstream stream error");
    let detail = match err.pointer("/metadata/error_type").and_then(|v| v.as_str()) {
        Some(t) => format!("{} (error_type={})", message, t),
        None => message.to_string(),
    };
    let msg = format!("LLM stream error: {}", detail);

    if code == 429 {
        AppError::RateLimited { message: msg, retry_after: None }
    } else if code >= 500 {
        AppError::ServerError(code, msg)
    } else {
        AppError::Llm(msg)
    }
}

/// 将 HTTP 错误状态映射为 AppError，供 chat 与 chat_stream 共用。
fn map_http_error(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body_text: &str,
    source: &str,
) -> AppError {
    let retry_after = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(std::time::Duration::from_secs);
    let msg = format!("{} API returned error (status {}): {}", source, status, body_text);
    if status.as_u16() == 429 {
        AppError::RateLimited { message: msg, retry_after }
    } else if status.as_u16() >= 500 {
        AppError::ServerError(status.as_u16(), msg)
    } else {
        AppError::Llm(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;

    /// 把若干原始 SSE 行喂给解析器，收集全部事件。
    fn collect_sse_events(lines: &[&str]) -> Vec<Result<LlmStreamEvent, AppError>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let body: String = lines.iter().map(|l| format!("{}\n", l)).collect();
        let bytes: Vec<Result<tokio_util::bytes::Bytes, std::io::Error>> =
            vec![Ok(tokio_util::bytes::Bytes::from(body))];
        let mut parsed = parse_openai_sse_stream(stream::iter(bytes));
        runtime.block_on(async {
            let mut out = Vec::new();
            while let Some(ev) = parsed.next().await {
                out.push(ev);
            }
            out
        })
    }

    /// 核心回归：网关在 HTTP 200 之后用单条 `error` 帧上报 502。
    /// 修复前该帧被静默忽略 → 落到 EOF 分支 → 上层误判"LLM 返回空响应"并原地重试 3 次。
    #[test]
    fn sse_error_frame_is_surfaced_as_error_not_silence() {
        let events = collect_sse_events(&[
            r#"data: {"id":"gen-1","object":"chat.completion.chunk","choices":[],"error":{"code":502,"message":"JSON error injected into SSE stream","metadata":{"error_type":"provider_unavailable"}}}"#,
        ]);

        assert_eq!(events.len(), 1, "error frame should terminate the stream");
        let err = events[0].as_ref().expect_err("error frame must not be Ok");
        // 5xx 映射为 ServerError，才能被 retry_class 识别为可重试
        assert!(
            matches!(err, AppError::ServerError(502, _)),
            "expected ServerError(502), got {:?}",
            err
        );
        assert!(err.to_string().contains("provider_unavailable"));
        // 绝不能出现 Done——否则会被当成"正常收尾但无内容"
        assert!(
            !events.iter().any(|e| matches!(e, Ok(LlmStreamEvent::Done))),
            "must not emit Done for an error frame"
        );
    }

    #[test]
    fn sse_error_frame_code_429_maps_to_rate_limited() {
        let events = collect_sse_events(&[
            r#"data: {"choices":[],"error":{"code":429,"message":"rate limited"}}"#,
        ]);
        let err = events[0].as_ref().expect_err("should be Err");
        assert!(matches!(err, AppError::RateLimited { .. }), "got {:?}", err);
        assert!(err.is_rate_limited());
    }

    /// 回归：连接建立后一个事件都没产出就 EOF（网关空响应）。
    /// 修复前补发 Done → 上层判"空响应"并重试；修复后直接上抛错误。
    #[test]
    fn sse_empty_stream_surfaces_error_instead_of_done() {
        let events = collect_sse_events(&[]);
        assert_eq!(events.len(), 1);
        let err = events[0].as_ref().expect_err("empty stream must be Err");
        assert!(err.to_string().contains("without any event"), "got {}", err);
    }

    /// 仅带 usage 的空响应同样算失败（无任何内容产出）。
    #[test]
    fn sse_usage_only_stream_is_treated_as_empty() {
        let events = collect_sse_events(&[
            r#"data: {"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":0,"total_tokens":10}}"#,
        ]);
        assert_eq!(events.len(), 2, "expect Usage then Err");
        assert!(matches!(events[0], Ok(LlmStreamEvent::Usage(_))));
        assert!(events[1].is_err(), "usage-only stream must end with Err");
    }

    /// 保留行为：已有内容但缺 [DONE] 时仍补发 Done，不丢已产出内容。
    #[test]
    fn sse_content_without_done_sentinel_still_completes() {
        let events = collect_sse_events(&[
            r#"data: {"choices":[{"delta":{"content":"部分内容"}}]}"#,
        ]);
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                Ok(LlmStreamEvent::Chunk(t)) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "部分内容", "content before EOF must be preserved");
        assert!(
            events.iter().any(|e| matches!(e, Ok(LlmStreamEvent::Done))),
            "should still emit Done after flushing content"
        );
        assert!(events.iter().all(|e| e.is_ok()), "no error expected");
    }

    /// 正常路径回归：[DONE] 收尾的流不应产生任何错误。
    #[test]
    fn sse_normal_done_stream_has_no_error() {
        let events = collect_sse_events(&[
            r#"data: {"choices":[{"delta":{"content":"你好"}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            r#"data: {"choices":[],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}"#,
            "data: [DONE]",
        ]);
        assert!(events.iter().all(|e| e.is_ok()), "unexpected error: {:?}", events);
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                Ok(LlmStreamEvent::Chunk(t)) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "你好");
        assert!(matches!(events.last().unwrap(), Ok(LlmStreamEvent::Done)));
    }

    /// error 帧前若已有内容，error 仍须上抛（不能被已产出的内容掩盖）。
    #[test]
    fn sse_error_frame_after_content_still_errors() {
        let events = collect_sse_events(&[
            r#"data: {"choices":[{"delta":{"content":"开头"}}]}"#,
            r#"data: {"choices":[],"error":{"code":503,"message":"upstream gone"}}"#,
        ]);
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], Ok(LlmStreamEvent::Chunk(_))));
        assert!(matches!(events[1], Err(AppError::ServerError(503, _))));
    }

    #[test]
    fn parse_arguments_parses_valid_json_object() {
        let args = serde_json::Value::String(r#"{"file_path":"src/main.rs","offset":10}"#.to_string());
        let parsed = parse_arguments(args).unwrap();
        assert_eq!(parsed["file_path"], "src/main.rs");
        assert_eq!(parsed["offset"], 10);
    }

    #[test]
    fn parse_arguments_handles_unescaped_newlines_in_strings() {
        let raw = r#"{"file_path":"src/main.rs","content":"line1
line2"}"#;
        let args = serde_json::Value::String(raw.to_string());
        let parsed = parse_arguments(args).unwrap();
        assert_eq!(parsed["file_path"], "src/main.rs");
        assert_eq!(parsed["content"].as_str().unwrap(), "line1\nline2");
    }

    #[test]
    fn parse_arguments_strips_markdown_fence() {
        let raw = "```json\n{\"file_path\":\"src/main.rs\"}\n```";
        let args = serde_json::Value::String(raw.to_string());
        let parsed = parse_arguments(args).unwrap();
        assert_eq!(parsed["file_path"], "src/main.rs");
    }

    #[test]
    fn parse_arguments_removes_trailing_comma() {
        let raw = r#"{"file_path":"src/main.rs","offset":10,}"#;
        let args = serde_json::Value::String(raw.to_string());
        let parsed = parse_arguments(args).unwrap();
        assert_eq!(parsed["file_path"], "src/main.rs");
    }

    #[test]
    fn parse_arguments_falls_back_to_raw_string_on_unrecoverable_input() {
        let raw = "not json at all";
        let args = serde_json::Value::String(raw.to_string());
        let parsed = parse_arguments(args).unwrap();
        assert_eq!(parsed.as_str().unwrap(), "not json at all");
    }
}