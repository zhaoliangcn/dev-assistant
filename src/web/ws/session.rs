//! WebSocket 会话管理。
//!
//! 每个 WebSocket 连接对应一个 `WebSession`，持有独立的 `Agent` 实例。
//! Agent 使用 `Arc<LlmClient>` 和 `Arc<ToolRegistry>` 等共享资源，
//! 但 `ContextManager` 是每个会话独立的。

use std::sync::Arc;

use chrono::Utc;

use crate::agent::{Agent, AgentConfig};
use crate::llm::LlmClient;
use crate::persist::SessionStore;

/// Web 会话：包装一个独立的 Agent 实例。
pub struct WebSession {
    /// 会话 ID（创建时的时间戳）
    pub id: String,
    /// 独立的 Agent 实例
    pub agent: Agent,
    /// 创建时间
    #[allow(dead_code)]
    pub created_at: chrono::DateTime<Utc>,
    /// 已处理的消息数
    pub message_count: usize,
}

impl WebSession {
    /// 创建一个新的 Web 会话（Phase 1：基础聊天模式）。
    ///
    /// 每个会话持有独立的 `Agent`，共享 `Arc<LlmClient>`。
    /// `ToolRegistry` 和 `AsyncToolRegistry` 在 Phase 2 中集成。
    ///
    /// `working_dir` 指代要操作的项目目录（所有工具的工作根），
    /// 由 `chat` handler 在建立连接时注入，而非 `env::current_dir()`。
    pub fn new(
        llm: Arc<LlmClient>,
        config: AgentConfig,
        session_store: Option<SessionStore>,
        system_prompt: String,
        max_tokens: usize,
        working_dir: std::path::PathBuf,
    ) -> Self {
        let context = crate::agent::ContextManager::new(system_prompt, max_tokens);
        let tools = crate::tools::ToolRegistry::new(
            working_dir.clone(),
            Arc::new(crate::security::SecurityPolicy::new(
                &working_dir,
                None,
                false,
            )),
        );
        let agent = Agent::new(context, tools, None, llm, config, Vec::new(), session_store);

        Self {
            id: Utc::now().format("%Y%m%d-%H%M%S-%3f").to_string(),
            agent,
            created_at: Utc::now(),
            message_count: 0,
        }
    }

    /// 获取会话的简短摘要信息（用于界面显示）。
    #[allow(dead_code)]
    pub fn summary(&self) -> String {
        format!(
            "会话 {} | 创建于 {} | {} 条消息",
            &self.id[..16],
            self.created_at.format("%H:%M:%S"),
            self.message_count
        )
    }

    /// 递增消息计数。
    pub fn increment_message_count(&mut self) {
        self.message_count += 1;
    }
}