use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
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
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn chat(
        &self,
        http_client: &Client,
        request: &LlmRequest,
    ) -> Result<LlmResponse, AppError> {
        let api_url = self.config.api_url.trim_end_matches('/').to_string();
        let url = if api_url.ends_with("/api/chat") {
            api_url
        } else {
            format!("{}/api/chat", api_url)
        };

        // 转换 messages 为 Ollama 格式
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
            "stream": false,
            "options": {
                "temperature": request.temperature,
            },
        });

        // Ollama 也支持 tools（较新版本）
        if let Some(tools) = &request.tools {
            body["tools"] = serde_json::to_value(tools).unwrap_or(Value::Null);
        }

        debug!(url = %url, model = %request.model, "Ollama chat request");

        let response = http_client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(AppError::Llm(format!(
                "Ollama API returned error (status {}): {}",
                status, body_text
            )));
        }

        let data: Value = response.json().await?;
        normalize_ollama_response(data)
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