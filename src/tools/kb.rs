//! KnowledgeBase 工具。
//!
//! 提供 `kb_store` 和 `kb_query` 两个工具，用于读写项目知识库。
//!
//! KnowledgeBase 存储在 `.kb/` 目录下，使用 Markdown 文件 + YAML frontmatter 格式。
//! 索引文件 `.kb/index.json` 维护所有条目的元数据，支持标签和关键词检索。
//!
//! # 目录结构
//!
//! ```text
//! .kb/
//! ├── index.json
//! ├── decisions/
//! │   └── ADR-001-use-ecs.md
//! ├── interfaces/
//! │   └── renderer-api.md
//! ├── summaries/
//! ├── issues/
//! ├── progress/
//! └── templates/
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::{ToolArgs, ToolContext, ToolDefinition, ToolResult};
use crate::utils::error::AppError;

/// 查询结果中单条内容的最大字符数。
const MAX_CONTENT_CHARS: usize = 2000;

// ---------------------------------------------------------------------------
// 索引数据结构
// ---------------------------------------------------------------------------

/// KB 索引文件结构。
#[derive(Debug, Serialize, Deserialize)]
pub struct KbIndex {
    pub version: u32,
    pub updated: String,
    pub entries: HashMap<String, KbIndexEntry>,
}

impl Default for KbIndex {
    fn default() -> Self {
        Self {
            version: 1,
            updated: Utc::now().to_rfc3339(),
            entries: HashMap::new(),
        }
    }
}

/// 索引中的单个条目。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct KbIndexEntry {
    /// 相对于 `.kb/` 的文件路径
    pub path: String,
    /// 条目类型：decision / interface / summary / issue
    #[serde(rename = "type")]
    pub entry_type: String,
    /// 标题
    pub title: String,
    /// 标签列表
    #[serde(default)]
    pub tags: Vec<String>,
    /// 状态：proposed / accepted / deprecated / superseded / draft / completed
    #[serde(default)]
    pub status: String,
    /// 是否已归档（dream 遗忘机制置位，默认 active）。
    /// `kb_query` 默认过滤归档条目，传 `include_archived=true` 可检索。
    #[serde(default)]
    pub archived: bool,
    /// 关联条目 ID 列表
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relates_to: Option<Vec<String>>,
    /// 依赖的条目 ID 列表
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<String>>,
    /// 替代的条目 ID 列表
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<Vec<String>>,
    /// 创建者角色
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// 创建时间
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// 更新时间
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    /// 被 kb_query 命中的次数（Dream 遗忘阶段用作健康分激励）
    ///
    /// **注意**：自查询统计 sidecar 上线后，此字段的真相源已迁至
    /// `.kb/query-stats.json`（见 [`load_query_stats`]）。index.json 中的该值
    /// 不再被 `kb_query`/`kb_store` 维护，仅作历史字段保留；遗忘阶段会从
    /// sidecar 注水后再用于 `compute_health`。
    #[deprecated(since = "0.2.0", note = "查询统计已迁移至 .kb/query-stats.json sidecar，此字段仅保留以兼容旧 index.json 反序列化")]
    #[serde(default)]
    pub query_count: u64,
    /// 最近一次被 kb_query 命中的时间
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_query_at: Option<String>,
}

// ---------------------------------------------------------------------------
// 查询统计 sidecar
// ---------------------------------------------------------------------------

/// 单个 KB 条目的查询命中统计，独立存放在 `.kb/query-stats.json`。
///
/// 与 index.json 分离的目的：`kb_query` 高频调用时只重写这个小文件，
/// 而非每次都整体序列化整个 index.json（O(n)）。遗忘阶段读取本 sidecar
/// 注水到 `KbIndexEntry` 后再计算健康分。
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub(crate) struct QueryStats {
    /// 被 kb_query 命中的次数
    #[serde(default)]
    pub query_count: u64,
    /// 最近一次被命中的时间
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_query_at: Option<String>,
}

/// 查询统计 sidecar 文件路径：`.kb/query-stats.json`。
pub(crate) fn query_stats_path(kb_root: &Path) -> PathBuf {
    kb_root.join("query-stats.json")
}

/// 加载查询统计 sidecar。
///
/// sidecar 不存在或损坏时，从 `baseline`（index.json 的条目）一次性迁移现有
/// `query_count`/`last_query_at`，并尽力写回 sidecar。迁移后 sidecar 即为查询
/// 统计的唯一真相源，index.json 中的对应字段不再被维护。
#[allow(deprecated)] // 仅迁移代码读取 index.json 中已弃用的 `query_count` 旧值
pub(crate) fn load_query_stats(kb_root: &Path, baseline: &KbIndex) -> HashMap<String, QueryStats> {
    let path = query_stats_path(kb_root);
    match fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => {
            match serde_json::from_str::<HashMap<String, QueryStats>>(&content) {
                Ok(stats) => return stats,
                Err(e) => debug!(path = %path.display(), error = %e, "query-stats sidecar 解析失败，从索引迁移重建"),
            }
        }
        Ok(_) => debug!(path = %path.display(), "query-stats sidecar 为空，从索引迁移重建"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => debug!(path = %path.display(), error = %e, "读取 query-stats sidecar 失败，从索引迁移重建"),
    }

    // 迁移：从 baseline 拷贝非零统计（避免无意义写入空映射）
    let mut stats: HashMap<String, QueryStats> = HashMap::new();
    for (id, e) in &baseline.entries {
        if e.query_count > 0 || e.last_query_at.is_some() {
            stats.insert(id.clone(), QueryStats {
                query_count: e.query_count,
                last_query_at: e.last_query_at.clone(),
            });
        }
    }
    if let Err(e) = save_query_stats(kb_root, &stats) {
        debug!(error = %e, "迁移查询统计 sidecar 写入失败，下次将再次尝试迁移");
    }
    stats
}

