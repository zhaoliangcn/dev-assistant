use std::sync::Mutex;
use std::time::Duration;

use rand::RngExt;
use reqwest::Client;
use tracing::{debug, warn};

use super::models::*;
use super::provider::{create_provider, LlmProvider};
use crate::utils::error::AppError;

const MAX_RETRIES: u32 = 5;
const BASE_DELAY_MS: u64 = 1000;

/// 多 provider 容器，支持运行时切换模型。
///
/// `active_idx` 使用 `Mutex` 内部可变性，使得 `&self` 即可切换模型。
/// 这允许通过 `Arc<LlmClient>` 在多个子 Agent 间共享同一个 LLM 客户端。
pub struct LlmClient {
    http_client: Client,
    providers: Vec<Box<dyn LlmProvider>>,
    provider_configs: Vec<ProviderConfig>,
    active_idx: Mutex<usize>,
}

impl LlmClient {
    /// 从 ProviderConfig 列表构建
    pub fn from_configs(configs: Vec<ProviderConfig>) -> Result<Self, AppError> {
        if configs.is_empty() {
            return Err(AppError::Config("No model providers configured".to_string()));
        }

        let http_client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| AppError::Config(format!("Failed to create HTTP client: {}", e)))?;

        let mut providers: Vec<Box<dyn LlmProvider>> = Vec::new();
        let mut provider_configs: Vec<ProviderConfig> = Vec::new();

        for cfg in &configs {
            let provider = create_provider(cfg)?;
            provider_configs.push(cfg.clone());
            providers.push(provider);
        }

        Ok(Self {
            http_client,
            providers,
            provider_configs,
            active_idx: Mutex::new(0),
        })
    }

    /// 兼容旧的单模型构造方式（从环境变量）
    #[allow(dead_code)]
    pub fn new(config: &LlmConfig) -> Result<Self, AppError> {
        let provider_config = ProviderConfig {
            name: "default".to_string(),
            provider: config.provider.clone(),
            api_url: config.api_url.clone(),
            api_key: Some(config.api_key.clone()),
            model: config.model.clone(),
            temperature: Some(config.temperature),
            max_tokens: Some(config.max_tokens),
        };
        Self::from_configs(vec![provider_config])
    }

    /// 切换到指定名称的模型
    pub fn switch_model(&self, name: &str) -> Result<(), AppError> {
        let idx = self
            .provider_configs
            .iter()
            .position(|c| c.name == name)
            .ok_or_else(|| AppError::Config(format!("Unknown model: '{}'", name)))?;
        *self.active_idx.lock().unwrap() = idx;
        Ok(())
    }

    /// 当前活跃模型名称
    pub fn active_model(&self) -> &str {
        let idx = *self.active_idx.lock().unwrap();
        &self.provider_configs[idx].name
    }

    /// 列出所有可用模型名称
    pub fn list_models(&self) -> Vec<&str> {
        self.provider_configs.iter().map(|c| c.name.as_str()).collect()
    }

    pub async fn call(
        &self,
        messages: Vec<LlmMessage>,
        tools: Vec<ToolSchema>,
    ) -> Result<LlmResponse, AppError> {
        let active_idx = *self.active_idx.lock().unwrap();
        let cfg = &self.provider_configs[active_idx];
        let provider = &self.providers[active_idx];

        debug!(model = %cfg.name, provider = %cfg.provider, "Calling LLM API");

        let request = LlmRequest {
            model: cfg.model.clone(),
            messages,
            tools: Some(tools),
            temperature: cfg.temperature.unwrap_or(0.2),
            max_tokens: cfg.max_tokens.unwrap_or(8192),
        };

        let mut attempt = 0u32;
        loop {
            match provider.chat(&self.http_client, &request).await {
                Ok(resp) => return Ok(resp),
                Err(ref e) if e.is_rate_limited() && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let delay_ms = BASE_DELAY_MS * 2u64.pow(attempt - 1)
                        + rand::rng().random_range(0..500);
                    warn!(
                        attempt = attempt,
                        max_retries = MAX_RETRIES,
                        delay_ms = delay_ms,
                        "429 rate limit hit, retrying after backoff"
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

// 保留旧接口兼容性，但标记为 deprecated
#[deprecated(since = "0.2.0", note = "use from_configs instead")]
impl LlmClient {
    #[allow(deprecated, dead_code)]
    pub fn legacy_new(config: LlmConfig) -> Self {
        Self::from_configs(vec![ProviderConfig {
            name: "default".to_string(),
            provider: config.provider,
            api_url: config.api_url,
            api_key: Some(config.api_key),
            model: config.model,
            temperature: Some(config.temperature),
            max_tokens: Some(config.max_tokens),
        }])
        .expect("Failed to create LlmClient from legacy config")
    }
}