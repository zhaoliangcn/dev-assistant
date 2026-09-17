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
        });
        // temperature 为 None 时省略 options，避免 json! 序列化为 null 被部分 Ollama 版本拒绝
        // LlmRequest.temperature 为 f64（非 Option），始终写入 options.temperature
        body["options"] = serde_json::json!({ "temperature": request.temperature });

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

            // 按 index 累积工具调用增量：Ollama 会把单个 tool call 的 arguments
            // 分片送到多个 done:false chunk，需在 done:true 时合并后一次性发出。
            struct AccToolCall {
                id: String,
                name: String,
                arguments: String,
            }
            let mut acc_tool_calls: Vec<(usize, AccToolCall)> = Vec::new();

            macro_rules! flush_tool_calls {
                () => {{
                    if !acc_tool_calls.is_empty() {
                        acc_tool_calls.sort_by_key(|(idx, _)| *idx);
                        for (_idx, acc) in acc_tool_calls.drain(..) {
                            if acc.name.is_empty() {
                                continue;
                            }
                            let arguments = match serde_json::from_str::<Value>(&acc.arguments) {
                                Ok(v) => v,
                                Err(_) => Value::Object(Default::default()),
                            };
                            // Ollama 的 tool_calls 不带 id，缺失时生成 UUID 保证下游唯一性
                            let id = if acc.id.is_empty() {
                                uuid::Uuid::new_v4().to_string()
                            } else {
                                acc.id.clone()
                            };
                            yield LlmStreamEvent::ToolCallDelta(ToolCall {
                                id,
                                function: ToolCallFunction {
                                    name: acc.name,
                                    arguments,
                                },
                            });
                        }
                    }
                }};
            }

            let mut done_emitted = false;
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
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

                        // done=true 分支：先 flush 尾段文本，再合并并 flush 工具调用，最后 Done
                        if data["done"].as_bool().unwrap_or(false) {
                            if let Some(content) = data["message"]["content"].as_str() {
                                if !content.is_empty() {
                                    yield LlmStreamEvent::Chunk(content.to_string());
                                }
                            }

                            // 提取 usage 信息（Ollama 在最后 done=true 的行中返回）
                            if let Some(prompt_count) = data.get("prompt_eval_count").and_then(|v| v.as_u64()) {
                                let eval_count = data.get("eval_count").and_then(|v| v.as_u64()).unwrap_or(0);
                                yield LlmStreamEvent::Usage(TokenUsage {
                                    prompt_tokens: prompt_count as usize,
                                    completion_tokens: eval_count as usize,
                                    total_tokens: (prompt_count + eval_count) as usize,
                                });
                            }

                            // 合并 done=true 行上的 tool_calls（可能携带完整或尾段 arguments）
                            if let Some(tcs) = data["message"]["tool_calls"].as_array() {
                                for tc in tcs {
                                    let index = tc["index"].as_i64().unwrap_or(0) as usize;
                                    let pos = acc_tool_calls.iter().position(|(i, _)| *i == index);
                                    if let Some(pos) = pos {
                                        let acc = &mut acc_tool_calls[pos].1;
                                        if let Some(id) = tc["id"].as_str() {
                                            if !id.is_empty() {
                                                acc.id = id.to_string();
                                            }
                                        }
                                        if let Some(func) = tc.get("function") {
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
                                        let id = tc["id"].as_str().unwrap_or_default().to_string();
                                        let func_obj = tc.get("function").cloned().unwrap_or(Value::Object(Default::default()));
                                        let name = func_obj.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                                        let arguments = func_obj.get("arguments").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                                        acc_tool_calls.push((index, AccToolCall { id, name, arguments }));
                                    }
                                }
                            }

                            flush_tool_calls!();
                            yield LlmStreamEvent::Done;
                            break;
                        }

                        // 处理文本内容
                        if let Some(content) = data["message"]["content"].as_str() {
                            if !content.is_empty() {
                                yield LlmStreamEvent::Chunk(content.to_string());
                            }
                        }

                        // 处理工具调用增量：按 index 累积，不在中间 yield
                        if let Some(tcs) = data["message"]["tool_calls"].as_array() {
                            for tc in tcs {
                                let index = tc["index"].as_i64().unwrap_or(0) as usize;
                                let pos = acc_tool_calls.iter().position(|(i, _)| *i == index);
                                if let Some(pos) = pos {
                                    let acc = &mut acc_tool_calls[pos].1;
                                    if let Some(id) = tc["id"].as_str() {
                                        if !id.is_empty() {
                                            acc.id = id.to_string();
                                        }
                                    }
                                    if let Some(func) = tc.get("function") {
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
                                    let id = tc["id"].as_str().unwrap_or_default().to_string();
                                    let func_obj = tc.get("function").cloned().unwrap_or(Value::Object(Default::default()));
                                    let name = func_obj.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                                    let arguments = func_obj.get("arguments").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                                    acc_tool_calls.push((index, AccToolCall { id, name, arguments }));
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        // EOF 未收到 done=true：flush 残余工具调用并发 Done，避免调用方挂起
                        if !done_emitted {
                            flush_tool_calls!();
                            yield LlmStreamEvent::Done;
                        }
                        break;
                    }
                    Err(e) => {
                        // 连接中断：flush 残余工具调用后传播错误
                        if !done_emitted {
                            flush_tool_calls!();
                        }
                        yield Err(AppError::Llm(format!("Ollama stream read error: {}", e)))?;
                        break;
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