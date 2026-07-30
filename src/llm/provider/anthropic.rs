use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde_json::Value;
use std::pin::Pin;
use tracing::debug;
use tracing::warn;

use super::super::models::*;
use super::LlmProvider;
use crate::utils::error::AppError;

/// Anthropic Claude provider
pub struct AnthropicProvider {
    config: ProviderConfig,
}

impl AnthropicProvider {
    pub fn new(config: &ProviderConfig) -> Result<Self, AppError> {
        if config.api_key.is_none() {
            return Err(AppError::Config(
                "Anthropic provider requires api_key".to_string(),
            ));
        }
        Ok(Self {
            config: config.clone(),
        })
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn chat(
        &self,
        http_client: &Client,
        request: &LlmRequest,
    ) -> Result<LlmResponse, AppError> {
        let api_url = self.config.api_url.trim_end_matches('/').to_string();
        let url = if api_url.ends_with("/v1/messages") {
            api_url
        } else {
            format!("{}/v1/messages", api_url)
        };

        let api_key = self.config.api_key.as_deref().unwrap_or("");

        // 分离 system 消息（Anthropic 的 system 是独立字段）
        let mut system: Option<String> = None;
        let mut api_messages: Vec<Value> = Vec::new();

        for msg in &request.messages {
            if msg.role == "system" {
                system = msg.content.clone();
            } else {
                let mut api_msg = serde_json::json!({
                    "role": msg.role,
                });
                if let Some(ref content) = msg.content {
                    api_msg["content"] = serde_json::Value::String(content.clone());
                }
                if let Some(ref tool_calls) = msg.tool_calls {
                    // Anthropic 把 tool_calls 放在 content 数组中
                    let mut content_parts: Vec<Value> = Vec::new();
                    for tc in tool_calls {
                        content_parts.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.function.name,
                            "input": tc.function.arguments,
                        }));
                    }
                    if let Some(ref text) = msg.content {
                        content_parts.insert(0, serde_json::json!({"type": "text", "text": text}));
                    }
                    api_msg["content"] = serde_json::Value::Array(content_parts);
                }
                if let Some(ref tool_call_id) = msg.tool_call_id {
                    // Tool result
                    api_msg["role"] = "user".into();
                    let content = serde_json::json!([{
                        "type": "tool_result",
                        "tool_use_id": tool_call_id,
                        "content": msg.content.as_deref().unwrap_or(""),
                    }]);
                    api_msg["content"] = content;
                }
                api_messages.push(api_msg);
            }
        }

        // 构建 Anthropic 格式的请求体
        let mut body = serde_json::json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": api_messages,
        });

        if let Some(s) = system {
            body["system"] = serde_json::Value::String(s);
        }

        // 转换 tools 格式
        if let Some(tools) = &request.tools {
            let anthropic_tools: Vec<Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.function.name,
                        "description": t.function.description,
                        "input_schema": t.function.parameters,
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(anthropic_tools);
        }

        debug!(url = %url, model = %request.model, "Anthropic chat request");

        let response = http_client
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let retry_after = response.headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(std::time::Duration::from_secs);
            let body_text = response.text().await.unwrap_or_default();
            let msg = format!("Anthropic API returned error (status {}): {}", status, body_text);
            return Err(if status.as_u16() == 429 {
                AppError::RateLimited { message: msg, retry_after }
            } else {
                AppError::Llm(msg)
            });
        }

        let data: Value = response.json().await?;
        normalize_anthropic_response(data)
    }

    async fn chat_stream(
        &self,
        http_client: &Client,
        request: &LlmRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmStreamEvent, AppError>> + Send>>, AppError>
    {
        let api_url = self.config.api_url.trim_end_matches('/').to_string();
        let url = if api_url.ends_with("/v1/messages") {
            api_url
        } else {
            format!("{}/v1/messages", api_url)
        };

        let api_key = self.config.api_key.as_deref().unwrap_or("");

        // 分离 system 消息（Anthropic 的 system 是独立字段）
        let mut system: Option<String> = None;
        let mut api_messages: Vec<Value> = Vec::new();

        for msg in &request.messages {
            if msg.role == "system" {
                system = msg.content.clone();
            } else {
                let mut api_msg = serde_json::json!({
                    "role": msg.role,
                });
                if let Some(ref content) = msg.content {
                    api_msg["content"] = serde_json::Value::String(content.clone());
                }
                if let Some(ref tool_calls) = msg.tool_calls {
                    // Anthropic 把 tool_calls 放在 content 数组中
                    let mut content_parts: Vec<Value> = Vec::new();
                    for tc in tool_calls {
                        content_parts.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.function.name,
                            "input": tc.function.arguments,
                        }));
                    }
                    if let Some(ref text) = msg.content {
                        content_parts.insert(0, serde_json::json!({"type": "text", "text": text}));
                    }
                    api_msg["content"] = serde_json::Value::Array(content_parts);
                }
                if let Some(ref tool_call_id) = msg.tool_call_id {
                    // Tool result
                    api_msg["role"] = "user".into();
                    let content = serde_json::json!([{
                        "type": "tool_result",
                        "tool_use_id": tool_call_id,
                        "content": msg.content.as_deref().unwrap_or(""),
                    }]);
                    api_msg["content"] = content;
                }
                api_messages.push(api_msg);
            }
        }

        // 构建 Anthropic 格式的请求体（启用流式）
        let mut body = serde_json::json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": api_messages,
            "stream": true,
        });

        if let Some(s) = system {
            body["system"] = serde_json::Value::String(s);
        }

        // 转换 tools 格式
        if let Some(tools) = &request.tools {
            let anthropic_tools: Vec<Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.function.name,
                        "description": t.function.description,
                        "input_schema": t.function.parameters,
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(anthropic_tools);
        }

        debug!(url = %url, model = %request.model, "Anthropic chat stream request");

        let response = http_client
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let retry_after = response.headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(std::time::Duration::from_secs);
            let body_text = response.text().await.unwrap_or_default();
            let msg = format!("Anthropic API returned error (status {}): {}", status, body_text);
            return Err(if status.as_u16() == 429 {
                AppError::RateLimited { message: msg, retry_after }
            } else {
                AppError::Llm(msg)
            });
        }

        // 解析 SSE 流
        Ok(parse_anthropic_sse_stream(response))
    }
}

