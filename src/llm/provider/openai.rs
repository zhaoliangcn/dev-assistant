use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use tracing::debug;
use tracing::warn;

use super::super::models::*;
use super::LlmProvider;
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
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    async fn chat(
        &self,
        http_client: &Client,
        request: &LlmRequest,
    ) -> Result<LlmResponse, AppError> {
        let api_url = self.config.api_url.trim_end_matches('/').to_string();
        let url = if api_url.ends_with("/chat/completions") {
            api_url
        } else {
            format!("{}/chat/completions", api_url)
        };

        // 构建 OpenAI 格式的请求体
        let mut body = serde_json::json!({
            "model": request.model,
            "messages": request.messages,
            "temperature": request.temperature,
            "max_tokens": request.max_tokens,
        });
        if let Some(tools) = &request.tools {
            body["tools"] = serde_json::to_value(tools).unwrap_or(Value::Null);
        }

        debug!(url = %url, model = %request.model, "OpenAI chat request");

        let mut req = http_client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body);

        if let Some(ref key) = self.config.api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }

        let response = req.send().await?;
        let status = response.status();

        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(AppError::Llm(format!(
                "LLM API returned error (status {}): {}",
                status, body_text
            )));
        }

        let data: Value = response.json().await?;
        normalize_chat_response(data)
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

fn parse_arguments(args: Value) -> Result<Value, AppError> {
    if let Some(s) = args.as_str() {
        if let Ok(parsed) = serde_json::from_str(s) {
            return Ok(parsed);
        }
        // 如果解析失败（如含未转义换行符），返回原始字符串作为 Value，
        // 避免整个响应解析失败，让工具执行器处理
        warn!(len = s.len(), "Failed to parse tool arguments as JSON, falling back to raw string");
        Ok(serde_json::Value::String(s.to_string()))
    } else {
        Ok(args)
    }
}