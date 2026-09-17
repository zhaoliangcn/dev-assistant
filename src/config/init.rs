//! `dev-assistant init` 交互式配置向导。
//!
//! 通过终端问答收集 provider / API URL / API Key / 模型名，
//! 生成或追加到 `.dev-assistant-models.toml`。
//! 全部参数可通过 CLI 标志提供（脚本化场景），缺失项回退到交互提问。

use std::io::{self, BufRead, Write};

use crate::config::{ensure_models_template, models_config_path, MODELS_TEMPLATE};
use crate::llm::{ModelsConfig, ProviderConfig};
use crate::utils::error::AppError;

/// init 子命令参数（全部可选：缺失项交互式提问）。
#[derive(Debug, Default, Clone)]
pub struct InitArgs {
    pub provider: Option<String>,
    pub api_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub name: Option<String>,
}

/// 常见 provider 预设：名称 → 默认 API URL。
const PROVIDER_PRESETS: &[(&str, &str)] = &[
    ("openai", "https://api.openai.com/v1"),
    ("openai-compatible", "https://（兼容服务的 base url）"),
    ("anthropic", "https://api.anthropic.com"),
    ("ollama", "http://localhost:11434"),
];

fn prompt(reader: &mut impl BufRead, label: &str, default: Option<&str>) -> Result<String, AppError> {
    match default {
        Some(d) => print!("{} [{}]: ", label, d),
        None => print!("{}: ", label),
    }
    io::stdout().flush().map_err(|e| AppError::Io(e))?;
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(AppError::Io)?;
    let trimmed = line.trim().to_string();
    if trimmed.is_empty() {
        Ok(default.unwrap_or("").to_string())
    } else {
        Ok(trimmed)
    }
}

/// 从预设列表选择 provider（编号或直接输入）。
fn prompt_provider(reader: &mut impl BufRead) -> Result<(String, String), AppError> {
    println!("可用 provider 预设：");
    for (i, (name, url)) in PROVIDER_PRESETS.iter().enumerate() {
        println!("  {}. {} — 默认 URL: {}", i + 1, name, url);
    }
    let raw = prompt(reader, "选择 provider（编号或名称，回车=openai）", Some("openai"))?;
    let idx = raw.parse::<usize>().ok().and_then(|n| n.checked_sub(1));
    if let Some(i) = idx {
        if let Some((name, url)) = PROVIDER_PRESETS.get(i) {
            return Ok((name.to_string(), url.to_string()));
        }
    }
    // 直接输入名称：匹配预设取默认 URL，否则原样接受
    for (name, url) in PROVIDER_PRESETS {
        if *name == raw {
            return Ok((name.to_string(), url.to_string()));
        }
    }
    Ok((raw, String::new()))
}

/// 执行 init：写入配置文件，返回写入路径。
pub fn run_init(args: InitArgs) -> Result<std::path::PathBuf, AppError> {
    let toml_path = models_config_path(None);
    let mut reader = io::stdin().lock();

    let interactive = args.provider.is_none()
        && args.api_url.is_none()
        && args.api_key.is_none()
        && args.model.is_none();

    // ── 收集配置（交互或标志）──
    // provider：交互模式走预设选择；标志模式按名称匹配预设默认 URL
    let (provider, preset_url) = if interactive && args.provider.is_none() {
        prompt_provider(&mut reader)?
    } else {
        let p = args
            .provider
            .clone()
            .unwrap_or_else(|| "openai".to_string());
        let url = PROVIDER_PRESETS
            .iter()
            .find(|(n, _)| *n == p)
            .map(|(_, u)| u.to_string())
            .unwrap_or_default();
        (p, url)
    };
    let provider = provider.trim().to_string();

    let api_url = match args.api_url.clone().filter(|s| !s.trim().is_empty()) {
        Some(u) => u,
        None => prompt(&mut reader, "API URL", Some(&preset_url))?,
    };
    let api_url = api_url.trim().to_string();

    let api_key = match args.api_key.clone().filter(|s| !s.trim().is_empty()) {
        Some(k) => k,
        None => prompt(&mut reader, "API Key（ollama 可留空）", Some(""))?,
    };
    let api_key = api_key.trim().to_string();

    let model = match args.model.clone().filter(|s| !s.trim().is_empty()) {
        Some(m) => m,
        None => prompt(&mut reader, "模型名（如 gpt-4o-mini / claude-sonnet-4）", None)?,
    };
    let model = model.trim().to_string();

    if api_url.is_empty() || model.is_empty() {
        return Err(AppError::Config("API URL 与模型名不能为空".to_string()));
    }

    let name = args
        .name
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| model.clone());

    let cfg = ProviderConfig {
        name,
        provider: provider.clone(),
        api_url,
        api_key: if api_key.is_empty() { None } else { Some(api_key) },
        model,
        temperature: Some(0.6),
        max_output_tokens: None,
        reasoning_effort: None,
    };

    // ── 写入文件 ──
    if toml_path.exists() {
        // 已有配置：追加新模型（重名则覆盖更新）
        let content = std::fs::read_to_string(&toml_path).map_err(AppError::Io)?;
        let mut parsed: ModelsConfig = toml::from_str(&content)
            .map_err(|e| AppError::Config(format!("解析 {} 失败: {}", toml_path.display(), e)))?;
        if let Some(existing) = parsed.models.iter_mut().find(|m| m.name == cfg.name) {
            *existing = cfg.clone();
        } else {
            parsed.models.push(cfg.clone());
        }
        let out = toml::to_string(&parsed)
            .map_err(|e| AppError::Config(format!("序列化模型配置失败: {}", e)))?;
        std::fs::write(&toml_path, out).map_err(AppError::Io)?;
    } else {
        // 无配置：先写模板（保留注释），再以程序化方式追加该模型
        ensure_models_template(&toml_path)
            .map_err(|e| AppError::Io(e))?;
        let mut parsed: ModelsConfig = toml::from_str(MODELS_TEMPLATE)
            .map_err(|e| AppError::Config(format!("内置模板解析失败: {}", e)))?;
        parsed.models.clear();
        parsed.models.push(cfg.clone());
        let out = toml::to_string(&parsed)
            .map_err(|e| AppError::Config(format!("序列化模型配置失败: {}", e)))?;
        std::fs::write(&toml_path, out).map_err(AppError::Io)?;
    }

    println!("✅ 模型配置已写入: {}", toml_path.display());
    println!("   名称: {} | provider: {} | 模型: {}", cfg.name, cfg.provider, cfg.model);
    println!("   现在可以直接运行 dev-assistant 或 dev-assistant --web");
    Ok(toml_path)
}

/// 供测试：校验生成的 ProviderConfig 字段合法性。
pub fn validate(cfg: &ProviderConfig) -> Result<(), AppError> {
    if cfg.name.trim().is_empty() {
        return Err(AppError::Config("模型名称不能为空".to_string()));
    }
    if cfg.api_url.trim().is_empty() {
        return Err(AppError::Config("API URL 不能为空".to_string()));
    }
    if cfg.model.trim().is_empty() {
        return Err(AppError::Config("模型名不能为空".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_rejects_empty() {
        let cfg = ProviderConfig {
            name: "".into(),
            provider: "openai".into(),
            api_url: "http://x".into(),
            api_key: None,
            model: "m".into(),
            temperature: None,
            max_output_tokens: None,
            reasoning_effort: None,
        };
        assert!(validate(&cfg).is_err());
    }
}
