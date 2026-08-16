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
use tokio::sync::{Mutex, Notify};
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

    // W10: 每连接一个取消通知 — 前端"停止生成"时唤醒正在运行的 agent 任务
    let cancel_notify = Arc::new(Notify::new());

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
                    let cancel_notify = cancel_notify.clone();

                    tokio::spawn(async move {
                        // 发送 thinking 事件
                        let thinking = ServerEvent::thinking("正在处理消息...");
                        let _ = conn_manager.send_to(conn_id, thinking).await;

                        // 创建 WebMessageOutput 来捕获 Agent 输出并转发为事件
                        let mut output = WebMessageOutput::new(conn_manager.clone(), conn_id);

                        // 锁定 session 并运行 Agent
                        let mut session_guard = session.lock().await;
                        let agent = &mut session_guard.agent;

                        // W10: 运行 Agent，同时监听取消通知。
                        // 前端发送 Cancel 后，notify_one 唤醒此分支，
                        // agent.run 的 future 被 drop，锁随之释放。
                        let result = tokio::select! {
                            r = agent.run(msg_content.clone(), &mut output) => r,
                            _ = cancel_notify.notified() => {
                                output.status_aborted();
                                // 补发 done，让前端复位 busy 状态
                                let _ = conn_manager.send_to(conn_id, ServerEvent::done(msg_id)).await;
                                // 取消路径：把已记录的事件刷盘，保证会话详情/列表可见完整数据
                                agent.flush_persistence();
                                return;
                            }
                        };

                        // 回合结束：立即刷盘，确保 Web 会话详情与背景 ingest 能读到完整回合
                        agent.flush_persistence();

                        // 释放锁，避免在发送事件时持有
                        drop(session_guard);

                        match result {
                            Ok(agent_result) => {
                                if agent_result.success {
                                    // 已流式推送过内容时不再重复发送最终消息，
                                    // 避免前端出现两条 assistant 消息
                                    if !output.streamed {
                                        let msg = ServerEvent::assistant_message(
                                            agent_result.message,
                                            false,
                                        );
                                        let _ = conn_manager.send_to(conn_id, msg).await;
                                    }
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
                    // W10: 唤醒正在运行的 agent 任务（若存在）
                    cancel_notify.notify_one();
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
    /// 是否已通过 `streaming_assistant` 推送过流式内容。
    /// 为 true 时，run() 返回后不再重复发送最终消息，避免前端出现两条。
    streamed: bool,
    /// 已通过 `AssistantStreamDelta` 发送的内容字节数（基于 UTF-8 字节边界）。
    /// agent 仍传"累积完整内容"，本实现只截取 `content[sent_len..]` 作为 delta，
    /// 把全量重传降为增量下发，避免长回复 O(n²) 传输与渲染。
    /// 注意：用字节而非字符计数，因 `content` 是 UTF-8，按字节切片在 char 边界对齐，
    /// 切到半个多字节字符会 panic——见下方 `safe_slice_from` 处理。
    sent_len: usize,
}

impl WebMessageOutput {
    fn new(conn_manager: Arc<ConnectionManager>, conn_id: usize) -> Self {
        Self {
            conn_manager,
            conn_id,
            streamed: false,
            sent_len: 0,
        }
    }

    /// 发送"已停止生成"状态事件（取消分支用）。
    fn status_aborted(&self) {
        let event = ServerEvent::status("已停止生成");
        if let Err(e) = self.conn_manager.try_send_to(self.conn_id, event) {
            tracing::warn!("WebSocket 取消状态发送失败: {}", e);
        }
    }

    /// 从 `content` 的 `sent_len` 字节偏移处安全截取剩余部分作为 delta。
    ///
    /// `sent_len` 始终落在已发送 delta 的末尾（即前一次 `content.len()` 的字符边界），
    /// 故此处切片在 UTF-8 字符边界对齐。若因异常情况未对齐，则向前回退到最近
    /// 字符边界，保证不产生半个字符。
    fn safe_slice_from<'a>(&self, content: &'a str) -> &'a str {
        if self.sent_len >= content.len() {
            return ""; // 本次内容没有新增（或更短，理论上不应发生）
        }
        // floor_char_boundary: 找到 <= sent_len 的最近字符边界
        let mut idx = self.sent_len;
        while idx > 0 && !content.is_char_boundary(idx) {
            idx -= 1;
        }
        &content[idx..]
    }
}

impl MessageOutput for WebMessageOutput {
    fn emit(&mut self, level: crate::utils::message_level::MessageLevel, msg: &str) {
        let event = match level {
            crate::utils::message_level::MessageLevel::Info => {
                if msg.starts_with("💭") || msg.contains("思考") || msg.contains("分析") {
                    Some(ServerEvent::thinking(msg))
                } else if msg.starts_with("🔧") || msg.starts_with("LLM 请求调用") || msg.contains("工具") {
                    let tool_name = msg
                        .strip_prefix("🔧")
                        .or_else(|| msg.strip_prefix("执行工具: "))
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("工具");
                    Some(ServerEvent::tool_call(tool_name, serde_json::Value::Null))
                } else if msg.starts_with("✅") {
                    Some(ServerEvent::tool_result("操作", true, msg))
                } else if msg.starts_with("❌") || msg.starts_with("⚠️") {
                    Some(ServerEvent::status(msg))
                } else if msg.starts_with("↻") {
                    Some(ServerEvent::thinking(msg))
                } else {
                    Some(ServerEvent::status(msg))
                }
            }
            crate::utils::message_level::MessageLevel::Success => {
                let content = msg.strip_prefix("✅ ").unwrap_or(msg);
                Some(ServerEvent::tool_result("操作", true, content))
            }
            crate::utils::message_level::MessageLevel::Error => {
                Some(ServerEvent::error(msg))
            }
            crate::utils::message_level::MessageLevel::Warning => {
                Some(ServerEvent::status(format!("⚠️ {}", msg)))
            }
            crate::utils::message_level::MessageLevel::Debug => {
                None
            }
        };

        if let Some(ref event) = event {
            if let Err(e) = self.conn_manager.try_send_to(self.conn_id, event.clone()) {
                tracing::warn!("WebSocket 同步发送事件失败: {}", e);
            }
        }
    }

    /// C1: 将 Agent 的流式内容转发为 `assistant_stream_delta` 增量事件。
    ///
    /// trait 仍按"累积完整内容"调用（agent/mod.rs 逐 token push_str 后传入），
    /// 本实现只截取 `content[sent_len..]` 作为 delta 下发，把每 token 全量重传
    /// 降为增量，避免长回复 O(n²) 的网络与渲染开销。
    ///
    /// 多步回合（tool loop）处理：agent 每个 step 重新累积内容，`content` 会
    /// 比上一次短。检测到 `content.len() < sent_len` 时判定为新回合，重置
    /// `sent_len` 从头发送，保证后续 assistant 消息不被吞掉前缀。
    ///
    /// `is_final=true` 时即便 delta 为空也发送（前端据此移除流式光标）。
    /// `streamed` 标志阻止 run() 返回后重复发送最终消息。
    fn streaming_assistant(&mut self, content: &str, is_final: bool) {
        self.streamed = true;
        // 新一回合流式：content 比上次记录短 → agent 已开始新累积，从头发
        if content.len() < self.sent_len {
            self.sent_len = 0;
        }
        let delta = self.safe_slice_from(content).to_string();
        // 无论发送是否成功都推进 sent_len，避免下次重发同一 delta 造成前端重复
        self.sent_len = content.len();
        if delta.is_empty() && !is_final {
            return;
        }
        let event = ServerEvent::assistant_stream_delta(delta, is_final);
        if let Err(e) = self.conn_manager.try_send_to(self.conn_id, event) {
            tracing::warn!("WebSocket 流式增量事件发送失败: {}", e);
        }
    }

    /// 将 Token 用量信息转发为 `token_usage` 事件。
    ///
    /// 每次 Agent 收到 `LlmStreamEvent::Usage` 时调用此方法，
    /// 将累计用量发送到前端展示。
    fn report_token_usage(&mut self, prompt_tokens: usize, completion_tokens: usize, total_tokens: usize) {
        let event = ServerEvent::token_usage(prompt_tokens, completion_tokens, total_tokens);
        if let Err(e) = self.conn_manager.try_send_to(self.conn_id, event) {
            tracing::warn!("WebSocket token_usage 事件发送失败: {}", e);
        }
    }
}