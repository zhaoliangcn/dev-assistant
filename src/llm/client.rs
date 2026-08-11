use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use std::pin::Pin;
use futures::Stream;

use rand::RngExt;
use reqwest::Client;
use tracing::{debug, info, warn};

use super::models::*;
use super::provider::{create_provider, LlmProvider};
use crate::utils::error::AppError;

/// 最大重试次数（连续 429 的自动重试上限）。
const MAX_RETRIES: u32 = 5;

/// 退避初始延迟（毫秒）。
const BASE_DELAY_MS: u64 = 2000;

/// 退避乘数（指数退避的底数）。
const BACKOFF_MULTIPLIER: f64 = 2.0;

/// 最大延迟上限（防止无限增长）。
const MAX_DELAY: Duration = Duration::from_secs(120);

/// 统一的指数退避 + 抖动重试循环。
///
/// 对 `429`（限流）和 `5xx`（服务端错误）自动重试，最多 `MAX_RETRIES` 次。
/// 优先使用服务端建议的 `Retry-After`，否则使用指数退避 + 随机抖动。
///
/// `call` 与 `call_streaming` 共享此逻辑，避免重试/退避代码重复。
async fn retry_with_backoff<T, F, Fut>(mut f: F) -> Result<T, AppError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, AppError>>,
{
    let mut attempt = 0u32;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(ref e)
                if (e.is_rate_limited() || e.is_server_error()) && attempt < MAX_RETRIES =>
            {
                attempt += 1;

                // 优先使用服务端建议的 Retry-After，否则用指数退避
                let delay = match e.retry_after() {
                    Some(server_delay) if server_delay <= MAX_DELAY => server_delay,
                    _ => {
                        let base = BASE_DELAY_MS as f64
                            * BACKOFF_MULTIPLIER.powi(attempt as i32 - 1);
                        let jitter_range = (base * 0.25) as u64;
                        let jitter = if jitter_range > 0 {
                            rand::rng().random_range(0..=jitter_range)
                        } else {
                            0
                        };
                        Duration::from_millis(base as u64 + jitter).min(MAX_DELAY)
                    }
                };

                let error_type = if e.is_rate_limited() { "429" } else { "5xx" };

                info!(
                    attempt = attempt,
                    max_retries = MAX_RETRIES,
                    delay_ms = delay.as_millis() as u64,
                    error_type = error_type,
                    "API 请求失败（{}），等待后重试",
                    error_type,
                );

                if attempt >= 3 {
                    warn!(
                        attempt = attempt,
                        max_retries = MAX_RETRIES,
                        delay_ms = delay.as_millis() as u64,
                        error_type = error_type,
                        "API 持续返回 {}，准备故障转移",
                        error_type,
                    );
                }

                tokio::time::sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// 多 provider 容器，支持运行时切换模型。
///
/// `active_idx` 使用 `AtomicUsize`，无锁且无中毒风险，
/// 使得 `&self` 即可切换模型，支持 `Arc<LlmClient>` 在多子 Agent 间安全共享。
pub struct LlmClient {
    http_client: Client,
    providers: Vec<Box<dyn LlmProvider>>,
    provider_configs: Vec<ProviderConfig>,
    active_idx: AtomicUsize,
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
            active_idx: AtomicUsize::new(0),
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
        self.active_idx.store(idx, Ordering::SeqCst);
        Ok(())
    }

    /// 当前活跃模型名称
    pub fn active_model(&self) -> &str {
        let idx = self.active_idx.load(Ordering::SeqCst);
        &self.provider_configs[idx].name
    }

    /// 列出所有可用模型名称
    pub fn list_models(&self) -> Vec<&str> {
        self.provider_configs.iter().map(|c| c.name.as_str()).collect()
    }

    /// 列出所有可用模型详情：(名称, provider, 是否当前激活)
    pub fn list_model_info(&self) -> Vec<(String, String, bool)> {
        let active_idx = self.active_idx.load(Ordering::SeqCst);
        self.provider_configs
            .iter()
            .enumerate()
            .map(|(i, c)| (c.name.clone(), c.provider.clone(), i == active_idx))
            .collect()
    }

    #[allow(dead_code)]
    pub async fn call(
        &self,
        messages: Vec<LlmMessage>,
        tools: Vec<ToolSchema>,
    ) -> Result<LlmResponse, AppError> {
        let start_idx = self.active_idx.load(Ordering::SeqCst);
        let total_providers = self.providers.len();
        let mut last_error: Option<AppError> = None;

        // 从当前活跃 provider 开始尝试，逐个故障转移
        for offset in 0..total_providers {
            let idx = (start_idx + offset) % total_providers;
            let cfg = &self.provider_configs[idx];
            let provider = &self.providers[idx];

            // 如果不是第一个尝试的 provider，记录故障转移日志
            if offset > 0 {
                warn!(
                    from_provider = %self.provider_configs[start_idx].name,
                    to_provider = %cfg.name,
                    "429/5xx 故障转移：切换到 {}",
                    cfg.name,
                );
                // 更新活跃索引以便后续调用使用新 provider
                self.active_idx.store(idx, Ordering::SeqCst);
            }

            debug!(model = %cfg.name, provider = %cfg.provider, "Calling LLM API");

            let request = LlmRequest {
                model: cfg.model.clone(),
                messages: messages.clone(),
                tools: Some(tools.clone()),
                temperature: cfg.temperature.unwrap_or(0.2),
                max_tokens: cfg.max_tokens.unwrap_or(262144),
            };

            match retry_with_backoff(|| provider.chat(&self.http_client, &request)).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    last_error = Some(e);
                }
            }

            // 如果这是最后一个 provider，不再继续尝试
            if offset == total_providers - 1 {
                break;
            }

            // retries exhausted on this provider, try the next one
            // 在两个 provider 之间增加短暂延迟，避免连续冲击下一个 API
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        // 所有 provider 均失败
        Err(last_error.unwrap_or_else(|| {
            AppError::Llm("所有 LLM provider 均不可用，请检查网络连接或 API 配置".to_string())
        }))
    }

    /// 调用 LLM API 并返回流式响应。
    ///
    /// 返回一个 Stream，每个事件包含一个 `LlmStreamEvent`。
    /// 支持多 provider 故障转移和重试逻辑。
    pub async fn call_streaming(
        &self,
        messages: Vec<LlmMessage>,
        tools: Vec<ToolSchema>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmStreamEvent, AppError>> + Send>>, AppError> {
        let start_idx = self.active_idx.load(Ordering::SeqCst);
        let total_providers = self.providers.len();
        let mut last_error: Option<AppError> = None;

        // 从当前活跃 provider 开始尝试，逐个故障转移
        for offset in 0..total_providers {
            let idx = (start_idx + offset) % total_providers;
            let cfg = &self.provider_configs[idx];
            let provider = &self.providers[idx];

            // 如果不是第一个尝试的 provider，记录故障转移日志
            if offset > 0 {
                warn!(
                    from_provider = %self.provider_configs[start_idx].name,
                    to_provider = %cfg.name,
                    "429/5xx 故障转移：切换到 {}",
                    cfg.name,
                );
                // 更新活跃索引以便后续调用使用新 provider
                self.active_idx.store(idx, Ordering::SeqCst);
            }

            debug!(model = %cfg.name, provider = %cfg.provider, "Calling LLM API (streaming)");

            let request = LlmRequest {
                model: cfg.model.clone(),
                messages: messages.clone(),
                tools: Some(tools.clone()),
                temperature: cfg.temperature.unwrap_or(0.2),
                max_tokens: cfg.max_tokens.unwrap_or(262144),
            };

            match retry_with_backoff(|| provider.chat_stream(&self.http_client, &request)).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    last_error = Some(e);
                }
            }

            // 如果这是最后一个 provider，不再继续尝试
            if offset == total_providers - 1 {
                break;
            }

            // retries exhausted on this provider, try the next one
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        // 所有 provider 均失败
        Err(last_error.unwrap_or_else(|| {
            AppError::Llm("所有 LLM provider 均不可用，请检查网络连接或 API 配置".to_string())
        }))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> LlmClient {
        LlmClient::from_configs(vec![
            ProviderConfig {
                name: "model-a".to_string(),
                provider: "openai".to_string(),
                api_url: "http://localhost:9999/v1".to_string(),
                api_key: Some("test-key".to_string()),
                model: "gpt-4o".to_string(),
                temperature: Some(0.0),
                max_tokens: Some(100),
            },
            ProviderConfig {
                name: "model-b".to_string(),
                provider: "anthropic".to_string(),
                api_url: "http://localhost:9998/v1".to_string(),
                api_key: Some("test-key-2".to_string()),
                model: "claude-3".to_string(),
                temperature: Some(0.5),
                max_tokens: Some(200),
            },
        ])
        .unwrap()
    }

    #[test]
    fn test_from_configs_empty_returns_error() {
        let result = LlmClient::from_configs(vec![]);
        assert!(result.is_err());
        match result {
            Err(AppError::Config(msg)) => assert!(msg.contains("No model providers")),
            _ => panic!("Expected Config error"),
        }
    }

    #[test]
    fn test_list_models_returns_all_names() {
        let client = test_client();
        let models = client.list_models();
        assert_eq!(models.len(), 2);
        assert!(models.contains(&"model-a"));
        assert!(models.contains(&"model-b"));
    }

    #[test]
    fn test_active_model_defaults_to_first() {
        let client = test_client();
        assert_eq!(client.active_model(), "model-a");
    }

    #[test]
    fn test_switch_model_changes_active() {
        let client = test_client();
        assert!(client.switch_model("model-b").is_ok());
        assert_eq!(client.active_model(), "model-b");
    }

    #[test]
    fn test_switch_model_unknown_returns_error() {
        let client = test_client();
        let result = client.switch_model("nonexistent");
        assert!(result.is_err());
        match result {
            Err(AppError::Config(msg)) => assert!(msg.contains("Unknown model")),
            _ => panic!("Expected Config error"),
        }
    }

    #[test]
    fn test_switch_model_round_trip() {
        let client = test_client();
        client.switch_model("model-b").unwrap();
        assert_eq!(client.active_model(), "model-b");
        client.switch_model("model-a").unwrap();
        assert_eq!(client.active_model(), "model-a");
    }

    #[test]
    fn test_call_returns_error_when_no_providers() {
        let client = LlmClient::from_configs(vec![ProviderConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            api_url: "http://localhost:1/v1".to_string(),
            api_key: Some("test-key".to_string()),
            model: "test-model".to_string(),
            temperature: Some(0.0),
            max_tokens: Some(100),
        }])
        .unwrap();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(client.call(
            vec![LlmMessage {
                role: "user".to_string(),
                content: Some("hello".to_string()),
                tool_calls: None,
                tool_call_id: None,
            }],
            vec![],
        ));
        // 连接被拒绝，应该是某种错误
        assert!(result.is_err());
    }
}