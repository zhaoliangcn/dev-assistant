//! WebSocket 事件类型定义。
//!
//! 定义了浏览器与服务器之间通过 WebSocket 传输的所有消息类型。
//! 使用 JSON 格式序列化，支持双工通信。

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 浏览器 → 服务端
// ---------------------------------------------------------------------------

/// 客户端发送给服务端的消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// 用户发送的对话消息
    UserMessage {
        content: String,
        #[serde(default)]
        id: String,
    },
    /// 取消当前正在处理的请求
    Cancel {
        #[serde(default)]
        message_id: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// 服务端 → 浏览器
// ---------------------------------------------------------------------------

/// 服务端发送给客户端的事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    /// Agent 正在思考/处理中
    Thinking {
        content: String,
        #[serde(default)]
        id: String,
    },
    /// 工具调用信息
    ToolCall {
        tool_name: String,
        args: serde_json::Value,
        #[serde(default)]
        id: String,
    },
    /// 工具执行结果
    ToolResult {
        tool_name: String,
        success: bool,
        content: String,
        #[serde(default)]
        id: String,
    },
    /// 助手回复消息（最终文本输出）
    AssistantMessage {
        content: String,
        #[serde(default)]
        streaming: bool,
        #[serde(default)]
        id: String,
    },
    /// 错误信息
    Error {
        content: String,
        #[serde(default)]
        id: String,
    },
    /// 处理完成
    Done {
        #[serde(default)]
        message_id: String,
    },
    /// 状态信息（连接状态、模型信息等）
    Status {
        content: String,
        #[serde(default)]
        id: String,
    },
    /// 会话已就绪，包含会话 ID
    SessionReady {
        session_id: String,
    },
    /// Token 用量信息（累计 prompt/completion/total tokens）
    TokenUsage {
        prompt_tokens: usize,
        completion_tokens: usize,
        total_tokens: usize,
        #[serde(default)]
        id: String,
    },
}

impl ServerEvent {
    /// 创建一个思考事件
    pub fn thinking(content: impl Into<String>) -> Self {
        Self::Thinking {
            content: content.into(),
            id: uuid_v4(),
        }
    }

    /// 创建一个工具调用事件
    pub fn tool_call(tool_name: impl Into<String>, args: serde_json::Value) -> Self {
        Self::ToolCall {
            tool_name: tool_name.into(),
            args,
            id: uuid_v4(),
        }
    }

    /// 创建一个工具结果事件
    pub fn tool_result(tool_name: impl Into<String>, success: bool, content: impl Into<String>) -> Self {
        Self::ToolResult {
            tool_name: tool_name.into(),
            success,
            content: content.into(),
            id: uuid_v4(),
        }
    }

    /// 创建一个助手消息事件
    pub fn assistant_message(content: impl Into<String>, streaming: bool) -> Self {
        Self::AssistantMessage {
            content: content.into(),
            streaming,
            id: uuid_v4(),
        }
    }

    /// 创建一个错误事件
    pub fn error(content: impl Into<String>) -> Self {
        Self::Error {
            content: content.into(),
            id: uuid_v4(),
        }
    }

    /// 创建一个完成事件
    pub fn done(message_id: impl Into<String>) -> Self {
        Self::Done {
            message_id: message_id.into(),
        }
    }

    /// 创建一个状态事件
    pub fn status(content: impl Into<String>) -> Self {
        Self::Status {
            content: content.into(),
            id: uuid_v4(),
        }
    }

    /// 创建一个会话就绪事件
    pub fn session_ready(session_id: impl Into<String>) -> Self {
        Self::SessionReady {
            session_id: session_id.into(),
        }
    }

    /// 创建一个 Token 用量事件
    pub fn token_usage(prompt_tokens: usize, completion_tokens: usize, total_tokens: usize) -> Self {
        Self::TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            id: uuid_v4(),
        }
    }
}

/// 生成一个简单的 UUID v4 格式 ID（用于事件追踪）
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let nanos = now.as_nanos();
    let rand_part: u64 = {
        // Simple pseudo-random from timestamp bits
        (nanos as u64).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407)
    };
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (nanos >> 32) as u32,
        (nanos >> 16) as u16,
        (rand_part >> 48) as u16 & 0x0fff,
        (rand_part >> 32) as u16 & 0x3fff | 0x8000,
        rand_part & 0xffff_ffff_ffff
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_message_deser() {
        let json = r#"{"type":"user_message","content":"hello","id":"msg_001"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::UserMessage { content, id } => {
                assert_eq!(content, "hello");
                assert_eq!(id, "msg_001");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_server_event_ser() {
        let event = ServerEvent::thinking("analyzing...");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"thinking\""));
        assert!(json.contains("analyzing"));
    }

    #[test]
    fn test_done_event() {
        let event = ServerEvent::done("msg_001");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"done\""));
        assert!(json.contains("msg_001"));
    }
}