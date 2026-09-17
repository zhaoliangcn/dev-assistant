use std::env;
use std::path::Path;

use crate::agent::AgentConfig;
use crate::llm::{LlmConfig, ModelsConfig, ProviderConfig};
use crate::utils::error::AppError;

pub mod init;

const MODELS_FILE: &str = ".dev-assistant-models.toml";

/// 模型配置文件查找目录：可执行文件所在目录（跟随安装位置）。
fn models_config_dir() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// 计算模型配置文件的完整路径（与 `load_models` 的查找顺序一致，不读取文件）。
///
/// - `explicit_path`：`--config` 显式指定（若给出则必须存在）
/// - 否则：可执行文件所在目录下的 `.dev-assistant-models.toml`
///
/// Web 端持久化模型配置时使用同一路径，保证「读取与写入同源」。
pub fn models_config_path(explicit_path: Option<&Path>) -> std::path::PathBuf {
    match explicit_path {
        Some(p) => p.to_path_buf(),
        None => models_config_dir().join(MODELS_FILE),
    }
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

/// 首次运行时自动生成的模型配置模板（带完整注释，用户只需填 API Key）。
pub const MODELS_TEMPLATE: &str = r#"# ── Dev-Assistant 模型配置（首次运行自动生成）──
#
# 快速开始：把下方 api_key 换成你的真实密钥即可。
#
# 也可以不编辑本文件，改用以下任一方式：
#   1. 环境变量快速启动（单模型，无需本文件）：
#        LLM_API_URL=https://api.openai.com/v1 LLM_API_KEY=sk-xxx LLM_MODEL=gpt-4o-mini dev-assistant
#   2. 交互式向导生成配置：
#        dev-assistant init
#   3. Web 界面配置（零配置启动后浏览器里添加）：
#        dev-assistant --web   →   打开 http://127.0.0.1:8080 → 右上角 ⚙️
#
# ${VAR} 占位符会在启动时从环境变量读取（配合 .env 文件使用）。
#
# 支持的 provider 类型：
#   openai, openai-compatible  — OpenAI 及兼容服务（DeepSeek、Kimi、智谱、商汤等）
#   anthropic, claude          — Anthropic Claude
#   ollama                     — Ollama 本地模型

[[models]]
name = "default"
provider = "openai"
api_url = "https://api.openai.com/v1"
api_key = "sk-请替换为你的密钥"
model = "gpt-4o-mini"
temperature = 0.6
# max_output_tokens = 16384        # 单次响应输出上限；未设则由 provider 自决
# reasoning_effort = "low"         # 推理力度 low/medium/high/none；未设沿用服务端默认
"#;

/// 若模型配置文件不存在则生成注释模板，返回是否生成了模板。
///
/// 生成模板本身不视为错误——调用方决定后续行为：
/// CLI 模式提示后退出，Web 模式继续零配置启动（浏览器内配置）。
pub fn ensure_models_template(toml_path: &Path) -> std::io::Result<bool> {
    if toml_path.exists() {
        return Ok(false);
    }
    if let Some(parent) = toml_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(toml_path, MODELS_TEMPLATE)?;
    Ok(true)
}

/// 加载所有模型配置。
///
/// 查找顺序：
/// 1. `--config` 显式指定的路径（若指定则必须存在）
/// 2. 可执行文件所在目录下的 `.dev-assistant-models.toml`
/// 3. 回退到环境变量加载单模型（LLM_API_URL / LLM_API_KEY / LLM_MODEL）
/// 4. 均不可用时：自动生成配置模板并返回空列表（不报错）。
///    - CLI 模式：调用方（App::build）提示后退出
///    - Web 模式：调用方（web::serve）零配置启动，浏览器内配置
pub fn load_models(explicit_path: Option<&Path>) -> Result<Vec<ProviderConfig>, AppError> {
    // 1. 显式 --config 路径
    if let Some(path) = explicit_path {
        return load_models_file(path);
    }

    // 2. 可执行文件所在目录的默认文件
    let toml_path = models_config_path(None);
    if toml_path.exists() {
        return load_models_file(&toml_path);
    }

    // 3. Fallback: 环境变量齐全时加载单模型（不生成模板，避免噪音）
    let api_url = match env::var("LLM_API_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return ensure_zero_config(&toml_path),
    };
    let api_key = match env::var("LLM_API_KEY") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return ensure_zero_config(&toml_path),
    };
    let provider = env::var("LLM_PROVIDER").unwrap_or_else(|_| "openai".to_string());
    let model = env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());

    let temperature: f32 = env::var("LLM_TEMPERATURE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.2);

    // 输出上限：优先 LLM_MAX_OUTPUT_TOKENS，回退旧名 LLM_MAX_TOKENS；
    // 均未设 → None（不发送该字段，由 provider 用自身默认上限）。
    let max_output_tokens: Option<usize> = env::var("LLM_MAX_OUTPUT_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok())
        .or_else(|| env::var("LLM_MAX_TOKENS").ok().and_then(|v| v.parse().ok()));

    // 推理力度：LLM_REASONING_EFFORT（low/medium/high/none）；未设 → None（服务端默认）
    let reasoning_effort: Option<String> = env::var("LLM_REASONING_EFFORT")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());

    Ok(vec![ProviderConfig {
        name: "default".to_string(),
        provider,
        api_url,
        api_key: Some(api_key),
        model,
        temperature: Some(temperature),
        max_output_tokens,
        reasoning_effort,
    }])
}

/// 零配置兜底：生成配置模板后返回空列表（由调用方决定 CLI 提示退出或 Web 零配置启动）。
fn ensure_zero_config(toml_path: &Path) -> Result<Vec<ProviderConfig>, AppError> {
    match ensure_models_template(toml_path) {
        Ok(true) => Ok(Vec::new()),
        Ok(false) => Ok(Vec::new()),
        Err(e) => Err(AppError::Config(format!(
            "未检测到模型配置，且自动生成模板 {} 失败: {}（也可设置环境变量 LLM_API_URL / LLM_API_KEY 快速启动）",
            toml_path.display(),
            e
        ))),
    }
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

    // 输出上限：优先 LLM_MAX_OUTPUT_TOKENS，回退旧名 LLM_MAX_TOKENS；均未设用安全默认 8192。
    let max_output_tokens: usize = env::var("LLM_MAX_OUTPUT_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok())
        .or_else(|| env::var("LLM_MAX_TOKENS").ok().and_then(|v| v.parse().ok()))
        .unwrap_or(8192);

    Ok(LlmConfig {
        provider,
        api_url,
        api_key,
        model,
        temperature,
        max_output_tokens,
    })
}

pub fn load_agent_config() -> AgentConfig {
    let max_iterations: usize = env::var("MAX_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);

    AgentConfig { max_iterations }
}
