//! WebSocket 聊天处理器。
//!
//! 处理 WebSocket 连接，接收用户消息，调用 Agent 处理，返回事件流。

use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use futures::stream::StreamExt;
use futures::SinkExt;
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, error, info, warn};

use crate::persist::SessionStore;
use crate::utils::message_output::MessageOutput;
use crate::web::ws::events::{ClientMessage, ServerEvent};
use crate::web::ws::session::WebSession;
use crate::web::ws::ConnectionManager;
use crate::web::AppState;

/// WebSocket 升级处理器。
///
/// 路径: `GET /ws/chat`
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    info!("新的 WebSocket 连接请求");
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// 处理单个 WebSocket 连接。
async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (sender, mut receiver) = socket.split();

    // 创建会话级别的 SessionStore
    let session_store = SessionStore::create(&state.working_dir)
        .map_err(|e| {
            warn!(error = %e, "无法创建会话持久化存储，将跳过持久化");
        })
        .ok();

    // 创建 WebSession
    let web_session = Arc::new(Mutex::new(WebSession::new(
        state.llm.clone(),
        state.agent_config.clone(),
        session_store,
        state.system_prompt.clone(),
        state.max_tokens,
    )));

    // 获取会话 ID 用于注册
    let session_id = {
        let session = web_session.lock().await;
        session.id.clone()
    };

    // 创建连接管理器并注册
    let conn_manager = Arc::new(ConnectionManager::new());
    let (conn_id, mut event_rx) = conn_manager
        .register(session_id.clone())
        .await;

    // 发送就绪事件
    let session_ready = ServerEvent::session_ready(&session_id);
    let _ = conn_manager.send_to(conn_id, session_ready).await;

    // 启动发送任务：从 event_rx 读取事件并发送到 WebSocket
    let send_task = tokio::spawn(async move {
        let mut ws_sender = sender;
        while let Some(event) = event_rx.recv().await {
            let json = match serde_json::to_string(&event) {
                Ok(j) => j,
                Err(e) => {
                    error!("序列化事件失败: {}", e);
                    continue;
                }
            };
            if ws_sender.send(Message::Text(json.into())).await.is_err() {
                debug!("WebSocket 发送失败，连接可能已关闭");
                break;
            }
        }
    });

    // 接收任务：从 WebSocket 接收消息并处理
    let recv_conn_manager = conn_manager.clone();
    let recv_conn_id = conn_id;
    let recv_web_session = web_session.clone();

    let recv_task = tokio::spawn(async move {
        while let Some(msg) = receiver.next().await {
            let msg = match msg {
                Ok(msg) => msg,
                Err(e) => {
                    warn!("WebSocket 接收错误: {}", e);
                    break;
                }
            };

            let text = match msg {
                Message::Text(text) => text,
                Message::Close(_) => {
                    info!("WebSocket 关闭帧收到");
                    break;
                }
                _ => continue,
            };

            // 解析客户端消息
            let client_msg: ClientMessage = match serde_json::from_str(&text) {
                Ok(msg) => msg,
                Err(e) => {
                    warn!("解析客户端消息失败: {}", e);
                    let err_event = ServerEvent::error(format!("消息格式错误: {}", e));
                    let _ = recv_conn_manager.send_to(recv_conn_id, err_event).await;
                    continue;
                }
            };

            match client_msg {
                ClientMessage::UserMessage { content, id } => {
                    info!("收到用户消息: id={}, len={}", id, content.len());

                    // 递增消息计数
                    {
                        let mut session = recv_web_session.lock().await;
                        session.increment_message_count();
                    }

                    // 在独立任务中运行 Agent，避免阻塞 WebSocket 接收
                    let conn_manager = recv_conn_manager.clone();
                    let conn_id = recv_conn_id;
                    let msg_id = id.clone();
                    let msg_content = content.clone();
                    let session = recv_web_session.clone();

                    tokio::spawn(async move {
                        // 发送 thinking 事件
                        let thinking = ServerEvent::thinking("正在处理消息...");
                        let _ = conn_manager.send_to(conn_id, thinking).await;

                        // 创建 WebMessageOutput 来捕获 Agent 输出并转发为事件
                        let mut output = WebMessageOutput::new(conn_manager.clone(), conn_id);

                        // 锁定 session 并运行 Agent
                        let mut session_guard = session.lock().await;
                        let agent = &mut session_guard.agent;

                        // 将用户消息添加到 Agent 上下文
                        agent.start_turn(msg_content.clone(), &mut output);

                        // 运行 Agent 直到完成
                        let result = agent.run(msg_content.clone(), &mut output).await;

                        // 释放锁，避免在发送事件时持有
                        drop(session_guard);

                        match result {
                            Ok(agent_result) => {
                                if agent_result.success {
                                    // 发送最终消息（非流式，已完整生成）
                                    let msg = ServerEvent::assistant_message(
                                        agent_result.message,
                                        false,
                                    );
                                    let _ = conn_manager.send_to(conn_id, msg).await;
                                } else {
                                    let err = ServerEvent::error(agent_result.message);
                                    let _ = conn_manager.send_to(conn_id, err).await;
                                }
                            }
                            Err(e) => {
                                let err = ServerEvent::error(format!("处理失败: {}", e));
                                let _ = conn_manager.send_to(conn_id, err).await;
                            }
                        }

                        // 发送完成事件
                        let done = ServerEvent::done(msg_id);
                        let _ = conn_manager.send_to(conn_id, done).await;
                    });
                }
                ClientMessage::Cancel { message_id } => {
                    info!("取消请求: message_id={:?}", message_id);
                    let status = ServerEvent::status("已取消");
                    let _ = recv_conn_manager.send_to(recv_conn_id, status).await;
                }
            }
        }
    });

    // 等待发送或接收任务完成
    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }

    // 清理
    conn_manager.unregister(conn_id).await;
    info!("WebSocket 连接已关闭: id={}", conn_id);
}

// ---------------------------------------------------------------------------
// WebMessageOutput — 将 Agent 输出转换为 WebSocket 事件
// ---------------------------------------------------------------------------

/// 将 `MessageOutput` 实现为 WebSocket 事件发送器。
///
/// Agent 在执行过程中通过 `MessageOutput` 输出消息，此实现将消息
/// 转换为对应的 `ServerEvent` 并通过连接管理器发送到浏览器。
struct WebMessageOutput {
    conn_manager: Arc<ConnectionManager>,
    conn_id: usize,
}

impl WebMessageOutput {
    fn new(conn_manager: Arc<ConnectionManager>, conn_id: usize) -> Self {
        Self {
            conn_manager,
            conn_id,
        }
    }
}

impl MessageOutput for WebMessageOutput {
    fn emit(&mut self, level: crate::utils::message_level::MessageLevel, msg: &str) {
        let event = match level {
            crate::utils::message_level::MessageLevel::Info => {
                Some(ServerEvent::status(msg))
            }
            crate::utils::message_level::MessageLevel::Success => {
                Some(ServerEvent::status(format!("✅ {}", msg)))
            }
            crate::utils::message_level::MessageLevel::Error => {
                Some(ServerEvent::error(msg))
            }
            crate::utils::message_level::MessageLevel::Warning => {
                Some(ServerEvent::status(format!("⚠️ {}", msg)))
            }
            crate::utils::message_level::MessageLevel::Debug => {
                // 调试消息仅在 verbose 模式下发送
                None
            }
        };

        if let Some(event) = event {
            let _ = self.conn_manager.send_to(self.conn_id, event);
        }
    }
}