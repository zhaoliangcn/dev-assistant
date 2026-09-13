//! 长期记忆区（`.kb/memory/entries/`）。
//!
//! 与分层摘要（`.kb/summaries/{session_id}/`，按会话分桶、供恢复回溯）不同，
//! 长期记忆只存“值得跨会话携带”的精简条目：会话结束（finish）时把该会话的
//! 分层摘要晋升为一条条目；新会话启动时按预算注入，作为单条 system 消息进入上下文。
//!
//! 条目文件命名：`entry-{时间}-{pid}-{seq}.md`，字典序即时间序。
//! 数量上限 [`MAX_ENTRIES`]，超出的最旧条目在写入时清理（遗忘机制）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::agent::summary::strip_frontmatter;
use crate::agent::token_counter::TokenCounter;
use crate::utils::atomic_write::atomic_write;
use crate::utils::error::AppError;

/// 条目文件名前缀。
const ENTRY_PREFIX: &str = "entry-";
/// 长期记忆条目数量上限（超出即删除最旧条目）。
pub const MAX_ENTRIES: usize = 30;

/// 一条长期记忆条目。
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    /// 创建时间（`YYYY-MM-DD HH:MM`）
    pub created: String,
    /// 来源标注（如 `session:s20260913-201430-93102`）
    pub source: String,
    /// 正文（Markdown，已去除 frontmatter）
    pub body: String,
    /// 估算 token 数
    pub tokens: usize,
}

/// 长期记忆存储。`kb_root` 为 `.kb/` 目录。
pub struct MemoryStore {
    root: PathBuf,
}

/// 同一秒内多条条目的文件名去重序号。
static ENTRY_SEQ: AtomicUsize = AtomicUsize::new(0);

impl MemoryStore {
    pub fn new(kb_root: &Path) -> Self {
        let root = kb_root.join("memory");
        if let Err(e) = std::fs::create_dir_all(root.join("entries")) {
            tracing::warn!("创建长期记忆目录失败: {}: {}", root.display(), e);
        }
        Self { root }
    }

    fn entries_dir(&self) -> PathBuf {
        self.root.join("entries")
    }

    /// 长期区是否没有任何条目。
    pub fn is_empty(&self) -> bool {
        self.entry_files().is_empty()
    }

    /// 晋升一条长期记忆（原子写入并做数量修剪）。
    pub fn save_entry(&self, body: &str, source: &str) -> Result<(), AppError> {
        let body = body.trim();
        if body.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(self.entries_dir()).map_err(AppError::Io)?;
        let now = chrono::Local::now();
        let seq = ENTRY_SEQ.fetch_add(1, Ordering::Relaxed);
        let fname = format!(
            "{}{}-{}-{:04}.md",
            ENTRY_PREFIX,
            now.format("%Y%m%d-%H%M%S"),
            std::process::id(),
            seq
        );
        let frontmatter = format!(
            "---\ntype: long-term-memory\ncreated: \"{}\"\nsource: \"{}\"\n---\n",
            now.format("%Y-%m-%d %H:%M"),
            source.replace('"', "'")
        );
        atomic_write(
            &self.entries_dir().join(&fname),
            format!("{}\n{}", frontmatter, body).as_bytes(),
        )?;
        self.prune()?;
        tracing::info!(entry = %fname, source = %source, "已晋升长期记忆条目");
        Ok(())
    }

    /// 按 token 预算从最新到最旧挑选条目；单条装不下则跳过、继续试更小的。
    pub fn load_recent(&self, budget_tokens: usize) -> Vec<MemoryEntry> {
        let mut selected = Vec::new();
        let mut remaining = budget_tokens;
        for path in self.entry_files() {
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let entry = parse_entry(&content);
            if entry.tokens > remaining {
                continue;
            }
            remaining -= entry.tokens;
            selected.push(entry);
        }
        selected
    }

    /// 最新一条记忆的正文（用于晋升幂等判断）。
    pub fn latest_body(&self) -> Option<String> {
        let path = self.entry_files().into_iter().next()?;
        std::fs::read_to_string(path).ok().map(|c| parse_entry(&c).body)
    }

