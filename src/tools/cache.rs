use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::debug;

type Timestamp = u64;

fn now_timestamp() -> Timestamp {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 文件缓存条目
#[derive(Debug, Clone)]
struct CacheEntry {
    content: String,
    mtime: Timestamp,
    accessed_at: Timestamp,
    size: usize,
}

impl CacheEntry {
    fn new(content: String, mtime: Timestamp, size: usize) -> Self {
        Self {
            content,
            mtime,
            accessed_at: now_timestamp(),
            size,
        }
    }

    fn touch(&mut self) {
        self.accessed_at = now_timestamp();
    }
}

/// 文件读取缓存配置
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// 最大缓存条目数
    pub max_entries: usize,
    /// 单个文件最大大小（字节），超过此大小不缓存
    pub max_file_size: usize,
    /// 缓存过期时间（秒）
    pub ttl_seconds: u64,
    /// 是否启用缓存
    pub enabled: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 1000,
            max_file_size: 1_048_576, // 1MB
            ttl_seconds: 300,         // 5分钟
            enabled: true,
        }
    }
}

/// 文件读取缓存（基于 mtime 失效）
#[derive(Debug, Clone)]
pub struct ReadCache {
    cache: Arc<RwLock<HashMap<PathBuf, CacheEntry>>>,
    config: CacheConfig,
    hits: Arc<RwLock<usize>>,
    misses: Arc<RwLock<usize>>,
}

