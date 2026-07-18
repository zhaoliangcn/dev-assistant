use async_trait::async_trait;
use reqwest::Client;

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

mod openai;
mod anthropic;
mod ollama;

pub use anthropic::AnthropicProvider;
pub use ollama::OllamaProvider;
pub use openai::OpenAIProvider;