/// 解析 Anthropic SSE (Server-Sent Events) 流式响应。
///
/// Anthropic 格式示例：
/// ```
/// event: message_start
/// data: {"type":"message_start","message":{"id":"msg_123","type":"message","role":"assistant","content":[...]}}
///
/// event: content_block_start
/// data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}
///
/// event: content_block_delta
/// data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}
///
/// event: message_stop
/// data: {"type":"message_stop"}
/// ```
fn parse_anthropic_sse_stream(
    response: reqwest::Response,
) -> Pin<Box<dyn Stream<Item = Result<LlmStreamEvent, AppError>> + Send>> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    // 使用 tokio_util::StreamReader 将 Stream 转换为 AsyncRead
    let stream = response.bytes_stream();
    let stream = stream.map(|result| {
        result.map(|bytes| tokio_util::bytes::Bytes::from(bytes))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    });
    let reader = tokio_util::io::StreamReader::new(stream);
    let reader = BufReader::new(reader);
    let mut reader = Box::pin(reader);

    Box::pin(async_stream::try_stream! {
        let mut lines = reader.as_mut().lines();

        while let Ok(Some(line_result)) = lines.next_line().await {
            let line = line_result.trim();

            let line = line.trim();

            // 跳过空行
            if line.is_empty() {
                continue;
            }

            // Anthropic SSE 格式: "event: type\ndata: {...}"
            if let Some(data_str) = line.strip_prefix("data: ") {
                let data: Value = match serde_json::from_str(data_str) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(error = %e, "Failed to parse SSE data line, skipping");
                        continue;
                    }
                };

                let event_type = data["type"].as_str().unwrap_or("");

                match event_type {
                    "message_start" => {
                        // message_start 事件，开始累积内容
                    }
                    "content_block_start" => {
                        // content_block_start 事件，开始一个新的内容块
                        // 这里可以记录当前块的类型（text 或 tool_use）
                    }
                    "content_block_delta" => {
                        // content_block_delta 事件，包含增量内容
                        if let Some(delta) = data["delta"].as_object() {
                            // 文本增量
                            if let Some(text) = delta.get("text_delta").and_then(|v| v.as_str()) {
                                if !text.is_empty() {
                                    yield LlmStreamEvent::Chunk(text.to_string());
                                }
                            }

                            // 工具调用增量
                            // Anthropic 格式: {"type":"tool_use","name":"ReadFile","input":"{...}"}
                            if let Some(tool_use) = delta.get("tool_use") {
                                let name = tool_use.get("name").and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string();
                                let input = tool_use.get("input").and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string();

                                let parsed_args = parse_arguments(serde_json::Value::String(input.to_string()))?;
                                yield LlmStreamEvent::ToolCallDelta(ToolCall {
                                    id: uuid::Uuid::new_v4().to_string(), // Anthropic 不提供 ID，生成一个
                                    function: ToolCallFunction {
                                        name,
                                        arguments: parsed_args,
                                    },
                                });
                            }
                        }
                    }
                    "content_block_stop" => {
                        // content_block_stop 事件，结束当前内容块
                    }
                    "message_delta" => {
                        // message_delta 事件，包含最后的 delta
                        // 可能包含 usage 信息等
                    }
                    "message_stop" => {
                        // message_stop 事件，消息结束
                        yield LlmStreamEvent::Done;
                    }
                    "error" => {
                        // 错误事件
                        if let Some(msg) = data["error"].as_str() {
                            Err(AppError::Llm(format!("Anthropic stream error: {}", msg)))?;
                        }
                    }
                    _ => {
                        debug!(event_type = %event_type, "Unknown Anthropic SSE event, skipping");
                    }
                }
            }
        }
    })
}