/// 写回查询统计 sidecar。
pub(crate) fn save_query_stats(
    kb_root: &Path,
    stats: &HashMap<String, QueryStats>,
) -> Result<(), AppError> {
    let path = query_stats_path(kb_root);
    let json = serde_json::to_string_pretty(stats).map_err(AppError::Json)?;
    fs::write(&path, json).map_err(|e| {
        AppError::Io(std::io::Error::other(format!("写入 query-stats sidecar 失败: {}", e)))
    })
}

/// 查询结果条目（包含可选的正文内容）。
#[derive(Debug, Clone, Serialize)]
pub struct KbQueryResult {
    /// 条目 ID
    pub id: String,
    /// 文件路径
    pub path: String,
    /// 条目类型
    pub entry_type: String,
    /// 标题
    pub title: String,
    /// 标签
    pub tags: Vec<String>,
    /// 状态
    pub status: String,
    /// 匹配分数
    pub score: i32,
    /// 可选的正文内容
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

// ---------------------------------------------------------------------------
// 工具定义
// ---------------------------------------------------------------------------

/// `kb_store` 工具定义。
///
/// 创建或更新 KnowledgeBase 条目。用于记录架构决策、模块接口定义、问题追踪等。
pub fn kb_store_tool() -> ToolDefinition {
    ToolDefinition {
        name: "kb_store".to_string(),
        description: "Create or update a KB entry. For architecture decisions, interfaces, issues. Content must include YAML frontmatter (---).".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Entry path relative to .kb/, e.g. 'decisions/ADR-001-use-ecs.md'"
                },
                "content": {
                    "type": "string",
                    "description": "Full Markdown content (including YAML frontmatter)"
                },
                "update_index": {
                    "type": "boolean",
                    "description": "Whether to update index.json automatically",
                    "default": true
                }
            },
            "required": ["path", "content"]
        }),
        skip_security: false,
        handler: Box::new(kb_store_handler),
    }
}

/// `kb_query` 工具定义。
///
/// 检索 KnowledgeBase 条目。支持按标签、类型、关键词过滤。
pub fn kb_query_tool() -> ToolDefinition {
    ToolDefinition {
        name: "kb_query".to_string(),
        description: "Search KnowledgeBase entries. Supports filtering by keywords, type, and tags.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search keywords"
                },
                "type": {
                    "type": "string",
                    "enum": ["decision", "interface", "summary", "issue", "any"],
                    "description": "Entry type filter",
                    "default": "any"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Tag filter (matches if any tag matches)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return",
                    "default": 5
                },
                "include_content": {
                    "type": "boolean",
                    "description": "Whether to include entry content",
                    "default": false
                },
                "include_archived": {
                    "type": "boolean",
                    "description": "Whether to include archived entries (default false, archived entries are filtered out)",
                    "default": false
                }
            },
            "required": []
        }),
        skip_security: false,
        handler: Box::new(kb_query_handler),
    }
}

// ---------------------------------------------------------------------------
// 工具处理函数
// ---------------------------------------------------------------------------

