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
            let msg = format!("LLM API returned error (status {}): {}", status, body_text);
            return Err(if status.as_u16() == 429 {
                AppError::RateLimited(msg)
            } else {
                AppError::Llm(msg)
            });
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
        if let Ok(parsed) = try_parse_json_args(s) {
            return Ok(parsed);
        }
        // 尽力修复后仍无法解析，返回原始字符串作为 Value，
        // 避免整个响应解析失败，让工具执行器有机会给出可读错误。
        let preview: String = s.chars().take(80).collect();
        warn!(
            len = s.len(),
            preview = %preview,
            "Failed to parse tool arguments as JSON, falling back to raw string"
        );
        Ok(serde_json::Value::String(s.to_string()))
    } else {
        Ok(args)
    }
}

/// 尝试以多种容错策略解析 LLM 生成的工具参数 JSON。
///
/// 常见模型错误：
/// - 字符串值中包含未转义的换行符
/// - 外层包裹 markdown code fence
/// - 首尾空白
fn try_parse_json_args(raw: &str) -> Result<Value, serde_json::Error> {
    // 1. 原样解析
    if let Ok(v) = serde_json::from_str(raw) {
        return Ok(v);
    }

    // 2. 去除首尾空白和 markdown fence
    let trimmed = raw.trim();
    let without_fence = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|s| s.strip_suffix("```"))
        .unwrap_or(trimmed)
        .trim();
    if let Ok(v) = serde_json::from_str(without_fence) {
        return Ok(v);
    }

    // 3. 转义字符串值内部的实际换行符（LLM 最容易犯的 JSON 错误）
    let escaped = escape_newlines_in_json(without_fence);
    if let Ok(v) = serde_json::from_str(&escaped) {
        return Ok(v);
    }

    // 4. 移除 trailing comma
    let no_trailing_comma = without_fence.replace(",}", "}").replace(",]", "]");
    if let Ok(v) = serde_json::from_str(&no_trailing_comma) {
        return Ok(v);
    }

    serde_json::from_str(raw)
}

/// 在 JSON 字符串字面量内部转义未转义的换行符。
///
/// 使用轻量级状态机：遇到 `"` 切换 in_string 状态；
/// 在字符串内部将实际 `\r\n`/`\n`/`\r` 替换为 `\\n`。
fn escape_newlines_in_json(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_string = false;
    let mut escape = false;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if escape {
            result.push(c);
            escape = false;
            continue;
        }
        if c == '\\' {
            result.push(c);
            escape = true;
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            result.push(c);
            continue;
        }
        if in_string && (c == '\n' || c == '\r') {
            // 统一转换成 \\n
            result.push_str("\\n");
            // 跳过 \\r 后的 \\n，避免生成 \\n\\n
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            continue;
        }
        result.push(c);
    }

    result
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

    #[test]
    fn escape_newlines_in_json_only_escapes_inside_strings() {
        let input = "{\n  \"a\": \"b\nc\",\n  \"d\": 1\n}";
        let escaped = escape_newlines_in_json(input);
        // 字符串外部的换行保持原样，字符串内部的换行被转义
        assert_eq!(escaped, "{\n  \"a\": \"b\\nc\",\n  \"d\": 1\n}");
    }
}