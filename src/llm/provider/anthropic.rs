use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use tracing::debug;

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