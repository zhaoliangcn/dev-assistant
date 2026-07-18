use std::env;
use std::path::Path;

use crate::agent::AgentConfig;
use crate::llm::{LlmConfig, ModelsConfig, ProviderConfig};
use crate::utils::error::AppError;

const MODELS_FILE: &str = ".dev-assistant-models.toml";

/// 加载所有模型配置。
/// 优先读取 TOML 文件，若不存在则 fallback 到环境变量。
pub fn load_models(project_dir: &Path) -> Result<Vec<ProviderConfig>, AppError> {
    // 尝试加载 TOML 文件
    let toml_path = project_dir.join(MODELS_FILE);
    if toml_path.exists() {
        let content = std::fs::read_to_string(&toml_path)
            .map_err(|e| AppError::Config(format!("读取 {} 失败: {}", MODELS_FILE, e)))?;
        let mut models: ModelsConfig = toml::from_str(&content)
            .map_err(|e| AppError::Config(format!("解析 {} 失败: {}", MODELS_FILE, e)))?;
        // 解析 ${VAR} 占位符
        for m in &mut models.models {
            m.resolve_env_vars();
        }
        if models.models.is_empty() {
            return Err(AppError::Config(format!(
                "{} 中未定义任何模型",
                MODELS_FILE
            )));
        }
        return Ok(models.models);
    }

    // Fallback: 从环境变量加载单模型
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

    Ok(vec![ProviderConfig {
        name: "default".to_string(),
        provider,
        api_url,
        api_key: Some(api_key),
        model,
        temperature: Some(temperature),
        max_tokens: Some(max_tokens),
    }])
}

/// 保留旧的单模型加载函数，供 main.rs 中 --provider / --model 等 CLI 覆盖使用
#[allow(dead_code)]
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
