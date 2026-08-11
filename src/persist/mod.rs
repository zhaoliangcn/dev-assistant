//! 会话数据持久化存储。
//!
//! 使用 append-only JSONL（JSON Lines）格式将每一次对话事件、工具调用和
//! 上下文压缩记录到文件中。即使 Agent 内部的上下文压缩删除了历史消息，
//! 所有原始数据仍可在持久化文件中找到。
//!
//! 文件路径: `.dev-assistant-store/session_{YYYYMMDD-HHMMSS}.jsonl`
//!
//! 每条记录独立一行 JSON，可通过 `jq`、`grep` 或 [`SessionStore::read_events`] 查询。

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::utils::error::AppError;

// ── flush 策略配置（通过环境变量可调） ──

/// 批量 flush 的条数阈值，达到此数后触发一次 flush。
fn flush_batch_size() -> u32 {
    std::env::var("PERSIST_FLUSH_BATCH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10)
}

/// 批量 flush 的时间间隔（毫秒），超过此时间后触发一次 flush。
fn flush_interval_ms() -> u64 {
    std::env::var("PERSIST_FLUSH_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000)
}

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

// ---------------------------------------------------------------------------
// 事件类型
// ---------------------------------------------------------------------------

/// 所有可持久化的会话事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    /// 用户消息
    UserMessage {
        timestamp: String,
        session_id: String,
        content: String,
    },
    /// 助手（LLM）文本回复
    AssistantMessage {
        timestamp: String,
        session_id: String,
        content: String,
    },
    /// 系统消息（技能激活、重启提示等）
    SystemMessage {
        timestamp: String,
        session_id: String,
        content: String,
    },
    /// 助手请求调用工具
    ToolCallRequest {
        timestamp: String,
        session_id: String,
        tool_call_id: String,
        name: String,
        arguments: Value,
    },
    /// 工具执行结果
    ToolResult {
        timestamp: String,
        session_id: String,
        tool_call_id: String,
        name: String,
        success: bool,
        content: String,
    },
    /// 上下文压缩事件（记录被截断的信息量）
    Compression {
        timestamp: String,
        session_id: String,
        original_messages: usize,
        after_messages: usize,
        kept_rounds: usize,
        original_tokens: usize,
        after_tokens: usize,
    },
}

// ---------------------------------------------------------------------------
// SessionStore
// ---------------------------------------------------------------------------

/// 会话数据持久化存储。
///
/// 使用 append-only JSONL 格式，每条记录独立一行 JSON。
/// 即使发生上下文压缩，所有历史数据仍保留在文件中。
///
/// # 用法
///
/// ```ignore
/// let mut store = SessionStore::create(working_dir)?;
/// store.record_user_message("你好");
/// store.record_assistant_message("你好！有什么可以帮助你的？");
/// store.record_tool_call("call_123", "read_file", serde_json::json!({"path": "src/main.rs"}));
/// store.record_tool_result("call_123", "read_file", true, "file content...");
/// ```
pub struct SessionStore {
    /// 带缓冲的写入器，减少每次事件落盘的 I/O 次数。
    writer: BufWriter<File>,
    path: PathBuf,
    session_id: String,
    /// 当前批次已写入但尚未 flush 的事件数。
    pending_events: u32,
    /// 最近一次 flush 的时间戳，用于定时 flush。
    last_flush: Instant,
    /// 连续写入失败次数。达到阈值后升级日志级别。
    consecutive_write_failures: u32,
}

impl SessionStore {
    /// 在工作目录下创建持久化存储。
    ///
    /// 创建 `.dev-assistant-store/` 目录（如不存在），
    /// 然后以 append 模式打开 `session_{timestamp}.jsonl`。
    pub fn create(working_dir: &Path) -> Result<Self, AppError> {
        let store_dir = working_dir.join(".dev-assistant-store");
        fs::create_dir_all(&store_dir).map_err(|e| {
            AppError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "创建持久化存储目录失败 ({}): {}",
                    store_dir.display(),
                    e
                ),
            ))
        })?;

        let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let filename = format!("session_{}.jsonl", timestamp);
        let path = store_dir.join(&filename);

        let mut options = OpenOptions::new();
        options.create(true).append(true).write(true);
        #[cfg(unix)]
        options.custom_flags(libc::O_CLOEXEC); // SECURITY: Auto-close on exec()
        #[cfg(unix)]
        options.mode(0o600); // SECURITY: 仅所有者可读写
        let file = options.open(&path).map_err(|e| {
            AppError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "创建持久化存储文件失败 ({}): {}",
                    path.display(),
                    e
                ),
            ))
        })?;

        Ok(Self {
            writer: BufWriter::new(file),
            path,
            session_id: timestamp.to_string(),
            pending_events: 0,
            last_flush: Instant::now(),
            consecutive_write_failures: 0,
        })
    }

    /// 获取存储文件路径。
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 获取会话 ID（即创建时的时间戳）。
    #[allow(dead_code)]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    // ── 内部辅助 ──

    /// 按需 flush：当累积事件数达到阈值，或距离上次 flush 超过时间间隔时触发。
    fn maybe_flush(&mut self) {
        let batch_size = flush_batch_size();
        let interval = Duration::from_millis(flush_interval_ms());

        let should_flush = self.pending_events >= batch_size
            || self.last_flush.elapsed() >= interval;

        if should_flush {
            let write_ok = self.writer.flush().is_ok();
            if write_ok {
                self.pending_events = 0;
                self.last_flush = Instant::now();
                self.consecutive_write_failures = 0;
            } else {
                self.consecutive_write_failures += 1;
                if self.consecutive_write_failures >= 3 {
                    tracing::error!(
                        consecutive_failures = self.consecutive_write_failures,
                        file = %self.path.display(),
                        pending = self.pending_events,
                        "会话持久化连续 flush 失败（可能磁盘已满或权限变更），后续事件可能丢失"
                    );
                } else {
                    tracing::warn!(
                        file = %self.path.display(),
                        pending = self.pending_events,
                        "会话持久化 flush 失败"
                    );
                }
            }
        }
    }

    /// 将缓冲区中尚未落盘的事件立即写入文件。
    ///
    /// 应在会话结束、或任何跨模块读取该 JSONL 文件之前调用，
    /// 以缩小崩溃丢失窗口并保证读后写可见性（如 Web 会话详情、
    /// 背景 ingest 扫描等并发读者能看到完整数据）。
    pub fn flush(&mut self) {
        if self.pending_events == 0 {
            return;
        }
        if self.writer.flush().is_ok() {
            self.pending_events = 0;
            self.last_flush = Instant::now();
            self.consecutive_write_failures = 0;
        } else {
            self.consecutive_write_failures += 1;
            tracing::warn!(
                file = %self.path.display(),
                pending = self.pending_events,
                "会话持久化显式 flush 失败，缓冲事件可能丢失"
            );
        }
    }

    /// 追加一条事件记录到文件。写入失败仅记录 warning，不向上传播。
    fn append_event(&mut self, event: &SessionEvent) {
        match serde_json::to_string(event) {
            Ok(json) => {
                let write_ok = writeln!(self.writer, "{}", json).is_ok();
                if write_ok {
                    self.pending_events += 1;
                    self.maybe_flush();
                } else {
                    self.consecutive_write_failures += 1;
                    if self.consecutive_write_failures >= 3 {
                        tracing::error!(
                            consecutive_failures = self.consecutive_write_failures,
                            file = %self.path.display(),
                            "会话持久化连续写入失败（可能磁盘已满或权限变更），后续事件可能丢失"
                        );
                    } else {
                        tracing::warn!(
                            file = %self.path.display(),
                            "写入会话持久化事件失败"
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "序列化会话持久化事件失败"
                );
            }
        }
    }

    fn timestamp() -> String {
        Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
    }

    // ── 公开事件记录方法 ──

    /// 记录用户消息。
    pub fn record_user_message(&mut self, content: &str) {
        self.append_event(&SessionEvent::UserMessage {
            timestamp: Self::timestamp(),
            session_id: self.session_id.clone(),
            content: content.to_string(),
        });
    }

    /// 记录助手（LLM）文本回复。
    pub fn record_assistant_message(&mut self, content: &str) {
        self.append_event(&SessionEvent::AssistantMessage {
            timestamp: Self::timestamp(),
            session_id: self.session_id.clone(),
            content: content.to_string(),
        });
    }

    /// 记录系统消息。
    pub fn record_system_message(&mut self, content: &str) {
        self.append_event(&SessionEvent::SystemMessage {
            timestamp: Self::timestamp(),
            session_id: self.session_id.clone(),
            content: content.to_string(),
        });
    }

    /// 记录工具调用请求。
    pub fn record_tool_call(&mut self, tool_call_id: &str, name: &str, arguments: Value) {
        self.append_event(&SessionEvent::ToolCallRequest {
            timestamp: Self::timestamp(),
            session_id: self.session_id.clone(),
            tool_call_id: tool_call_id.to_string(),
            name: name.to_string(),
            arguments,
        });
    }

    /// 记录工具执行结果。
    pub fn record_tool_result(
        &mut self,
        tool_call_id: &str,
        name: &str,
        success: bool,
        content: &str,
    ) {
        self.append_event(&SessionEvent::ToolResult {
            timestamp: Self::timestamp(),
            session_id: self.session_id.clone(),
            tool_call_id: tool_call_id.to_string(),
            name: name.to_string(),
            success,
            content: content.to_string(),
        });
    }

    /// 记录上下文压缩事件。
    pub fn record_compression(
        &mut self,
        original_messages: usize,
        after_messages: usize,
        kept_rounds: usize,
        original_tokens: usize,
        after_tokens: usize,
    ) {
        self.append_event(&SessionEvent::Compression {
            timestamp: Self::timestamp(),
            session_id: self.session_id.clone(),
            original_messages,
            after_messages,
            kept_rounds,
            original_tokens,
            after_tokens,
        });
    }

    // ── 查询方法 ──

    /// 从指定路径的 JSONL 文件读取所有事件。
    ///
    /// 可用于按会话离线查询历史数据。
    #[allow(dead_code)]
    pub fn read_events(path: &Path) -> Result<Vec<SessionEvent>, AppError> {
        let file = File::open(path).map_err(AppError::Io)?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();

        for (line_num, line) in reader.lines().enumerate() {
            let line = line.map_err(AppError::Io)?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<SessionEvent>(trimmed) {
                Ok(event) => events.push(event),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        line = line_num + 1,
                        file = %path.display(),
                        "解析持久化事件失败，跳过该行"
                    );
                }
            }
        }

        Ok(events)
    }

    /// 列出所有历史会话存储文件。
    #[allow(dead_code)]
    pub fn list_sessions(working_dir: &Path) -> Result<Vec<PathBuf>, AppError> {
        let store_dir = working_dir.join(".dev-assistant-store");
        if !store_dir.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();
        let mut dir_entries = fs::read_dir(&store_dir).map_err(AppError::Io)?;
        while let Some(entry) = dir_entries.next().transpose().map_err(AppError::Io)? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                sessions.push(path);
            }
        }

        // 按文件名排序（即按时间排序）
        sessions.sort();
        Ok(sessions)
    }
}

