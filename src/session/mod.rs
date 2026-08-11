//! 会话日志渲染：从 `SessionStore` 的 JSONL 生成人类可读的会话日志。
//!
//! # 统一持久化方案
//!
//! 历史上存在两条并行的会话记录通路（`SessionStore` JSONL + `SessionLogger` 纯文本），
//! 数据冗余且存在不一致风险。统一后：
//!
//! - **`SessionStore`**（`crate::persist`）是唯一的写入源，记录结构化 JSONL 事件
//!   （用户消息、助手回复、工具调用/结果、压缩事件等）
//! - **本模块** 负责按需将 JSONL 渲染为人类可读的纯文本日志（带脱敏），
//!   供调试和回溯使用，不再自行写入文件
//!
//! `SessionStore` 在创建文件时设置了 `FD_CLOEXEC`（见 `restart.rs` 说明），
//! 确保 `exec()` 后文件句柄自动关闭。

use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;

use crate::persist::SessionEvent;
use crate::utils::error::AppError;

/// 预编译的脱敏正则表达式集合（线程安全，进程级缓存）。
struct SanitizePatterns {
    api_keys: Vec<Regex>,
    bearer: Regex,
    password: Regex,
    private_key: Regex,
    ssh_key: Regex,
    jwt: Regex,
}

static PATTERNS: Lazy<SanitizePatterns> = Lazy::new(|| {
    let api_keys = vec![
        Regex::new(r"(?i)(sk-[a-zA-Z0-9]{20,})").unwrap(),
        Regex::new(r"(?i)(AIza[a-zA-Z0-9_-]{35})").unwrap(),
        Regex::new(r"(?i)(gsk_[a-zA-Z0-9]{20,})").unwrap(),
        Regex::new(r"(?i)(key-[a-zA-Z0-9]{20,})").unwrap(),
    ];
    SanitizePatterns {
        api_keys,
        bearer: Regex::new(r"(?i)(bearer\s+)[a-zA-Z0-9_.-]{20,}").unwrap(),
        password: Regex::new(r#"(?i)(password|passwd|pwd)\s*[=:]\s*["']?([^"'\s,}]{4,})"#).unwrap(),
        private_key: Regex::new(r"-----BEGIN\s+(?:RSA\s+)?PRIVATE\s+KEY-----.*?-----END\s+(?:RSA\s+)?PRIVATE\s+KEY-----").unwrap(),
        ssh_key: Regex::new(r"ssh-(?:rsa|ed25519|dss|ecdsa)\s+[a-zA-Z0-9+/=]+").unwrap(),
        jwt: Regex::new(r"eyJ[a-zA-Z0-9_-]*\.eyJ[a-zA-Z0-9_-]*\.[a-zA-Z0-9_-]+").unwrap(),
    }
});

/// 从 SessionStore 的 JSONL 事件流生成人类可读的会话日志。
///
/// `events_path` 指向 `.dev-assistant-store/session_*.jsonl`。
/// 渲染时对内容做敏感信息脱敏，防止 API Key、密码、令牌等泄露到日志。
pub fn generate_readable_log(events_path: &Path) -> Result<String, AppError> {
    let events = crate::persist::SessionStore::read_events(events_path)?;

    let mut out = String::new();
    out.push_str("════════════════════════════════════════════\n");
    out.push_str(" Dev-Assistant 会话日志（由 JSONL 渲染）\n");
    out.push_str("════════════════════════════════════════════\n");
    for event in &events {
        out.push_str(&render_event(event));
        out.push('\n');
    }
    Ok(out)
}

/// 将单个事件渲染为一行可读文本。
fn render_event(event: &SessionEvent) -> String {
    match event {
        SessionEvent::UserMessage { timestamp, content, .. } => {
            format!("{} ▶ 用户: {}", timestamp, sanitize(content))
        }
        SessionEvent::AssistantMessage { timestamp, content, .. } => {
            format!("{} ◂ 助手: {}", timestamp, sanitize(content))
        }
        SessionEvent::SystemMessage { timestamp, content, .. } => {
            format!("{} ◆ 系统: {}", timestamp, sanitize(content))
        }
        SessionEvent::ToolCallRequest {
            timestamp,
            name,
            arguments,
            ..
        } => format!("{} ⚙ 工具调用: {} (参数: {})", timestamp, name, arguments),
        SessionEvent::ToolResult {
            timestamp,
            name,
            success,
            content,
            ..
        } => {
            let status = if *success { "✅" } else { "❌" };
            format!("{}   {} {}: {}", timestamp, status, name, sanitize(content))
        }
        SessionEvent::Compression {
            timestamp,
            original_messages,
            after_messages,
            kept_rounds,
            original_tokens,
            after_tokens,
            ..
        } => format!(
            "{} ◇ 压缩: {} → {} 条消息 (保留 {} 轮, {} → {} tokens)",
            timestamp,
            original_messages,
            after_messages,
            kept_rounds,
            original_tokens,
            after_tokens
        ),
    }
}

/// 敏感信息脱敏处理，防止 API Key、密码、令牌等泄露到日志。
fn sanitize(content: &str) -> String {
    let p = &*PATTERNS;
    let mut result = content.to_string();

    for re in &p.api_keys {
        result = re.replace_all(&result, "[REDACTED_API_KEY]").to_string();
    }
    result = p.bearer.replace_all(&result, "${1}[REDACTED_TOKEN]").to_string();
    result = p.password.replace_all(&result, "${1}=[REDACTED]").to_string();
    result = p.private_key.replace_all(&result, "[REDACTED_PRIVATE_KEY]").to_string();
    result = p.ssh_key.replace_all(&result, "[REDACTED_SSH_KEY]").to_string();
    result = p.jwt.replace_all(&result, "[REDACTED_JWT]").to_string();

    result
}