/// `kb_store` 处理函数。
///
/// 1. 验证路径安全性（防止路径遍历）
/// 2. 验证文件扩展名为 `.md`
/// 3. 解析 frontmatter 获取元数据
/// 4. 将内容写入 `.kb/{path}`
/// 5. 如果 `update_index` 为 true，更新 index.json
fn kb_store_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    let path = args.arguments["path"]
        .as_str()
        .ok_or_else(|| AppError::Llm("kb_store: 'path' is required".to_string()))?
        .to_string();

    // SECURITY: 验证路径不包含 `..` 遍历
    if path.contains("..") {
        return Err(AppError::Security(format!(
            "Path traversal detected in KB path: '{}'", path
        )));
    }

    // SECURITY: 验证路径以 `.md` 结尾
    if !path.ends_with(".md") {
        return Err(AppError::Llm(format!(
            "kb_store: path must end with '.md', got: '{}'", path
        )));
    }

    // 规范化路径：去除开头的 ".kb/" 或 ".kb\\" 前缀，防止 `kb_root.join(path)`
    // 把路径重复拼成 `.kb/.kb/...`。LLM 有时会传入完整路径 ".kb/decisions/foo.md"
    // 而非相对路径 "decisions/foo.md"，必须统一为相对路径。
    let path = path
        .trim_start_matches(".kb/")
        .trim_start_matches(".kb\\");

    let content = args.arguments["content"]
        .as_str()
        .ok_or_else(|| AppError::Llm("kb_store: 'content' is required".to_string()))?;

    let update_index = args.arguments["update_index"]
        .as_bool()
        .unwrap_or(true);

    // SECURITY: 确保解析后的路径仍在 kb_root 目录下
    // 先解析工作目录的真实路径（处理 /var → /private/var 等 symlink 情况）
    let working_dir_canonical = context.working_dir.canonicalize()
        .unwrap_or_else(|_| context.working_dir.clone());
    let kb_root = working_dir_canonical.join(".kb");
    let file_path = kb_root.join(path);

    // 检查 file_path 是否在 kb_root 内
    let check_path = if file_path.exists() {
        file_path.canonicalize().unwrap_or_else(|_| file_path.clone())
    } else if let Some(parent) = file_path.parent() {
        if parent.exists() {
            parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf())
                .join(file_path.file_name().unwrap())
        } else {
            file_path.clone()
        }
    } else {
        file_path.clone()
    };

    let kb_root_str = kb_root.to_string_lossy().to_string();
    let check_path_str = check_path.to_string_lossy().to_string();
    if !check_path_str.starts_with(&kb_root_str) {
        return Err(AppError::Security(format!(
            "Path traversal detected: '{}' is outside KB root '{}'",
            check_path_str, kb_root_str
        )));
    }

    // 确保父目录存在
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            AppError::Io(std::io::Error::other(
                format!("Failed to create KB directory '{}': {}", parent.display(), e),
            ))
        })?;
    }

    // 写入文件
    fs::write(&file_path, content).map_err(|e| {
        AppError::Io(std::io::Error::other(
            format!("Failed to write KB entry '{}': {}", file_path.display(), e),
        ))
    })?;

    debug!(path = %file_path.display(), "KB entry written");

    // 更新索引
    if update_index {
        update_index_entry(&kb_root, path, content)?;
    }

    Ok(ToolResult {
        success: true,
        security_evaluation: None,
        restart_requested: false,
                error_category: None,
        content: format!(
            "[kb_store] ✅ 条目已保存: {}\n路径: {}",
            path,
            file_path.display()
        ),
    })
}