impl ReadCache {
    pub fn new(config: CacheConfig) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            config,
            hits: Arc::new(RwLock::new(0)),
            misses: Arc::new(RwLock::new(0)),
        }
    }

    /// 创建默认配置的缓存
    pub fn default() -> Self {
        Self::new(CacheConfig::default())
    }

    /// 从缓存读取文件内容
    /// 
    /// 返回 Some(content) 表示命中缓存
    /// 返回 None 表示需要从磁盘读取
    pub fn read(&self, path: &Path) -> Option<String> {
        if !self.config.enabled {
            return None;
        }

        let path_buf = path.to_path_buf();
        let mut cache = self.cache.write().unwrap();
        
        if let Some(entry) = cache.get_mut(&path_buf) {
            // 检查 TTL
            let now = now_timestamp();
            if (now - entry.accessed_at) > self.config.ttl_seconds {
                debug!(path = ?path, "Cache entry expired (TTL)");
                cache.remove(&path_buf);
                *self.misses.write().unwrap() += 1;
                return None;
            }

            // 检查文件是否被修改
            match std::fs::metadata(path) {
                Ok(metadata) => {
                    let current_mtime = metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);

                    if current_mtime > entry.mtime {
                        debug!(path = ?path, "Cache entry invalidated (file modified)");
                        cache.remove(&path_buf);
                        *self.misses.write().unwrap() += 1;
                        return None;
                    }
                }
                Err(_) => {
                    // 文件不存在或无法访问，移除缓存
                    debug!(path = ?path, "Cache entry invalidated (file unavailable)");
                    cache.remove(&path_buf);
                    *self.misses.write().unwrap() += 1;
                    return None;
                }
            }

            // 命中缓存
            entry.touch();
            *self.hits.write().unwrap() += 1;
            debug!(path = ?path, "Cache hit");
            return Some(entry.content.clone());
        }

        *self.misses.write().unwrap() += 1;
        None
    }

    /// 异步从缓存读取文件内容
    /// 
    /// 返回 Some(content) 表示命中缓存
    /// 返回 None 表示需要从磁盘读取
    pub async fn read_async(&self, path: &Path) -> Option<String> {
        if !self.config.enabled {
            return None;
        }

        let path_buf = path.to_path_buf();
        
        // 第一次获取锁：检查缓存是否存在
        let cache_result = {
            let mut cache = self.cache.write().unwrap();
            if let Some(entry) = cache.get_mut(&path_buf) {
                // 检查 TTL
                let now = now_timestamp();
                if (now - entry.accessed_at) > self.config.ttl_seconds {
                    debug!(path = ?path, "Cache entry expired (TTL)");
                    cache.remove(&path_buf);
                    *self.misses.write().unwrap() += 1;
                    return None;
                }

                // 获取当前缓存内容和 mtime
                let content = entry.content.clone();
                let cached_mtime = entry.mtime;
                
                // touch 并增加命中计数
                entry.touch();
                *self.hits.write().unwrap() += 1;
                
                Some((content, cached_mtime))
            } else {
                *self.misses.write().unwrap() += 1;
                None
            }
        };

        // 如果缓存命中，检查文件是否被修改（异步）
        if let Some((content, cached_mtime)) = cache_result {
            match tokio::fs::metadata(path).await {
                Ok(metadata) => {
                    let current_mtime = metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);

                    if current_mtime > cached_mtime {
                        debug!(path = ?path, "Cache entry invalidated (file modified)");
                        self.invalidate(path);
                        return None;
                    }
                }
                Err(_) => {
                    // 文件不存在或无法访问，移除缓存
                    debug!(path = ?path, "Cache entry invalidated (file unavailable)");
                    self.invalidate(path);
                    return None;
                }
            }

            // 命中缓存
            debug!(path = ?path, "Cache hit");
            return Some(content);
        }

        None
    }

    /// 将文件内容写入缓存
    pub fn write(&self, path: &Path, content: &str) {
        if !self.config.enabled {
            return;
        }

        // 检查文件大小限制
        if content.len() > self.config.max_file_size {
            debug!(path = ?path, size = content.len(), "File too large for cache");
            return;
        }

        let path_buf = path.to_path_buf();
        
        // 获取文件 mtime
        let mtime = std::fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(now_timestamp());

        let mut cache = self.cache.write().unwrap();

        // 如果缓存已满，进行清理
        if cache.len() >= self.config.max_entries {
            self.cleanup(&mut cache);
        }

        cache.insert(path_buf, CacheEntry::new(content.to_string(), mtime, content.len()));
        debug!(path = ?path, "Cache written");
    }

    /// 异步将文件内容写入缓存
    pub async fn write_async(&self, path: &Path, content: &str) {
        if !self.config.enabled {
            return;
        }

        // 检查文件大小限制
        if content.len() > self.config.max_file_size {
            debug!(path = ?path, size = content.len(), "File too large for cache");
            return;
        }

        let path_buf = path.to_path_buf();
        
        // 先获取文件 mtime（异步），避免持有锁时进行 IO
        let mtime = tokio::fs::metadata(path)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(now_timestamp());

        // 获取锁并写入缓存
        let mut cache = self.cache.write().unwrap();

        // 如果缓存已满，进行清理
        if cache.len() >= self.config.max_entries {
            self.cleanup(&mut cache);
        }

        cache.insert(path_buf, CacheEntry::new(content.to_string(), mtime, content.len()));
        debug!(path = ?path, "Cache written");
    }

    /// 移除指定路径的缓存
    pub fn invalidate(&self, path: &Path) {
        let mut cache = self.cache.write().unwrap();
        if cache.remove(path).is_some() {
            debug!(path = ?path, "Cache invalidated");
        }
    }

    /// 清除所有缓存
    pub fn clear(&self) {
        let mut cache = self.cache.write().unwrap();
        let count = cache.len();
        cache.clear();
        debug!(count, "Cache cleared");
    }

    /// 获取缓存统计信息
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.cache.read().unwrap().len(),
            hits: *self.hits.read().unwrap(),
            misses: *self.misses.read().unwrap(),
            hit_rate: {
                let hits = *self.hits.read().unwrap();
                let misses = *self.misses.read().unwrap();
                let total = hits + misses;
                if total == 0 {
                    0.0
                } else {
                    hits as f64 / total as f64 * 100.0
                }
            },
        }
    }

    /// 清理过期或最久未使用的条目
    fn cleanup(&self, cache: &mut HashMap<PathBuf, CacheEntry>) {
        let now = now_timestamp();
        
        // 首先移除过期的条目
        cache.retain(|_, entry| {
            (now - entry.accessed_at) <= self.config.ttl_seconds
        });

        // 如果仍然超过限制，移除最久未使用的条目
        if cache.len() >= self.config.max_entries {
            // 先收集所有需要移除的路径
            let paths_to_remove: Vec<PathBuf> = {
                let mut entries: Vec<_> = cache.iter().collect();
                entries.sort_by_key(|(_, entry)| entry.accessed_at);
                
                let to_remove = entries.len() / 5;
                entries.into_iter().take(to_remove).map(|(p, _)| p.to_path_buf()).collect()
            };
            
            // 然后移除这些路径
            let removed_count = paths_to_remove.len();
            for path in paths_to_remove {
                cache.remove(path.as_path());
            }
            
            debug!(removed = removed_count, "Cache cleanup completed");
        }
    }
}