impl Drop for SessionStore {
    fn drop(&mut self) {
        let _ = self.writer.flush();
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_create_and_record() {
        let dir = tempdir().unwrap();
        let mut store = SessionStore::create(dir.path()).unwrap();

        // 验证文件已创建
        assert!(store.path().exists());
        assert!(store.path().to_string_lossy().contains("session_"));

        // 记录各种事件
        store.record_user_message("hello");
        store.record_assistant_message("hi there");
        store.record_tool_call("call_1", "read_file", serde_json::json!({"path": "test.txt"}));
        store.record_tool_result("call_1", "read_file", true, "file content");
        store.record_compression(50, 12, 6, 8000, 2000);

        // 刷新并验证文件内容
        drop(store);

        let events = SessionStore::read_events(&dir.path().join(".dev-assistant-store").join(
            fs::read_dir(dir.path().join(".dev-assistant-store"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .file_name(),
        ))
        .unwrap();

        assert_eq!(events.len(), 5);

        // 验证事件类型和字段
        match &events[0] {
            SessionEvent::UserMessage { content, .. } => assert_eq!(content, "hello"),
            other => panic!("Expected UserMessage, got {:?}", other),
        }
        match &events[1] {
            SessionEvent::AssistantMessage { content, .. } => assert_eq!(content, "hi there"),
            other => panic!("Expected AssistantMessage, got {:?}", other),
        }
        match &events[2] {
            SessionEvent::ToolCallRequest { name, .. } => assert_eq!(name, "read_file"),
            other => panic!("Expected ToolCallRequest, got {:?}", other),
        }
        match &events[3] {
            SessionEvent::ToolResult { name, success, .. } => {
                assert_eq!(name, "read_file");
                assert!(*success);
            }
            other => panic!("Expected ToolResult, got {:?}", other),
        }
        match &events[4] {
            SessionEvent::Compression { original_messages, after_messages, kept_rounds, .. } => {
                assert_eq!(*original_messages, 50);
                assert_eq!(*after_messages, 12);
                assert_eq!(*kept_rounds, 6);
            }
            other => panic!("Expected Compression, got {:?}", other),
        }
    }

    #[test]
    fn test_list_sessions() {
        let dir = tempdir().unwrap();
        let sessions = SessionStore::list_sessions(dir.path()).unwrap();
        assert!(sessions.is_empty());

        let _store = SessionStore::create(dir.path()).unwrap();
        let sessions = SessionStore::list_sessions(dir.path()).unwrap();
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn test_empty_file() {
        let dir = tempdir().unwrap();
        let store_path = dir.path().join("empty.jsonl");
        File::create(&store_path).unwrap();

        let events = SessionStore::read_events(&store_path).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_session_id_is_consistent() {
        let dir = tempdir().unwrap();
        let store = SessionStore::create(dir.path()).unwrap();
        let sid = store.session_id().to_string();

        // session_id 应该是非空的时间戳字符串
        assert!(!sid.is_empty());
        assert!(sid.len() >= 8); // 至少 YYYYMMDD
    }
}