/// 解析工具调用参数（简单版本，用于 Anthropic 流式响应）。
fn parse_arguments(args: Value) -> Result<Value, AppError> {
    if let Some(s) = args.as_str() {
        if let Ok(v) = serde_json::from_str(s) {
            return Ok(v);
        }
        Ok(serde_json::Value::String(s.to_string()))
    } else {
        Ok(args)
    }
}

/// 标准化 Anthropic 的响应为 LlmResponse
fn normalize_anthropic_response(data: Value) -> Result<LlmResponse, AppError> {
    let content = data["content"].as_array().ok_or_else(|| {
        AppError::Llm(format!(
            "Anthropic response missing 'content' array: {}",
            serde_json::to_string(&data).unwrap_or_default()
        ))
    })?;

    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    for block in content {
        match block["type"].as_str() {
            Some("text") => {
                if let Some(t) = block["text"].as_str() {
                    text_parts.push(t.to_string());
                }
            }
            Some("tool_use") => {
                let args = block["input"].clone();
                tool_calls.push(ToolCall {
                    id: block["id"].as_str().unwrap_or_default().to_string(),
                    function: ToolCallFunction {
                        name: block["name"].as_str().unwrap_or_default().to_string(),
                        arguments: args,
                    },
                });
            }
            _ => {}
        }
    }

    if !tool_calls.is_empty() {
        return Ok(LlmResponse::ToolCalls(tool_calls));
    }

    let text = text_parts.join("\n");
    if text.is_empty() {
        return Err(AppError::Llm(
            "Anthropic response has no text or tool calls".to_string(),
        ));
    }
    Ok(LlmResponse::Text(text))
}