/// 缓存统计信息
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// 当前缓存条目数
    pub entries: usize,
    /// 缓存命中次数
    pub hits: usize,
    /// 缓存未命中次数
    pub misses: usize,
    /// 命中率（百分比）
    pub hit_rate: f64,
}

impl CacheStats {
    pub fn to_string(&self) -> String {
        format!(
            "CacheStats: entries={}, hits={}, misses={}, hit_rate={:.2}%",
            self.entries, self.hits, self.misses, self.hit_rate
        )
    }
}

impl Default for ReadCache {
    fn default() -> Self {
        Self::default()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;
    use tempfile::tempdir;

    #[test]
    fn cache_read_write() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello world").unwrap();

        let cache = ReadCache::default();

        // 第一次读取应该未命中
        assert!(cache.read(&file_path).is_none());
        
        // 写入缓存
        cache.write(&file_path, "hello world");
        
        // 第二次读取应该命中
        assert_eq!(cache.read(&file_path), Some("hello world".to_string()));
    }

    #[test]
    fn cache_invalidated_on_modification() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "version 1").unwrap();

        let cache = ReadCache::default();

        // 读取并缓存
        cache.write(&file_path, "version 1");
        assert_eq!(cache.read(&file_path), Some("version 1".to_string()));

        // 直接使缓存失效
        cache.invalidate(&file_path);

        // 再次读取应该未命中
        assert!(cache.read(&file_path).is_none());
    }

    #[test]
    fn cache_invalidated_on_mtime_change() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "version 1").unwrap();

        let cache = ReadCache::default();

        // 读取并缓存
        cache.write(&file_path, "version 1");
        assert_eq!(cache.read(&file_path), Some("version 1".to_string()));

        // 修改文件并等待足够时间确保 mtime 变化
        std::thread::sleep(Duration::from_secs(1));
        fs::write(&file_path, "version 2").unwrap();

        // 再次读取应该未命中（文件已修改）
        assert!(cache.read(&file_path).is_none());
    }

    #[test]
    fn cache_ttl_expiry() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "content").unwrap();

        let cache = ReadCache::new(CacheConfig {
            ttl_seconds: 1,
            ..CacheConfig::default()
        });

        // 读取并缓存
        cache.write(&file_path, "content");
        assert_eq!(cache.read(&file_path), Some("content".to_string()));

        // 等待 TTL 过期
        std::thread::sleep(Duration::from_secs(2));

        // 再次读取应该未命中（TTL 过期）
        assert!(cache.read(&file_path).is_none());
    }

    #[test]
    fn cache_size_limit() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("large.txt");
        let large_content = "x".repeat(2 * 1024 * 1024); // 2MB
        fs::write(&file_path, &large_content).unwrap();

        let cache = ReadCache::new(CacheConfig {
            max_file_size: 1 * 1024 * 1024, // 1MB
            ..CacheConfig::default()
        });

        // 大文件不应该被缓存
        cache.write(&file_path, &large_content);
        assert!(cache.read(&file_path).is_none());
    }

    #[test]
    fn cache_cleanup() {
        let dir = tempdir().unwrap();
        
        let cache = ReadCache::new(CacheConfig {
            max_entries: 5,
            ..CacheConfig::default()
        });

        // 添加超过限制的条目
        for i in 0..10 {
            let file_path = dir.path().join(format!("file{}.txt", i));
            fs::write(&file_path, format!("content {}", i)).unwrap();
            cache.write(&file_path, &format!("content {}", i));
        }

        // 缓存应该被清理到限制范围内
        let stats = cache.stats();
        assert!(stats.entries <= 5);
    }

    #[test]
    fn cache_stats() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "content").unwrap();

        let cache = ReadCache::default();

        // 未命中
        assert!(cache.read(&file_path).is_none());
        
        // 写入并命中
        cache.write(&file_path, "content");
        assert_eq!(cache.read(&file_path), Some("content".to_string()));
        assert_eq!(cache.read(&file_path), Some("content".to_string()));

        let stats = cache.stats();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hit_rate, (2.0 / 3.0) * 100.0);
    }

    #[test]
    fn cache_disabled() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "content").unwrap();

        let cache = ReadCache::new(CacheConfig {
            enabled: false,
            ..CacheConfig::default()
        });

        // 缓存被禁用，写入和读取都应该无效
        cache.write(&file_path, "content");
        assert!(cache.read(&file_path).is_none());
    }
}
