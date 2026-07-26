//! 工具重试机制
//! 
//! 参考: grok-build 的重试配置设计

use std::time::Duration;

/// 退避配置
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackoffConfig {
    pub max_attempts: usize,
    pub initial_delay: Duration,
    pub multiplier: f64,
    pub max_delay: Duration,
}

impl BackoffConfig {
    #[allow(dead_code)] // reserved for future backoff configuration
    pub fn new(max_attempts: usize, initial_delay: Duration, multiplier: f64, max_delay: Duration) -> Self {
        Self {
            max_attempts,
            initial_delay,
            multiplier,
            max_delay,
        }
    }
    
    pub fn delay_for(&self, attempt: usize) -> Duration {
        if attempt == 0 {
            self.initial_delay
        } else {
            let delay = self.initial_delay.as_secs_f64() * (self.multiplier.powi(attempt as i32));
            Duration::from_secs_f64(delay.min(self.max_delay.as_secs_f64()))
        }
    }
    
    /// 异步重试函数
    pub async fn retry<F, T, E>(&self, mut f: F) -> Result<T, E>
    where
        F: FnMut() -> Result<T, E>,
        E: std::fmt::Display,
    {
        for attempt in 0..self.max_attempts {
            match f() {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if attempt == self.max_attempts - 1 {
                        return Err(e);
                    }
                    let delay = self.delay_for(attempt);
                    tracing::debug!(attempt, delay_ms = delay.as_millis(), error = %e, "Retrying after error");
                    tokio::time::sleep(delay).await;
                }
            }
        }
        unreachable!("max_attempts should prevent reaching here")
    }
    
    /// 带自定义重试条件的异步重试函数
    #[allow(dead_code)] // reserved for future async retry with condition
    pub async fn retry_with<F, T, E>(&self, mut f: F, should_retry: impl Fn(&E) -> bool) -> Result<T, E>
    where
        F: FnMut() -> Result<T, E>,
        E: std::fmt::Display,
    {
        for attempt in 0..self.max_attempts {
            match f() {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if attempt == self.max_attempts - 1 || !should_retry(&e) {
                        return Err(e);
                    }
                    let delay = self.delay_for(attempt);
                    tracing::debug!(attempt, delay_ms = delay.as_millis(), error = %e, "Retrying after error");
                    tokio::time::sleep(delay).await;
                }
            }
        }
        unreachable!("max_attempts should prevent reaching here")
    }
    
    /// 同步重试函数（用于同步工具执行）
    pub fn retry_sync<F, T, E>(&self, mut f: F) -> Result<T, E>
    where
        F: FnMut() -> Result<T, E>,
        E: std::fmt::Display,
    {
        for attempt in 0..self.max_attempts {
            match f() {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if attempt == self.max_attempts - 1 {
                        return Err(e);
                    }
                    let delay = self.delay_for(attempt);
                    tracing::debug!(attempt, delay_ms = delay.as_millis(), error = %e, "Retrying after error");
                    std::thread::sleep(delay);
                }
            }
        }
        unreachable!("max_attempts should prevent reaching here")
    }
    
    /// 带可重试判断的同步重试函数
    pub fn retry_sync_with_condition<F, T, E>(&self, mut f: F, should_retry: impl Fn(&E) -> bool) -> Result<T, E>
    where
        F: FnMut() -> Result<T, E>,
        E: std::fmt::Display,
    {
        for attempt in 0..self.max_attempts {
            match f() {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if attempt == self.max_attempts - 1 || !should_retry(&e) {
                        return Err(e);
                    }
                    let delay = self.delay_for(attempt);
                    tracing::debug!(attempt, delay_ms = delay.as_millis(), error = %e, "Retrying after error");
                    std::thread::sleep(delay);
                }
            }
        }
        unreachable!("max_attempts should prevent reaching here")
    }
}

/// 默认退避配置
impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay: Duration::from_millis(100),
            multiplier: 2.0,
            max_delay: Duration::from_secs(10),
        }
    }
}

/// 重试管理器
#[derive(Debug, Clone)]
pub struct RetryManager {
    default_backoff: BackoffConfig,
    per_tool_backoff: std::collections::HashMap<String, BackoffConfig>,
}

impl RetryManager {
    pub fn new(default_backoff: BackoffConfig) -> Self {
        Self {
            default_backoff,
            per_tool_backoff: std::collections::HashMap::new(),
        }
    }
    
    #[allow(dead_code)] // reserved for future per-tool retry configuration
    pub fn with_tool_backoff(mut self, tool_name: &str, backoff: BackoffConfig) -> Self {
        self.per_tool_backoff.insert(tool_name.to_string(), backoff);
        self
    }

    pub fn get_backoff(&self, tool_name: &str) -> &BackoffConfig {
        self.per_tool_backoff.get(tool_name).unwrap_or(&self.default_backoff)
    }