    /// 修剪超出 [`MAX_ENTRIES`] 的最旧条目，返回删除数量。
    fn prune(&self) -> Result<usize, AppError> {
        let files = self.entry_files();
        if files.len() <= MAX_ENTRIES {
            return Ok(0);
        }
        let mut removed = 0usize;
        for path in files.iter().skip(MAX_ENTRIES) {
            match std::fs::remove_file(path) {
                Ok(_) => removed += 1,
                Err(e) => tracing::warn!("删除过期长期记忆失败 {}: {}", path.display(), e),
            }
        }
        if removed > 0 {
            tracing::info!(removed, "长期记忆超出上限，已遗忘最旧条目");
        }
        Ok(removed)
    }

    /// 条目文件列表（按名称倒序 = 时间从新到旧）。
    fn entry_files(&self) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(self.entries_dir()) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| {
                        let n = n.to_string_lossy();
                        n.starts_with(ENTRY_PREFIX) && n.ends_with(".md")
                    })
                    .unwrap_or(false)
            })
            .collect();
        files.sort();
        files.reverse();
        files
    }
}

/// 把条目渲染为注入用的单条 system 消息文本（含时间、来源与冲突指引）。
pub fn render_entries(entries: &[MemoryEntry]) -> String {
    let mut out = format!(
        "【长期记忆 · 共 {} 条 · 新→旧】\n\
         以下是此前会话沉淀的记忆，可能已过时；与当前任务或当前代码冲突时，以当前信息为准。\n",
        entries.len()
    );
    for e in entries {
        out.push_str(&format!(
            "\n■ [{}] 来源：{}\n{}\n",
            if e.created.is_empty() { "未知时间" } else { &e.created },
            if e.source.is_empty() { "未知来源" } else { &e.source },
            e.body
        ));
    }
    out
}

fn parse_entry(content: &str) -> MemoryEntry {
    let body = strip_frontmatter(content).to_string();
    MemoryEntry {
        created: meta_field(content, "created"),
        source: meta_field(content, "source"),
        tokens: TokenCounter::estimate(&body),
        body,
    }
}

/// 从 frontmatter 取字符串字段（形如 `key: "value"`），只扫描文件头部 20 行。
fn meta_field(content: &str, key: &str) -> String {
    let prefix = format!("{}:", key);
    for line in content.lines().take(20) {
        if let Some(rest) = line.strip_prefix(prefix.as_str()) {
            return rest.trim().trim_matches('"').to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn save_and_load_newest_first() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());
        assert!(store.is_empty());
        store.save_entry("第一条记忆", "session:a").unwrap();
        store.save_entry("第二条记忆", "session:b").unwrap();
        let entries = store.load_recent(100_000);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].body, "第二条记忆", "最新条目在前");
        assert_eq!(entries[0].source, "session:b");
        assert!(!entries[0].created.is_empty(), "created 应从 frontmatter 解析");
    }

    #[test]
    fn budget_skips_oversized_but_keeps_smaller() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());
        store.save_entry("短", "session:small").unwrap();
        store
            .save_entry(&"很长很长很长很长很长很长很长很长很长很长的内容 ".repeat(200), "session:big")
            .unwrap();
        // 预算：先遇到 big（装不下跳过），再遇到 small（装入）
        let entries = store.load_recent(64);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, "session:small");
    }

    #[test]
    fn prune_removes_oldest_beyond_limit() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());
        for i in 0..(MAX_ENTRIES + 3) {
            store.save_entry(&format!("记忆 {}", i), "session:prune").unwrap();
        }
        assert_eq!(store.entry_files().len(), MAX_ENTRIES);
        let entries = store.load_recent(100_000);
        // 最旧的 3 条被遗忘，最新 MAX_ENTRIES 条仍在
        assert!(entries.iter().any(|e| e.body == format!("记忆 {}", MAX_ENTRIES + 2)));
        assert!(!entries.iter().any(|e| e.body == "记忆 0"));
    }

    #[test]
    fn empty_entry_not_saved() {
        let dir = tempdir().unwrap();
        let store = MemoryStore::new(dir.path());
        store.save_entry("   ", "session:x").unwrap();
        assert!(store.is_empty());
    }
}
