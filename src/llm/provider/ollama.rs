use async_trait::async_trait;
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use serde_json::Value;
use std::pin::Pin;
use tracing::debug;

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
            } else {
                AppError::Llm(msg)
            });
        }

        let stream = response.bytes_stream();
        let mapped = stream.map(|chunk_result| {
            match chunk_result {
                Ok(bytes) => {
                    // Ollama 流式响应是 NDJSON 格式，每行一个 JSON 对象
                    let text = String::from_utf8_lossy(&bytes);
                    let mut last_event: Option<Result<LlmStreamEvent, AppError>> = None;

                    for line in text.lines() {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<Value>(line) {
                            Ok(data) => {
                                if data["done"].as_bool().unwrap_or(false) {
                                    last_event = Some(Ok(LlmStreamEvent::Done));
                                } else if let Some(content) = data["message"]["content"].as_str() {
                                    if !content.is_empty() {
                                        last_event = Some(Ok(LlmStreamEvent::Chunk(content.to_string())));
                                    }
                                }
                            }
                            Err(e) => {
                                last_event = Some(Err(AppError::Llm(
                                    format!("Failed to parse Ollama streaming response: {}", e)
                                )));
                            }
                        }
                    }

                    // 如果没有产生任何事件（例如 JSON 被分片），返回空
                    last_event.unwrap_or(Ok(LlmStreamEvent::Chunk(String::new())))
                }
                Err(e) => Err(AppError::Llm(format!("Ollama stream error: {}", e))),
            }
        });

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