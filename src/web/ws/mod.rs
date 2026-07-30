//! WebSocket 连接管理器。
//!
//! 管理所有活跃的 WebSocket 连接，提供消息广播和连接生命周期管理。
//! 每个连接对应一个独立的 `WebSession`。

pub mod events;
pub mod session;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn};

use self::events::ServerEvent;

/// 连接 ID 生成器
static NEXT_CONN_ID: AtomicUsize = AtomicUsize::new(1);

/// 连接 ID 类型
pub type ConnectionId = usize;

/// 单个 WebSocket 连接的句柄。
/// 持有发送端，用于向该连接推送事件。
pub struct ConnectionHandle {
    #[allow(dead_code)]
    pub id: ConnectionId,
    #[allow(dead_code)]
    pub session_id: String,
    sender: mpsc::UnboundedSender<ServerEvent>,
}

impl ConnectionHandle {
    /// 向该连接发送一个事件。
    pub fn send(&self, event: ServerEvent) -> Result<(), String> {
        self.sender.send(event).map_err(|e| format!("发送事件失败: {}", e))
    }

    /// 获取连接的唯一标识
    #[allow(dead_code)]
    pub fn id(&self) -> ConnectionId {
        self.id
    }
}

/// WebSocket 连接管理器。
///
/// 管理所有活跃的 WebSocket 连接，支持按连接 ID 发送事件。
pub struct ConnectionManager {
    /// 所有活跃的连接
    connections: RwLock<HashMap<ConnectionId, ConnectionHandle>>,
}

impl ConnectionManager {
    /// 创建新的连接管理器。
    pub fn new() -> Self {
        Self {
            connections: RwLock::new(HashMap::new()),
        }
    }

    /// 注册一个新的 WebSocket 连接。
    ///
    /// 返回 `(ConnectionId, mpsc::UnboundedReceiver<ServerEvent>)`，
    /// 调用方应使用 receiver 来读取并发送事件到 WebSocket。
    pub async fn register(
        &self,
        session_id: String,
    ) -> (ConnectionId, mpsc::UnboundedReceiver<ServerEvent>) {
        let id = NEXT_CONN_ID.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::unbounded_channel::<ServerEvent>();

        let handle = ConnectionHandle {
            id,
            session_id,
            sender: tx,
        };

        self.connections.write().await.insert(id, handle);
        info!("WebSocket 连接已注册: id={}", id);

        (id, rx)
    }

    /// 注销一个 WebSocket 连接。
    pub async fn unregister(&self, id: ConnectionId) {
        self.connections.write().await.remove(&id);
        debug!("WebSocket 连接已注销: id={}", id);
    }

    /// 向指定连接发送事件。
    pub async fn send_to(&self, id: ConnectionId, event: ServerEvent) -> Result<(), String> {
        let connections = self.connections.read().await;
        match connections.get(&id) {
            Some(handle) => handle.send(event),
            None => Err(format!("连接不存在: id={}", id)),
        }
    }

    /// 同步版 `send_to`：用于无法 `.await` 的同步上下文（如 `MessageOutput::emit`）。
    ///
    /// 通过 `try_read` 非阻塞地获取读锁；若锁被占用则返回错误，调用方应处理
    /// （通常是丢弃该事件，因为下一条事件很快会跟上）。
    pub fn try_send_to(&self, id: ConnectionId, event: ServerEvent) -> Result<(), String> {
        match self.connections.try_read() {
            Ok(connections) => match connections.get(&id) {
                Some(handle) => handle.send(event),
                None => Err(format!("连接不存在: id={}", id)),
            },
            Err(_) => Err("连接表锁被占用，事件已丢弃".to_string()),
        }
    }

    /// 广播事件到所有活跃连接。
    #[allow(dead_code)]
    pub async fn broadcast(&self, event: ServerEvent) {
        let connections = self.connections.read().await;
        for handle in connections.values() {
            if let Err(e) = handle.send(event.clone()) {
                warn!("广播事件失败: id={}, error={}", handle.id, e);
            }
        }
    }

    /// 获取当前活跃连接数。
    #[allow(dead_code)]
    pub async fn active_count(&self) -> usize {
        self.connections.read().await.len()
    }

    /// 获取所有活跃连接的会话 ID 列表。
    #[allow(dead_code)]
    pub async fn active_sessions(&self) -> Vec<String> {
        self.connections
            .read()
            .await
            .values()
            .map(|h| h.session_id.clone())
            .collect()
    }
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_and_unregister() {
        let manager = ConnectionManager::new();
        let (id, _rx) = manager.register("test-session".to_string()).await;
        assert_eq!(manager.active_count().await, 1);
        manager.unregister(id).await;
        assert_eq!(manager.active_count().await, 0);
    }

    #[tokio::test]
    async fn test_send_to_connection() {
        let manager = ConnectionManager::new();
        let (id, mut rx) = manager.register("test-session".to_string()).await;

        let event = ServerEvent::status("test message");
        manager.send_to(id, event).await.unwrap();

        let received = rx.try_recv().unwrap();
        assert!(matches!(received, ServerEvent::Status { .. }));
    }

    #[tokio::test]
    async fn test_send_to_nonexistent() {
        let manager = ConnectionManager::new();
        let result = manager
            .send_to(999, ServerEvent::status("test"))
            .await;
        assert!(result.is_err());
    }
}