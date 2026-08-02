use std::env;
use std::path::Path;

use crate::agent::AgentConfig;
use crate::llm::{LlmConfig, ModelsConfig, ProviderConfig};
use crate::utils::error::AppError;

const MODELS_FILE: &str = ".dev-assistant-models.toml";

/// 模型配置文件查找目录：可执行文件所在目录（跟随安装位置）。
fn models_config_dir() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// 从指定 TOML 文件加载模型配置，并解析 `${VAR}` 环境变量占位符。
fn load_models_file(toml_path: &Path) -> Result<Vec<ProviderConfig>, AppError> {
    let content = std::fs::read_to_string(toml_path)
        .map_err(|e| AppError::Config(format!("读取 {} 失败: {}", toml_path.display(), e)))?;
    let mut models: ModelsConfig = toml::from_str(&content)
        .map_err(|e| AppError::Config(format!("解析 {} 失败: {}", toml_path.display(), e)))?;
    // 解析 ${VAR} 占位符
    for m in &mut models.models {
        m.resolve_env_vars();
    }
    if models.models.is_empty() {
        return Err(AppError::Config(format!(
            "{} 中未定义任何模型",
            toml_path.display()
        )));
    }
    Ok(models.models)
}

/// 加载所有模型配置。
///
/// 查找顺序：
/// 1. `--config` 显式指定的路径（若指定则必须存在）
/// 2. 可执行文件所在目录下的 `.dev-assistant-models.toml`
/// 3. 回退到环境变量加载单模型
pub fn load_models(explicit_path: Option<&Path>) -> Result<Vec<ProviderConfig>, AppError> {
    // 1. 显式 --config 路径
    if let Some(path) = explicit_path {
        return load_models_file(path);
    }

    // 2. 可执行文件所在目录的默认文件
    let toml_path = models_config_dir().join(MODELS_FILE);
    if toml_path.exists() {
        return load_models_file(&toml_path);
    }

    // 3. Fallback: 从环境变量加载单模型
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
        .unwrap_or(120);

    AgentConfig { max_iterations }
}
