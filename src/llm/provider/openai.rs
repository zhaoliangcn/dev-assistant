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
            "max_tokens": request.max_tokens,
            "stream": stream,
        });
        if stream {
            body["stream_options"] = serde_json::json!({"include_usage": true});
        }
        if let Some(tools) = &request.tools {
            if !tools.is_empty() {
                body["tools"] = serde_json::to_value(tools).unwrap_or(Value::Null);
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
    ) -> reqwest::RequestBuilder {
        let mut req = http_client
            .post(self.api_url())
            .header("Content-Type", "application/json")
            .json(body);

        if let Some(ref key) = self.config.api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
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

        let response = self.build_request(http_client, &body).send().await?;
        let status = response.status();

        if !status.is_success() {
            let retry_after = response.headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(std::time::Duration::from_secs);
            let body_text = response.text().await.unwrap_or_default();
            let msg = format!("LLM API returned error (status {}): {}", status, body_text);
            return Err(if status.as_u16() == 429 {
                AppError::RateLimited { message: msg, retry_after }
            } else if status.as_u16() >= 500 {
                AppError::ServerError(status.as_u16(), msg)
            } else {
                AppError::Llm(msg)
            });
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

        let response = self.build_request(http_client, &body).send().await?;
        let status = response.status();

        if !status.is_success() {
            let retry_after = response.headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(std::time::Duration::from_secs);
            let body_text = response.text().await.unwrap_or_default();
            let msg = format!("LLM API returned error (status {}): {}", status, body_text);
            return Err(if status.as_u16() == 429 {
                AppError::RateLimited { message: msg, retry_after }
            } else if status.as_u16() >= 500 {
                AppError::ServerError(status.as_u16(), msg)
            } else {
                AppError::Llm(msg)
            });
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

        while let Ok(Some(line_result)) = lines.next_line().await {
            let line = line_result.trim();

            // 跳过空行
            if line.is_empty() {
                continue;
            }

            // SSE 格式: "data: {...}"
            let Some(data_str) = line.strip_prefix("data: ") else {
                continue;
            };

                // 处理结束信号
                if data_str == "[DONE]" {
                    // 在结束前先 flush 所有累积的工具调用
                    flush_tool_calls!();
                    yield LlmStreamEvent::Done;
                    continue;
                }

                // 解析 JSON
                let data: Value = match serde_json::from_str(data_str) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(error = %e, "Failed to parse SSE data line, skipping");
                        continue;
                    }
                };

                // 提取 delta 内容
                if let Some(delta) = data["choices"].as_array()
                    .and_then(|arr| arr.first())
                    .and_then(|c| c["delta"].as_object())
                {
                    // 处理文本增量
            if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
                if !content.is_empty() {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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