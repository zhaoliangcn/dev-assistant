use async_trait::async_trait;
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use std::pin::Pin;

use super::models::*;
use crate::utils::error::AppError;

// ---------------------------------------------------------------------------
// LlmProvider trait — 每个模型 provider 实现此 trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// 发起一次 chat 请求，返回标准化后的 LlmResponse
    async fn chat(
        &self,
        http_client: &Client,
        request: &LlmRequest,
    ) -> Result<LlmResponse, AppError>;

    /// 发起流式 chat 请求，返回一个流式事件序列。
    ///
    /// 默认实现 fallback 到非流式 `chat()`，包装为单元素流。
    /// 各 provider 可以覆盖此方法以实现真正的流式支持。
    async fn chat_stream(
        &self,
        http_client: &Client,
        request: &LlmRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmStreamEvent, AppError>> + Send>>, AppError>
    {
        // fallback: 调用非流式接口，包装为流
        let response = self.chat(http_client, request).await?;
        match response {
            LlmResponse::Text(content) => {
                let stream = futures::stream::once(async { Ok(LlmStreamEvent::Chunk(content)) })
                    .chain(futures::stream::once(async { Ok(LlmStreamEvent::Done) }));
                Ok(Box::pin(stream))
            }
            LlmResponse::ToolCalls(tcs) => {
                let tool_events: Vec<Result<LlmStreamEvent, AppError>> = tcs.into_iter()
                    .map(|tc| Ok(LlmStreamEvent::ToolCallDelta(tc)))
                    .collect();
                let stream = futures::stream::iter(tool_events)
                    .chain(futures::stream::once(async { Ok(LlmStreamEvent::Done) }));
                Ok(Box::pin(stream))
            }
            LlmResponse::Error(err) => Err(AppError::Llm(err)),
        }
    }
}

// ---------------------------------------------------------------------------
// 工厂函数
// ---------------------------------------------------------------------------

pub fn create_provider(config: &ProviderConfig) -> Result<Box<dyn LlmProvider>, AppError> {
    match config.provider.to_lowercase().as_str() {
        "openai" | "openai-compatible" | "shangtang" | "deepseek" | "moonshot" | "zhipu" | "baidu" | "aliyun" | "siliconflow" => Ok(Box::new(OpenAIProvider::new(config)?)),
        "anthropic" | "claude" => Ok(Box::new(AnthropicProvider::new(config)?)),
        "ollama" => Ok(Box::new(OllamaProvider::new(config)?)),
        _ => Err(AppError::Config(format!(
            "Unknown provider '{}'. Supported: openai, openai-compatible, anthropic, ollama, and common Chinese providers (shangtang, deepseek, moonshot, zhipu, baidu, aliyun, siliconflow)",
            config.provider
        ))),
    }
}

mod common;
mod openai;
mod anthropic;
mod ollama;

pub use anthropic::AnthropicProvider;
pub use ollama::OllamaProvider;
pub use openai::OpenAIProvider;
pub(crate) use common::{parse_arguments, try_parse_json_args};