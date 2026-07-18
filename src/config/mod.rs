use std::env;

use crate::agent::AgentConfig;
use crate::llm::LlmConfig;
use crate::utils::error::AppError;

pub fn load_llm_config() -> Result<LlmConfig, AppError> {
    let provider = env::var("LLM_PROVIDER").unwrap_or_else(|_| "openai".to_string());
    let api_url = env::var("LLM_API_URL").map_err(|_| AppError::Env("LLM_API_URL".to_string()))?;
    let api_key = env::var("LLM_API_KEY").map_err(|_| AppError::Env("LLM_API_KEY".to_string()))?;
    let model = env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());

    let temperature: f32 = env::var("LLM_TEMPERATURE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.2);

    let max_tokens: usize = env::var("LLM_MAX_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8192);

    Ok(LlmConfig {
        provider,
        api_url,
        api_key,
        model,
        temperature,
        max_tokens,
    })
}

pub fn load_agent_config() -> AgentConfig {
    let max_iterations: usize = env::var("MAX_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);

    AgentConfig { max_iterations }
}
