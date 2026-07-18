use std::time::Duration;

use reqwest::Client;
use serde_json::Value;
use tracing::{debug, warn};

use super::models::*;
use crate::utils::error::AppError;

pub struct LlmClient {
    client: Client,
    config: LlmConfig,
}

impl LlmClient {
    pub fn new(config: LlmConfig) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("reqwest client builder should not fail with valid config"),
            config,
        }
    }

    pub async fn call(
        &self,
        messages: Vec<LlmMessage>,
        tools: Vec<ToolSchema>,
    ) -> Result<LlmResponse, AppError> {
        debug!(model = %self.config.model, "Calling LLM API");
        let request = LlmRequest {
            model: self.config.model.clone(),
            messages,
            tools: Some(tools),
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
        };

        let response = self
            .client
            .post(&self.config.api_url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "LLM API returned error");
            return Err(AppError::Llm(format!(
                "LLM API returned error (status {}): {}",
                status, body
            )));
        }

        let data: Value = response.json().await?;
        self.normalize_response(data)
    }

    fn normalize_response(&self, data: Value) -> Result<LlmResponse, AppError> {
        self.normalize_chat_response(data)
    }

    fn parse_arguments(&self, args: Value) -> Result<Value, AppError> {
        if let Some(s) = args.as_str() {
            if let Ok(parsed) = serde_json::from_str(s) {
                return Ok(parsed);
            }
            return Err(AppError::Llm(format!(
                "Failed to parse tool arguments as JSON: {}",
                s
            )));
        }
        Ok(args)
    }

    /// Shared normalization for OpenAI-compatible chat completion responses.
    /// Works for both OpenAI and Ollama since both use the same format.
    fn normalize_chat_response(&self, data: Value) -> Result<LlmResponse, AppError> {
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
                let args = self.parse_arguments(tc["function"]["arguments"].clone())?;
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
}
