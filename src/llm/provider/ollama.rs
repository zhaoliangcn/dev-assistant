use async_trait::async_trait;
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use serde_json::Value;
use std::pin::Pin;
use tracing::debug;
use tracing::warn;

use super::super::models::*;
use super::LlmProvider;
use crate::utils::error::AppError;

/// Ollama provider — 使用原生 `/api/chat` 接口
pub struct OllamaProvider {
    config: ProviderConfig,
}

impl OllamaProvider {
    pub fn new(config: &ProviderConfig) -> Result<Self, AppError> {
        Ok(Self {
            config: config.clone(),
        })
    }

    fn build_request_body(&self, request: &LlmRequest, stream: bool) -> Value {
        let ollama_messages: Vec<Value> = request
            .messages
            .iter()
            .map(|msg| {
                let mut m = serde_json::json!({
                    "role": msg.role,
                });
                if let Some(ref content) = msg.content {
                    m["content"] = serde_json::Value::String(content.clone());
                }
                m
            })
            .collect();

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": ollama_messages,
            "stream": stream,
            "options": {
                "temperature": request.temperature,
            },
        });

        if let Some(tools) = &request.tools {
            if !tools.is_empty() {
                body["tools"] = serde_json::to_value(tools).unwrap_or(Value::Null);
            }
        }

        body
    }

    fn api_url(&self) -> String {
        let api_url = self.config.api_url.trim_end_matches('/').to_string();
        if api_url.ends_with("/api/chat") {
            api_url
        } else {
            format!("{}/api/chat", api_url)
        }
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn chat(
        &self,
        http_client: &Client,
        request: &LlmRequest,
    ) -> Result<LlmResponse, AppError> {
        let url = self.api_url();
        let body = self.build_request_body(request, false);

        debug!(url = %url, model = %request.model, "Ollama chat request");

        let response = http_client
            .post(&url)
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
            let msg = format!("Ollama API returned error (status {}): {}", status, body_text);
            return Err(if status.as_u16() == 429 {
                AppError::RateLimited { message: msg, retry_after }
            } else if status.as_u16() >= 500 {
                AppError::ServerError(status.as_u16(), msg)
            } else {
                AppError::Llm(msg)
            });
        }

        let data: Value = response.json().await?;
        normalize_ollama_response(data)
    }

    async fn chat_stream(
        &self,
        http_client: &Client,
        request: &LlmRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmStreamEvent, AppError>> + Send>>, AppError>
    {
        let url = self.api_url();
        let body = self.build_request_body(request, true);

        debug!(url = %url, model = %request.model, "Ollama chat stream request");

        let response = http_client
            .post(&url)
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
            let msg = format!("Ollama API returned error (status {}): {}", status, body_text);
            return Err(if status.as_u16() == 429 {
                AppError::RateLimited { message: msg, retry_after }
            } else if status.as_u16() >= 500 {
                AppError::ServerError(status.as_u16(), msg)
            } else {
                AppError::Llm(msg)
            });
        }

        let stream = response.bytes_stream();

        // 使用 tokio_util::io::StreamReader + BufReader 进行行缓冲读取，
        // 避免 NDJSON 行跨 TCP 分片时解析失败。
        use tokio::io::{AsyncBufReadExt, BufReader};
        let stream = stream.map(|result| {
            result.map_err(|e| std::io::Error::other(e.to_string()))
        });
        let reader = tokio_util::io::StreamReader::new(stream);
        let reader = BufReader::new(reader);
        let mut reader = Box::pin(reader);

        let mapped = async_stream::try_stream! {
            let mut lines = reader.as_mut().lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }

                let data: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(error = %e, line = %line, "Failed to parse Ollama NDJSON line, skipping");
                        continue;
                    }
                };

                if data["done"].as_bool().unwrap_or(false) {
                    // 提取 usage 信息（Ollama 在最后 done=true 的行中返回）
                    if let Some(prompt_count) = data.get("prompt_eval_count").and_then(|v| v.as_u64()) {
                        let eval_count = data.get("eval_count").and_then(|v| v.as_u64()).unwrap_or(0);
                        yield LlmStreamEvent::Usage(TokenUsage {
                            prompt_tokens: prompt_count as usize,
                            completion_tokens: eval_count as usize,
                            total_tokens: (prompt_count + eval_count) as usize,
                        });
                    }

                    // 处理工具调用：Ollama 在 done=true 行也可能携带 tool_calls
                    if let Some(tcs) = data["message"]["tool_calls"].as_array() {
                        if !tcs.is_empty() {
                            for tc in tcs {
                                if let Some(func) = tc.get("function") {
                                    let name = func.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                                    let arguments = func.get("arguments").cloned().unwrap_or(Value::Object(Default::default()));
                                    yield LlmStreamEvent::ToolCallDelta(ToolCall {
                                        id: uuid::Uuid::new_v4().to_string(),
                                        function: ToolCallFunction { name, arguments },
                                    });
                                }
                            }
                        }
                    }
                    yield LlmStreamEvent::Done;
                    continue;
                }

                // 处理文本内容
                if let Some(content) = data["message"]["content"].as_str() {
                    if !content.is_empty() {
                        yield LlmStreamEvent::Chunk(content.to_string());
                    }
                }

                // 处理工具调用增量
                if let Some(tcs) = data["message"]["tool_calls"].as_array() {
                    if !tcs.is_empty() {
                        for tc in tcs {
                            if let Some(func) = tc.get("function") {
                                let name = func.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                                let arguments = func.get("arguments").cloned().unwrap_or(Value::Object(Default::default()));
                                yield LlmStreamEvent::ToolCallDelta(ToolCall {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    function: ToolCallFunction { name, arguments },
                                });
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(mapped))
    }
}

/// 标准化 Ollama 的 `/api/chat` 响应
fn normalize_ollama_response(data: Value) -> Result<LlmResponse, AppError> {
    let message = &data["message"];
    let content = message["content"].as_str().unwrap_or("").to_string();

    // Ollama 的 tool_calls 在 message["tool_calls"] 中
    if let Some(tcs) = message["tool_calls"].as_array() {
        let mut calls = Vec::new();
        for tc in tcs {
            let function = &tc["function"];
            let args = function["arguments"].clone();
            calls.push(ToolCall {
                id: tc["id"].as_str().unwrap_or_default().to_string(),
                function: ToolCallFunction {
                    name: function["name"].as_str().unwrap_or_default().to_string(),
                    arguments: if args.is_null() {
                        serde_json::Value::Object(Default::default())
                    } else {
                        args
                    },
                },
            });
        }
        if !calls.is_empty() {
            return Ok(LlmResponse::ToolCalls(calls));
        }
    }

    Ok(LlmResponse::Text(content))
}