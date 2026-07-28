use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use chrono::Local;
use libc;
use once_cell::sync::Lazy;
use regex::Regex;
use tracing::{debug, warn};

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

/// 交互会话日志记录器，将完整的交互过程持久化到文件，便于调试和回溯。
///
/// 日志文件格式为纯文本，每行包含时间戳、事件类型和内容，形如：
/// ```text
/// [2026-07-17 14:30:01] ▶ 用户: 你好
/// [2026-07-17 14:30:05] ● 思考: LLM 正在思考
/// [2026-07-17 14:30:12] ◂ 助手: 你好！有什么可以帮助你的？
/// ```
pub struct SessionLogger {
    file: File,
    path: PathBuf,
}

impl SessionLogger {
    /// 在指定目录下创建一个新的会话日志文件。
    /// 文件名格式: `.dev-assistant-session-{YYYYMMDD-HHMMSS}.log`
    pub fn create(working_dir: &Path) -> Result<Self, AppError> {
        let timestamp = Local::now().format("%Y%m%d-%H%M%S");
        let filename = format!(".dev-assistant-session-{}.log", timestamp);
        let path = working_dir.join(&filename);

        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            options.custom_flags(libc::O_CLOEXEC); // SECURITY: Auto-close on exec()
            options.mode(0o600); // SECURITY: Restrict log file to owner only
        }
        let file = options.open(&path).map_err(|e: std::io::Error| {
            AppError::Io(std::io::Error::new(
                e.kind(),
                format!("创建会话日志文件失败 ({}): {}", path.display(), e),
            ))
        })?;

        debug!(path = %path.display(), "Session log created");

        let mut logger = Self { file, path };

        if let Err(e) = writeln!(
            logger.file,
            "{} ════════════════════════════════════════════",
            Self::now()
        ) {
            warn!(error = %e, "Failed to write session log header");
        }
        if let Err(e) = writeln!(
            logger.file,
            "{}  Dev-Assistant 会话日志",
            Self::now()
        ) {
            warn!(error = %e, "Failed to write session log header");
        }
        if let Err(e) = writeln!(
            logger.file,
            "{} ════════════════════════════════════════════",
            Self::now()
        ) {
            warn!(error = %e, "Failed to write session log header");
        }
        logger.flush();

        Ok(logger)
    }

    /// 记录用户消息。
    pub fn log_user(&mut self, content: &str) {
        let sanitized = Self::sanitize(content);
        for line in sanitized.lines() {
            if let Err(e) = writeln!(self.file, "{} ▶ 用户: {}", Self::now(), line) {
                warn!(error = %e, "Failed to write user message to session log");
            }
        }
        self.flush();
    }

    /// 记录助手（LLM）的回复。
    pub fn log_assistant(&mut self, content: &str) {
        let sanitized = Self::sanitize(content);
        for line in sanitized.lines() {
            if let Err(e) = writeln!(self.file, "{} ◂ 助手: {}", Self::now(), line) {
                warn!(error = %e, "Failed to write assistant message to session log");
            }
        }
        self.flush();
    }

    /// 记录思考状态（LLM 正在处理中）。
    pub fn log_thinking(&mut self) {
        if let Err(e) = writeln!(self.file, "{} ● 思考: LLM 正在处理...", Self::now()) {
            warn!(error = %e, "Failed to write thinking status to session log");
        }
        self.flush();
    }

    /// 记录工具调用。
    #[allow(dead_code)]
    pub fn log_tool_call(&mut self, tool_name: &str, args: &str) {
        if let Err(e) = writeln!(
            self.file,
            "{} ⚙ 工具调用: {} (参数: {})",
            Self::now(),
            tool_name,
            args
        ) {
            warn!(error = %e, tool = %tool_name, "Failed to write tool call to session log");
        }
        self.flush();
    }

    /// 记录工具执行结果。
    #[allow(dead_code)]
    pub fn log_tool_result(&mut self, tool_name: &str, success: bool, summary: &str) {
        let status = if success { "✅" } else { "❌" };
        let sanitized = Self::sanitize(summary);
        for line in sanitized.lines() {
            if let Err(e) = writeln!(
                self.file,
                "{}   {} {}: {}",
                Self::now(),
                status,
                tool_name,
                line
            ) {
                warn!(error = %e, tool = %tool_name, "Failed to write tool result to session log");
            }
        }
        self.flush();
    }

    /// 记录一条状态消息（信息/成功/错误/警告）。
    pub fn log_status(&mut self, level: &str, msg: &str) {
        let icon = match level {
            "成功" => "✓",
            "错误" => "✗",
            "警告" => "⚠",
            "调试" => "◇",
            _ => "◆",
        };
        let sanitized = Self::sanitize(msg);
        for line in sanitized.lines() {
            if let Err(e) = writeln!(
                self.file,
                "{} {} [{}] {}",
                Self::now(),
                icon,
                level,
                line
            ) {
                warn!(error = %e, level = %level, "Failed to write status message to session log");
            }
        }
        self.flush();
    }

    /// 记录分隔线，用于标记不同轮次或阶段。
    #[allow(dead_code)]
    pub fn log_separator(&mut self, title: &str) {
        if let Err(e) = writeln!(self.file) {
            warn!(error = %e, "Failed to write separator to session log");
        }
        if let Err(e) = writeln!(
            self.file,
            "{} ── {} ──",
            Self::now(),
            title
        ) {
            warn!(error = %e, "Failed to write separator to session log");
        }
        if let Err(e) = writeln!(self.file) {
            warn!(error = %e, "Failed to write separator to session log");
        }
        self.flush();
    }

    /// 获取日志文件路径。
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 关闭日志文件，写入结束标记。
    pub fn close(&mut self) {
        if let Err(e) = writeln!(
            self.file,
            "{} ════════════════════════════════════════════",
            Self::now()
        ) {
            warn!(error = %e, "Failed to write session log footer");
        }
        if let Err(e) = writeln!(
            self.file,
            "{}  会话结束",
            Self::now()
        ) {
            warn!(error = %e, "Failed to write session log footer");
        }
        self.flush();
    }

    fn now() -> String {
        Local::now().format("[%Y-%m-%d %H:%M:%S]").to_string()
    }

    fn flush(&mut self) {
        let _ = self.file.flush();
    }

    /// 敏感信息脱敏处理，防止 API Key、密码、令牌等泄露到日志中。
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
}

impl Drop for SessionLogger {
    fn drop(&mut self) {
        self.close();
    }
}