    #[allow(dead_code)] // reserved for future async retry support
    pub async fn execute_with_retry<T, E>(&self, tool_name: &str, f: impl FnMut() -> Result<T, E>) -> Result<T, E>
    where
        E: std::fmt::Display,
    {
        let backoff = self.get_backoff(tool_name);
        backoff.retry(f).await
    }

    #[allow(dead_code)] // reserved for future sync retry support
    pub fn execute_with_retry_sync<T, E>(&self, tool_name: &str, f: impl FnMut() -> Result<T, E>) -> Result<T, E>
    where
        E: std::fmt::Display,
    {
        let backoff = self.get_backoff(tool_name);
        backoff.retry_sync(f)
    }
    
    /// 带条件判断的同步重试执行
    pub fn execute_with_retry_sync_condition<T, E>(
        &self, 
        tool_name: &str, 
        f: impl FnMut() -> Result<T, E>,
        should_retry: impl Fn(&E) -> bool,
    ) -> Result<T, E>
    where
        E: std::fmt::Display,
    {
        let backoff = self.get_backoff(tool_name);
        backoff.retry_sync_with_condition(f, should_retry)
    }
}

impl Default for RetryManager {
    fn default() -> Self {
        Self::new(BackoffConfig::default())
    }
}

/// 可重试错误 trait
#[allow(dead_code)] // reserved for future retry integration
pub trait RetryableError {
    fn is_retryable(&self) -> bool;
}

/// 网络错误可重试实现示例
#[allow(dead_code)]
impl RetryableError for reqwest::Error {
    fn is_retryable(&self) -> bool {
        self.is_timeout() || self.is_connect() || self.is_request()
    }
}

/// IO 错误可重试实现示例
#[allow(dead_code)]
impl RetryableError for std::io::Error {
    fn is_retryable(&self) -> bool {
        matches!(self.kind(),
            std::io::ErrorKind::Interrupted |
            std::io::ErrorKind::TimedOut |
            std::io::ErrorKind::WouldBlock
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    
    #[tokio::test]
    async fn retry_succeeds_after_failure() {
        let backoff = BackoffConfig {
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            multiplier: 1.0,
            max_delay: Duration::from_millis(10),
        };
        
        let counter = AtomicUsize::new(0);
        let result = backoff.retry(|| {
            let count = counter.fetch_add(1, Ordering::SeqCst);
            if count < 2 {
                Err(format!("failed attempt {}", count))
            } else {
                Ok("success")
            }
        }).await;
        
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }
    
    #[tokio::test]
    async fn retry_fails_after_max_attempts() {
        let backoff = BackoffConfig {
            max_attempts: 2,
            initial_delay: Duration::from_millis(1),
            multiplier: 1.0,
            max_delay: Duration::from_millis(10),
        };
        
        let counter = AtomicUsize::new(0);
        let result: Result<&str, String> = backoff.retry(|| {
            counter.fetch_add(1, Ordering::SeqCst);
            Err("always fails".to_string())
        }).await;
        
        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
    
    #[tokio::test]
    async fn retry_with_custom_condition() {
        let backoff = BackoffConfig {
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            multiplier: 1.0,
            max_delay: Duration::from_millis(10),
        };
        
        let counter = AtomicUsize::new(0);
        let result: Result<&str, String> = backoff.retry_with(|| {
            let count = counter.fetch_add(1, Ordering::SeqCst);
            if count == 0 {
                Err("retryable".to_string())
            } else {
                Err("fatal".to_string())
            }
        }, |e| e == "retryable").await;
        
        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
    
    #[test]
    fn backoff_delay_calculation() {
        let backoff = BackoffConfig {
            max_attempts: 5,
            initial_delay: Duration::from_millis(100),
            multiplier: 2.0,
            max_delay: Duration::from_secs(1),
        };
        
        assert_eq!(backoff.delay_for(0), Duration::from_millis(100));
        assert_eq!(backoff.delay_for(1), Duration::from_millis(200));
        assert_eq!(backoff.delay_for(2), Duration::from_millis(400));
        assert_eq!(backoff.delay_for(3), Duration::from_millis(800));
        assert_eq!(backoff.delay_for(4), Duration::from_secs(1)); // capped at max_delay
    }
    
    #[test]
    fn retry_manager_uses_default_backoff() {
        let manager = RetryManager::default();
        let backoff = manager.get_backoff("unknown_tool");
        
        assert_eq!(backoff.max_attempts, 3);
    }
    
    #[test]
    fn retry_manager_uses_per_tool_backoff() {
        let manager = RetryManager::default()
            .with_tool_backoff("special_tool", BackoffConfig {
                max_attempts: 5,
                ..BackoffConfig::default()
            });
        
        assert_eq!(manager.get_backoff("special_tool").max_attempts, 5);
        assert_eq!(manager.get_backoff("other_tool").max_attempts, 3);
    }
}