/// `kb_query` 处理函数。
///
/// 1. 加载 index.json
/// 2. 根据查询参数搜索条目
/// 3. 如果 `include_content` 为 true，也加载正文内容
fn kb_query_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    let query = args.arguments["query"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let type_filter = args.arguments["type"]
        .as_str()
        .map(|s| s.to_string());

    let tag_filter: Option<Vec<String>> = args.arguments["tags"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .filter(|v: &Vec<String>| !v.is_empty());

    let max_results = args.arguments["max_results"]
        .as_u64()
        .map(|n| n as usize)
        .unwrap_or(5);

    let include_content = args.arguments["include_content"]
        .as_bool()
        .unwrap_or(false);

    let include_archived = args.arguments["include_archived"]
        .as_bool()
        .unwrap_or(false);

    let kb_root = context.working_dir.join(".kb");
    let index_path = kb_root.join("index.json");

    // 检查索引文件是否存在
    if !index_path.exists() {
        return Ok(ToolResult {
            success: true,
            security_evaluation: None,
            restart_requested: false,
                error_category: None,
            content: "KnowledgeBase 为空（.kb/index.json 不存在）。请先使用 kb_store 创建条目。".to_string(),
        });
    }

    // 加载索引
    let index_content = fs::read_to_string(&index_path).map_err(|e| {
        AppError::Io(std::io::Error::other(
            format!("Failed to read KB index '{}': {}", index_path.display(), e),
        ))
    })?;

    let index: KbIndex = serde_json::from_str(&index_content).map_err(|e| {
        AppError::Config(format!("Failed to parse KB index: {}", e))
    })?;

    // 搜索
    let results = search_entries(
        &index,
        &query,
        type_filter.as_deref(),
        tag_filter.as_deref(),
        max_results,
        include_archived,
    );

    // 查询命中追踪：仅当存在关键词查询时记录（避免纯标签/类型浏览写盘）。
    // 统计写入独立 sidecar（query-stats.json），避免每次 kb_query 都整体
    // 重写 index.json（KB 增长后 O(n) IO）。写入失败仅记日志，不影响查询结果。
    if !query.is_empty() && !results.is_empty() {
        let mut stats = load_query_stats(&kb_root, &index);
        let now = Utc::now().to_rfc3339();
        for result in &results {
            let s = stats.entry(result.id.clone()).or_default();
            s.query_count = s.query_count.saturating_add(1);
            s.last_query_at = Some(now.clone());
        }
        if let Err(e) = save_query_stats(&kb_root, &stats) {
            debug!(error = %e, "写入查询统计 sidecar 失败，本次命中未持久化");
        }
    }

    // 如果需要内容，加载每个条目的正文
    let mut results_with_content = Vec::new();
    for mut result in results {
        if include_content {
            let entry_path = kb_root.join(&result.path);
            if entry_path.exists() {
                if let Ok(content) = fs::read_to_string(&entry_path) {
                    // 提取 frontmatter 之后的正文
                    let body = crate::utils::frontmatter::extract_body(&content);
                    if !body.is_empty() {
                        result.content = Some(body);
                    } else {
                        result.content = Some(content);
                    }
                }
            }
        }
        results_with_content.push(result);
    }

    // 格式化输出
    let output = format_query_results(&results_with_content, include_content);

    Ok(ToolResult {
        success: true,
        security_evaluation: None,
        restart_requested: false,
                error_category: None,
        content: output,
    })
}

// ---------------------------------------------------------------------------
// 索引管理
// ---------------------------------------------------------------------------

/// 更新索引文件中的条目。
///
/// 从条目的 frontmatter 中提取元数据，更新或添加到 index.json。
/// `pub(crate)`：供 dream 模块（consolidate 阶段写入新条目后纳入索引）复用。
pub(crate) fn update_index_entry(kb_root: &Path, entry_path: &str, content: &str) -> Result<(), AppError> {
    let index_path = kb_root.join("index.json");

    // 加载现有索引，或创建新索引
    let mut index: KbIndex = if index_path.exists() {
        let content = fs::read_to_string(&index_path).map_err(|e| {
            AppError::Io(std::io::Error::other(
                format!("Failed to read KB index: {}", e),
            ))
        })?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        KbIndex::default()
    };

    // 解析 frontmatter
    let fm = match crate::utils::frontmatter::parse_frontmatter(content) {
        Ok(fm) => fm,
        Err(e) => {
            debug!(error = %e, "Failed to parse frontmatter for index update, using defaults");
            HashMap::new()
        }
    };

    // 从 frontmatter 或路径中提取 ID
    let id = fm
        .get("id")
        .cloned()
        .unwrap_or_else(|| {
            // 从文件名中提取 ID（不含扩展名）
            Path::new(entry_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(entry_path)
                .to_string()
        });

    // 从 frontmatter 中提取标签
    let tags: Vec<String> = fm
        .get("tags")
        .map(|t| {
            t.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // 解析 relates_to 字段（逗号分隔）
    let relates_to: Option<Vec<String>> = fm.get("relates_to").map(|r| {
        r.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }).filter(|v: &Vec<String>| !v.is_empty());

    // 解析 depends_on 字段
    let depends_on: Option<Vec<String>> = fm.get("depends_on").map(|r| {
        r.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }).filter(|v: &Vec<String>| !v.is_empty());

    // 解析 supersedes 字段
    let supersedes: Option<Vec<String>> = fm.get("supersedes").map(|r| {
        r.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }).filter(|v: &Vec<String>| !v.is_empty());

    let now = Utc::now().to_rfc3339();

    // 解析 archived 字段（frontmatter 可写 archived: true，默认 false）
    let archived = fm.get("archived").map(|s| s.trim().eq_ignore_ascii_case("true")).unwrap_or(false);

    // `query_count` 已弃用（真相源为 sidecar），此处仅设默认值以满足结构体初始化。
    #[allow(deprecated)]
    let entry = KbIndexEntry {
        path: entry_path.to_string(),
        entry_type: fm.get("type").cloned().unwrap_or_else(|| "unknown".to_string()),
        title: fm.get("title").cloned().unwrap_or_else(|| id.clone()),
        tags,
        status: fm.get("status").cloned().unwrap_or_else(|| "draft".to_string()),
        archived,
        relates_to,
        depends_on,
        supersedes,
        author: fm.get("author").cloned(),
        created: fm.get("created").cloned(),
        updated: Some(now.clone()),
        query_count: 0,
        last_query_at: None,
    };

    // 增量优化：若条目已存在且元数据完全一致（仅 updated 时间戳不同），
    // 跳过全量序列化与写盘，避免 KB 增长后每次 kb_store 都产生 O(n) IO。
    if let Some(existing) = index.entries.get(&id) {
        // 查询统计由 query-stats.json sidecar 管理，索引不再继承/维护，
        // 否则会与 sidecar 形成双真相源。
        let unchanged = existing.path == entry.path
            && existing.entry_type == entry.entry_type
            && existing.title == entry.title
            && existing.tags == entry.tags
            && existing.status == entry.status
            && existing.archived == entry.archived
            && existing.relates_to == entry.relates_to
            && existing.depends_on == entry.depends_on
            && existing.supersedes == entry.supersedes
            && existing.author == entry.author
            && existing.created == entry.created;
        if unchanged {
            debug!(id = %id, "KB 索引条目无变化，跳过写盘");
            return Ok(());
        }
    }

    index.entries.insert(id, entry);
    index.updated = now;

    // 写入索引文件
    let index_json = serde_json::to_string_pretty(&index).map_err(|e| {
        AppError::Json(e)
    })?;

    fs::write(&index_path, &index_json).map_err(|e| {
        AppError::Io(std::io::Error::other(
            format!("Failed to write KB index: {}", e),
        ))
    })?;

    debug!(path = %index_path.display(), entries = %index.entries.len(), "KB index updated");
    Ok(())
}

// ---------------------------------------------------------------------------
// 检索算法
// ---------------------------------------------------------------------------

/// 判断字符是否为 CJK 字符（中文/日文/韩文）。
fn is_cjk_char(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}' |
        '\u{3400}'..='\u{4DBF}' |
        '\u{3000}'..='\u{303F}' |
        '\u{3040}'..='\u{309F}' |
        '\u{30A0}'..='\u{30FF}' |
        '\u{AC00}'..='\u{D7AF}'
    )
}

/// 将文本拆分为可搜索的 token 列表。
///
/// 规则：
/// - 连续 ASCII 字母/数字累积为一个单词 token（转小写）
/// - 驼峰边界拆分：`SyncToolCache` → `sync` `tool` `cache`
/// - CJK 字符按单字切分（中文无空格分词，按字匹配最稳）
/// - 其他字符（空格、标点、下划线等）作为分隔符
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut ascii_word = String::new();

    let flush = |tokens: &mut Vec<String>, word: &mut String| {
        if !word.is_empty() {
            tokens.push(word.clone());
            word.clear();
        }
    };

    for c in text.chars() {
        if is_cjk_char(c) {
            flush(&mut tokens, &mut ascii_word);
            tokens.push(c.to_string());
        } else if c.is_alphanumeric() {
            // 驼峰边界：大写字母前一个字符是小写/数字时拆分（SyncTool → Sync + Tool）
            if c.is_uppercase()
                && !ascii_word.is_empty()
                && ascii_word
                    .chars()
                    .last()
                    .map(|l| l.is_lowercase() || l.is_ascii_digit())
                    .unwrap_or(false)
            {
                flush(&mut tokens, &mut ascii_word);
            }
            ascii_word.push(c.to_lowercase().next().unwrap_or(c));
        } else {
            flush(&mut tokens, &mut ascii_word);
        }
    }
    flush(&mut tokens, &mut ascii_word);

    tokens
}

/// 判断查询 token 是否命中条目 token（精确匹配或前缀匹配）。
///
/// 前缀匹配支持部分查询（如 "arch" 命中 "architecture"），
/// 但只做单向（查询词是完整词的前缀），避免短查询词误匹配海量条目。
fn token_hit(query_token: &str, entry_token: &str) -> bool {
    entry_token == query_token || entry_token.starts_with(query_token)
}

/// 计算单个条目在给定字段 token 上的命中得分。
///
/// 精确命中得 `exact` 分，前缀命中得 `prefix` 分（降权）。
fn field_score(query_tokens: &[String], field_tokens: &[String], exact: i32, prefix: i32) -> i32 {
    let mut score = 0i32;
    for qt in query_tokens {
        for et in field_tokens {
            if et == qt {
                score += exact;
            } else if token_hit(qt, et) {
                score += prefix;
            }
        }
    }
    score
}

/// 在索引中搜索匹配的条目。
///
/// 使用分词 + 归一化 + 前缀模糊匹配：
/// - 中英文分词（英文按单词+驼峰拆分，中文按单字）
/// - 标题/ID/路径/标签字段加权打分（标题+5、ID+3、路径+2、标签+2）
/// - 前缀匹配支持部分关键词（如 "arch" 命中 "architecture"）
/// 返回按分数降序排列的结果。
fn search_entries(
    index: &KbIndex,
    query: &str,
    type_filter: Option<&str>,
    tag_filter: Option<&[String]>,
    max_results: usize,
    include_archived: bool,
) -> Vec<KbQueryResult> {
    let query_tokens = tokenize(query);
    let mut scored: Vec<(i32, &str, &KbIndexEntry)> = Vec::new();

    for (id, entry) in &index.entries {
        // 0. 归档过滤：默认排除归档条目（dream 遗忘机制的产物）
        if entry.archived && !include_archived {
            continue;
        }

        let mut score = 0i32;

        // 1. 类型过滤
        if let Some(t) = type_filter {
            if t != "any" && entry.entry_type != t {
                continue;
            }
        }

        // 2. 标签过滤（精确匹配）
        if let Some(tags) = tag_filter {
            if !tags.iter().any(|t| entry.tags.contains(t)) {
                continue;
            }
            score += 10;
        }

        // 3. 关键词匹配（分词 + 归一化 + 前缀模糊）
        if !query_tokens.is_empty() {
            let title_tokens = tokenize(&entry.title);
            let id_tokens = tokenize(id);
            let path_tokens = tokenize(&entry.path);
            let tag_tokens: Vec<String> = entry
                .tags
                .iter()
                .flat_map(|t| tokenize(t))
                .collect();

            score += field_score(&query_tokens, &title_tokens, 5, 3); // 标题命中
            score += field_score(&query_tokens, &id_tokens, 3, 2); // ID 命中
            score += field_score(&query_tokens, &path_tokens, 2, 1); // 路径命中
            score += field_score(&query_tokens, &tag_tokens, 2, 1); // 标签命中
        }

        // 4. 状态优先级
        if entry.status == "accepted" || entry.status == "completed" {
            score += 2;
        }

        // 无查询条件时给一个基础分，确保所有条目都被返回
        if query.is_empty() && tag_filter.is_none() {
            score = 1;
        }

        scored.push((score, id.as_str(), entry));
    }

    // 按分数降序排列
    scored.sort_by(|a, b| b.0.cmp(&a.0));

    // 取 top N
    scored
        .into_iter()
        .take(max_results)
        .map(|(score, id, entry)| KbQueryResult {
            id: id.to_string(),
            path: entry.path.clone(),
            entry_type: entry.entry_type.clone(),
            title: entry.title.clone(),
            tags: entry.tags.clone(),
            status: entry.status.clone(),
            score,
            content: None,
        })
        .collect()
}

/// 格式化查询结果为 Markdown 文本。
fn format_query_results(results: &[KbQueryResult], include_content: bool) -> String {
    if results.is_empty() {
        return "未找到匹配的 KnowledgeBase 条目。".to_string();
    }

    let mut output = format!("找到 {} 个匹配条目：\n\n", results.len());

    for (i, result) in results.iter().enumerate() {
        output.push_str(&format!(
            "{}. **{}** — {}\n",
            i + 1,
            result.id,
            result.title
        ));
        output.push_str(&format!(
            "   - 类型: {} | 状态: {} | 分数: {}\n",
            result.entry_type, result.status, result.score
        ));
        output.push_str(&format!("   - 路径: {}\n", result.path));

        if !result.tags.is_empty() {
            output.push_str(&format!("   - 标签: {}\n", result.tags.join(", ")));
        }

        if include_content {
            if let Some(ref content) = result.content {
                // 截断过长内容
                let truncated = if content.len() > MAX_CONTENT_CHARS {
                    let safe_boundary = content.floor_char_boundary(MAX_CONTENT_CHARS);
                    format!("{}...\n   [内容已截断，共 {} 字符]",
                        &content[..safe_boundary],
                        content.len())
                } else {
                    content.clone()
                };
                output.push_str(&format!("   - 内容: {}\n", truncated));
            } else {
                output.push_str("   - 内容: (文件不存在或无法读取)\n");
            }
        }
    }

    output
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn kb_test_context(working_dir: &Path) -> ToolContext {
        ToolContext {
            working_dir: working_dir.to_path_buf(),
            resources: None,
            cache: None,
            hooks: None,
        }
    }

    fn make_args(map: serde_json::Value) -> ToolArgs {
        ToolArgs {
            arguments: map,
        }
    }

    #[test]
    fn kb_store_creates_file_and_index() {
        let dir = tempdir().unwrap();
        let ctx = kb_test_context(dir.path());

        let content = "---\nid: TEST-001\ntype: decision\ntitle: Test Decision\ntags: [test, demo]\nstatus: proposed\n---\n# Test\n\nThis is a test.";
        let args = make_args(serde_json::json!({
            "path": "decisions/TEST-001.md",
            "content": content,
            "update_index": true
        }));

        let result = kb_store_handler(&args, &ctx).unwrap();
        assert!(result.success);

        // 验证文件已创建
        let file_path = dir.path().join(".kb/decisions/TEST-001.md");
        assert!(file_path.exists(), "KB entry file should exist");

        // 验证索引已更新
        let index_path = dir.path().join(".kb/index.json");
        assert!(index_path.exists(), "KB index file should exist");

        let index_content = fs::read_to_string(&index_path).unwrap();
        let index: KbIndex = serde_json::from_str(&index_content).unwrap();
        assert!(index.entries.contains_key("TEST-001"));
        assert_eq!(index.entries["TEST-001"].title, "Test Decision");
        assert_eq!(index.entries["TEST-001"].entry_type, "decision");
        assert!(index.entries["TEST-001"].tags.contains(&"test".to_string()));
    }

    #[test]
    fn kb_store_without_index_update() {
        let dir = tempdir().unwrap();
        let ctx = kb_test_context(dir.path());

        let content = "---\nid: TEST-002\ntype: decision\ntitle: No Index\n---\nBody";
        let args = make_args(serde_json::json!({
            "path": "decisions/TEST-002.md",
            "content": content,
            "update_index": false
        }));

        let result = kb_store_handler(&args, &ctx).unwrap();
        assert!(result.success);

        // 验证文件已创建
        let file_path = dir.path().join(".kb/decisions/TEST-002.md");
        assert!(file_path.exists());

        // 验证索引未创建
        let index_path = dir.path().join(".kb/index.json");
        assert!(!index_path.exists(), "Index should not be created when update_index=false");
    }

    #[test]
    fn kb_store_multiple_entries_updates_index() {
        let dir = tempdir().unwrap();
        let ctx = kb_test_context(dir.path());

        // 第一条
        let args1 = make_args(serde_json::json!({
            "path": "decisions/ADR-001.md",
            "content": "---\nid: ADR-001\ntype: decision\ntitle: First Decision\ntags: [arch]\n---\nBody 1",
            "update_index": true
        }));
        kb_store_handler(&args1, &ctx).unwrap();

        // 第二条
        let args2 = make_args(serde_json::json!({
            "path": "decisions/ADR-002.md",
            "content": "---\nid: ADR-002\ntype: decision\ntitle: Second Decision\ntags: [api]\n---\nBody 2",
            "update_index": true
        }));
        kb_store_handler(&args2, &ctx).unwrap();

        // 验证索引包含两条
        let index_path = dir.path().join(".kb/index.json");
        let index_content = fs::read_to_string(&index_path).unwrap();
        let index: KbIndex = serde_json::from_str(&index_content).unwrap();
        assert_eq!(index.entries.len(), 2);
        assert!(index.entries.contains_key("ADR-001"));
        assert!(index.entries.contains_key("ADR-002"));
    }

    #[test]
    fn kb_store_identical_entry_skips_index_write() {
        let dir = tempdir().unwrap();
        let ctx = kb_test_context(dir.path());

        // 首次写入
        let args = make_args(serde_json::json!({
            "path": "decisions/ADR-100.md",
            "content": "---\nid: ADR-100\ntype: decision\ntitle: Skip Write\ntags: [test]\nstatus: proposed\n---\nBody",
            "update_index": true
        }));
        kb_store_handler(&args, &ctx).unwrap();

        let index_path = dir.path().join(".kb/index.json");
        let first = fs::read_to_string(&index_path).unwrap();

        // 等一小段时间，确保 updated 时间戳若被写入必然不同
        std::thread::sleep(std::time::Duration::from_millis(20));

        // 相同内容重复写入：元数据无变化，应跳过写盘
        let args2 = make_args(serde_json::json!({
            "path": "decisions/ADR-100.md",
            "content": "---\nid: ADR-100\ntype: decision\ntitle: Skip Write\ntags: [test]\nstatus: proposed\n---\nBody",
            "update_index": true
        }));
        kb_store_handler(&args2, &ctx).unwrap();

        let second = fs::read_to_string(&index_path).unwrap();
        assert_eq!(first, second, "无变化条目不应重写索引文件");

        // 内容变化后应更新索引
        let args3 = make_args(serde_json::json!({
            "path": "decisions/ADR-100.md",
            "content": "---\nid: ADR-100\ntype: decision\ntitle: Skip Write Updated\ntags: [test]\nstatus: accepted\n---\nBody 2",
            "update_index": true
        }));
        kb_store_handler(&args3, &ctx).unwrap();
        let third = fs::read_to_string(&index_path).unwrap();
        assert_ne!(first, third, "内容变化后应重写索引文件");
        let index: KbIndex = serde_json::from_str(&third).unwrap();
        assert_eq!(index.entries["ADR-100"].title, "Skip Write Updated");
        assert_eq!(index.entries["ADR-100"].status, "accepted");
    }

    #[test]
    fn kb_store_id_from_filename_when_no_id_in_frontmatter() {
        let dir = tempdir().unwrap();
        let ctx = kb_test_context(dir.path());

        let content = "---\ntype: interface\ntitle: My Interface\n---\nBody";
        let args = make_args(serde_json::json!({
            "path": "interfaces/my-interface.md",
            "content": content,
            "update_index": true
        }));

        kb_store_handler(&args, &ctx).unwrap();

        let index_path = dir.path().join(".kb/index.json");
        let index_content = fs::read_to_string(&index_path).unwrap();
        let index: KbIndex = serde_json::from_str(&index_content).unwrap();
        // 使用文件名作为 ID
        assert!(index.entries.contains_key("my-interface"));
    }

    #[test]
    fn kb_query_empty_index_returns_message() {
        let dir = tempdir().unwrap();
        let ctx = kb_test_context(dir.path());

        let args = make_args(serde_json::json!({
            "query": "test",
            "type": "any"
        }));

        let result = kb_query_handler(&args, &ctx).unwrap();
        assert!(result.success);
        assert!(result.content.contains("为空"));
    }

    #[test]
    fn kb_query_finds_by_keyword() {
        let dir = tempdir().unwrap();
        let ctx = kb_test_context(dir.path());

        // 先创建一些条目
        store_test_entry(&ctx, "decisions/ADR-001.md", "ADR-001", "decision", "Use ECS Architecture", &["architecture", "ecs"]);
        store_test_entry(&ctx, "decisions/ADR-002.md", "ADR-002", "decision", "Choose wgpu", &["rendering", "graphics"]);
        store_test_entry(&ctx, "interfaces/renderer-api.md", "interface-renderer", "interface", "Renderer API", &["rendering", "api"]);

        // 查询 "ECS"
        let args = make_args(serde_json::json!({
            "query": "ECS",
            "max_results": 10
        }));

        let result = kb_query_handler(&args, &ctx).unwrap();
        assert!(result.success);
        assert!(result.content.contains("ADR-001"), "Should find ECS decision");
        assert!(result.content.contains("Use ECS Architecture"), "Should contain title");
    }

    #[test]
    fn kb_query_filters_by_type() {
        let dir = tempdir().unwrap();
        let ctx = kb_test_context(dir.path());

        store_test_entry(&ctx, "decisions/ADR-001.md", "ADR-001", "decision", "Decision 1", &["test"]);
        store_test_entry(&ctx, "interfaces/api.md", "api-1", "interface", "Interface 1", &["test"]);

        // 只查 decision 类型
        let args = make_args(serde_json::json!({
            "type": "decision",
            "max_results": 10
        }));

        let result = kb_query_handler(&args, &ctx).unwrap();
        assert!(result.content.contains("ADR-001"), "Should find decision");
        assert!(!result.content.contains("api-1"), "Should not find interface");
    }

    #[test]
    fn kb_query_filters_by_tags() {
        let dir = tempdir().unwrap();
        let ctx = kb_test_context(dir.path());

        store_test_entry(&ctx, "decisions/ADR-001.md", "ADR-001", "decision", "ECS Decision", &["architecture", "ecs"]);
        store_test_entry(&ctx, "decisions/ADR-002.md", "ADR-002", "decision", "Rendering Decision", &["rendering", "graphics"]);

        // 按标签 "rendering" 过滤
        let args = make_args(serde_json::json!({
            "tags": ["rendering"],
            "max_results": 10
        }));

        let result = kb_query_handler(&args, &ctx).unwrap();
        assert!(result.content.contains("ADR-002"), "Should find rendering entry");
        assert!(!result.content.contains("ADR-001"), "Should not find architecture entry");
    }

    #[test]
    fn kb_query_returns_content_when_requested() {
        let dir = tempdir().unwrap();
        let ctx = kb_test_context(dir.path());

        store_test_entry(&ctx, "decisions/ADR-001.md", "ADR-001", "decision", "Test Decision", &["test"]);

        let args = make_args(serde_json::json!({
            "query": "Test",
            "include_content": true,
            "max_results": 10
        }));

        let result = kb_query_handler(&args, &ctx).unwrap();
        assert!(result.content.contains("Test Decision Body"), "Should include body content");
    }

    #[test]
    fn kb_query_max_results_limits_output() {
        let dir = tempdir().unwrap();
        let ctx = kb_test_context(dir.path());

        for i in 1..=10 {
            let id = format!("ENTRY-{:03}", i);
            store_test_entry(&ctx, &format!("decisions/{}.md", &id), &id, "decision", &format!("Entry {}", i), &["test"]);
        }

        let args = make_args(serde_json::json!({
            "max_results": 3
        }));

        let result = kb_query_handler(&args, &ctx).unwrap();
        assert!(result.content.contains("找到 3 个匹配条目"), "Should limit to 3 results");
    }

    #[test]
    fn kb_query_excludes_archived_by_default() {
        let dir = tempdir().unwrap();
        let ctx = kb_test_context(dir.path());

        // 正常条目
        store_test_entry(&ctx, "decisions/ADR-200.md", "ADR-200", "decision", "Active Decision", &["test"]);

        // 归档条目：content 中带 archived: true frontmatter
        let archived_content = "---\nid: ADR-201\ntype: decision\ntitle: Archived Decision\ntags: [test]\nstatus: completed\narchived: true\n---\n# Archived\n\nBody.";
        let args_store = make_args(serde_json::json!({
            "path": "decisions/ADR-201.md",
            "content": archived_content,
            "update_index": true
        }));
        kb_store_handler(&args_store, &ctx).unwrap();

        // 默认查询：应只返回活跃条目
        let args = make_args(serde_json::json!({
            "query": "Decision",
            "max_results": 10
        }));
        let result = kb_query_handler(&args, &ctx).unwrap();
        assert!(result.content.contains("Active Decision"), "Should find active entry");
        assert!(
            !result.content.contains("Archived Decision"),
            "Archived entry should be excluded by default"
        );

        // include_archived=true：应包含归档条目
        let args2 = make_args(serde_json::json!({
            "query": "Decision",
            "max_results": 10,
            "include_archived": true
        }));
        let result2 = kb_query_handler(&args2, &ctx).unwrap();
        assert!(
            result2.content.contains("Archived Decision"),
            "Archived entry should be included when include_archived=true"
        );
    }

    // 辅助函数：创建测试条目
    fn store_test_entry(ctx: &ToolContext, path: &str, id: &str, entry_type: &str, title: &str, tags: &[&str]) {
        let tags_str = tags.iter().map(|t| format!("\"{}\"", t)).collect::<Vec<_>>().join(", ");
        let content = format!(
            "---\nid: {}\ntype: {}\ntitle: {}\ntags: [{}]\nstatus: accepted\n---\n# {}\n\n{} Body.",
            id, entry_type, title, tags_str, title, title
        );
        let args = make_args(serde_json::json!({
            "path": path,
            "content": content,
            "update_index": true
        }));
        kb_store_handler(&args, ctx).unwrap();
    }
}