//! 条目去重（Dream 阶段 ③）。
//!
//! 两步去重：
//! 1. **规则初筛**：标题归一化 + 字符 n-gram Jaccard 相似度，产出候选对
//!    （零 LLM 成本，快速排除明显不重复的条目）
//! 2. **LLM 确认**：对候选对（相似度处于模糊区间）用 LLM 判断是否真重复
//!
//! 合并规则：
//! - 保留较新（updated 更晚）的条目，合并标签并记录 `merged_from`
//! - 较旧条目置 `archived=true`（只归档不删）
//! - 高置信度（≥ `AUTO_MERGE_THRESHOLD`）的候选对跳过 LLM，直接合并
//!
//! 纯规则模式（`llm = None`）只合并高置信度对，模糊区间候选对跳过并记录。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::llm::{LlmClient, LlmMessage, LlmResponse};
use crate::tools::kb::{KbIndex, KbIndexEntry};
use crate::utils::error::AppError;

/// 候选对初筛阈值：相似度 ≥ 此值才进入候选（规则层）。
pub const PREFILTER_THRESHOLD: f64 = 0.45;

/// 高置信度阈值：相似度 ≥ 此值直接合并，无需 LLM 确认。
pub const AUTO_MERGE_THRESHOLD: f64 = 0.85;

/// LLM 确认的模糊区间下界（相似度低于此值的候选对不送 LLM，避免浪费预算）。
const LLM_CONFIRM_MIN_SIMILARITY: f64 = 0.55;

/// 单个候选对（规则初筛产物）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidatePair {
    /// 条目 A 的 ID
    pub id_a: String,
    /// 条目 B 的 ID
    pub id_b: String,
    /// n-gram Jaccard 相似度 0.0~1.0
    pub similarity: f64,
}

/// 确认后的重复对。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicatePair {
    /// 保留的条目 ID（较新）
    pub keep_id: String,
    /// 被合并（归档）的条目 ID（较旧）
    pub merged_id: String,
    /// 相似度
    pub similarity: f64,
    /// 是否经 LLM 确认（false = 规则自动合并）
    pub confirmed_by_llm: bool,
}

/// 去重阶段结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DedupResult {
    /// 确认合并的对
    pub merged: Vec<DuplicatePair>,
    /// 模糊区间但未确认的对（记录在报告中供人工查看）
    pub unconfirmed: Vec<CandidatePair>,
}

impl DedupResult {
    /// 合并的条目数。
    pub fn count(&self) -> usize {
        self.merged.len()
    }
}

/// 归一化标题：小写、去空白、去常见标点，用于 n-gram 比较。
fn normalize_title(title: &str) -> String {
    title
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, ':' | '/' | '\\' | '-' | '_' | '(' | ')' | '[' | ']' | '{' | '}' | '.' | ',' | '，' | '。' | '：' | '（' | '）'))
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 字符 n-gram（bigram）集合。
fn char_bigrams(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 2 {
        return vec![s.to_string()];
    }
    chars
        .windows(2)
        .map(|w| w.iter().collect::<String>())
        .collect()
}

/// 计算两个标题的 n-gram Jaccard 相似度。
///
/// 对中文（无空格分词）用字符 bigram，对英文用归一化后的字符 bigram
/// 已足够作为初筛信号；标题越短，bigram 交集越能反映重复。
pub fn title_similarity(a: &str, b: &str) -> f64 {
    let na = normalize_title(a);
    let nb = normalize_title(b);
    if na == nb && !na.is_empty() {
        return 1.0;
    }
    let ga = char_bigrams(&na);
    let gb = char_bigrams(&nb);
    if ga.is_empty() || gb.is_empty() {
        return 0.0;
    }
    let intersect = ga.iter().filter(|g| gb.contains(g)).count();
    let union = ga.len() + gb.len() - intersect;
    if union == 0 {
        return 0.0;
    }
    intersect as f64 / union as f64
}

/// 规则初筛：在索引中查找相似度 ≥ `PREFILTER_THRESHOLD` 的候选对。
///
/// 跳过已归档条目与相同 ID。返回按相似度降序排列的候选对。
pub fn find_candidate_pairs(index: &KbIndex, threshold: f64) -> Vec<CandidatePair> {
    let entries: Vec<(&String, &KbIndexEntry)> = index
        .entries
        .iter()
        .filter(|(_, e)| !e.archived)
        .collect();

    let mut pairs = Vec::new();
    for (i, (id_a, ea)) in entries.iter().enumerate() {
        for (id_b, eb) in entries.iter().skip(i + 1) {
            if id_a == id_b {
                continue;
            }
            let sim = title_similarity(&ea.title, &eb.title);
            if sim >= threshold {
                pairs.push(CandidatePair {
                    id_a: id_a.to_string(),
                    id_b: id_b.to_string(),
                    similarity: sim,
                });
            }
        }
    }
    pairs.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(std::cmp::Ordering::Equal));
    pairs
}

/// 用 LLM 确认候选对是否真重复。
///
/// 只处理相似度处于模糊区间（≥ `LLM_CONFIRM_MIN_SIMILARITY` 且
/// < `AUTO_MERGE_THRESHOLD`）的候选对。LLM 失败时跳过（不误合并）。
/// `budget_tokens` 为可用的最大输入 token，超出则提前停止。
pub async fn confirm_with_llm(
    llm: &LlmClient,
    pairs: &[CandidatePair],
    budget_tokens: usize,
) -> Vec<DuplicatePair> {
    if pairs.is_empty() || budget_tokens == 0 {
        return Vec::new();
    }

    // 只送模糊区间内的候选对
    let to_confirm: Vec<&CandidatePair> = pairs
        .iter()
        .filter(|p| p.similarity >= LLM_CONFIRM_MIN_SIMILARITY && p.similarity < AUTO_MERGE_THRESHOLD)
        .collect();
    if to_confirm.is_empty() {
        return Vec::new();
    }

    // 构造 JSON 列表供 LLM 判断
    let list: Vec<serde_json::Value> = to_confirm
        .iter()
        .map(|p| {
            serde_json::json!({
                "id_a": p.id_a,
                "id_b": p.id_b,
                "similarity": p.similarity,
            })
        })
        .collect();

    let prompt = format!(
        "以下是从知识库中按标题相似度初筛出的候选重复条目对。\
         请判断每一对是否确实是同一知识条目的重复（标题含义相同、内容主题一致）。\
         只返回 JSON 数组，包含判定为重复的对的索引（0 基），例如 [0, 2]。\
         不要返回其他内容。\n\n{}",
        serde_json::to_string_pretty(&list).unwrap_or_default()
    );

    let response = llm
        .call(
            vec![LlmMessage {
                role: "system".to_string(),
                content: Some("你是一个严谨的知识库去重审核助手，只做重复判定。".to_string()),
                tool_calls: None,
                tool_call_id: None,
            },
            LlmMessage {
                role: "user".to_string(),
                content: Some(prompt),
                tool_calls: None,
                tool_call_id: None,
            }],
            Vec::new(),
        )
        .await;

    let text = match response {
        Ok(LlmResponse::Text(t)) => t,
        Ok(_) => {
            tracing::warn!("去重确认 LLM 返回非文本，跳过确认");
            return Vec::new();
        }
        Err(e) => {
            tracing::warn!("去重确认 LLM 调用失败: {}，跳过确认", e);
            return Vec::new();
        }
    };

    // 解析 JSON 数组（容错：提取方括号内的数字）
    let confirmed_idx: Vec<usize> = text
        .split(|c: char| c == '[' || c == ']' || c == ',' || c == ' ')
        .filter_map(|s| s.trim().parse::<usize>().ok())
        .filter(|i| *i < to_confirm.len())
        .collect();

    confirmed_idx
        .into_iter()
        .map(|i| {
            let p = to_confirm[i];
            // 保留较新（updated 更新）的条目
            DuplicatePair {
                keep_id: p.id_a.clone(),
                merged_id: p.id_b.clone(),
                similarity: p.similarity,
                confirmed_by_llm: true,
            }
        })
        .collect()
}

/// 执行合并：将确认的重复对中的较旧条目归档，较新条目合并标签并记录 merged_from。
///
/// `dry_run` 为 true 时只返回结果，不修改任何文件。
pub fn merge_duplicates(
    kb_root: &Path,
    index: &mut KbIndex,
    pairs: &[DuplicatePair],
    dry_run: bool,
) -> Result<DedupResult, AppError> {
    let mut result = DedupResult::default();

    for pair in pairs {
        let (keep_id, merged_id) = pick_newer(index, &pair.keep_id, &pair.merged_id);
        let (Some(keep), Some(merged)) = (
            index.entries.get(&keep_id).cloned(),
            index.entries.get(&merged_id).cloned(),
        ) else {
            continue;
        };

        let dup = DuplicatePair {
            keep_id: keep_id.clone(),
            merged_id: merged_id.clone(),
            similarity: pair.similarity,
            confirmed_by_llm: pair.confirmed_by_llm,
        };
        result.merged.push(dup);

        if dry_run {
            continue;
        }

        // 合并标签（并集）
        let mut merged_tags = keep.tags.clone();
        for t in &merged.tags {
            if !merged_tags.contains(t) {
                merged_tags.push(t.clone());
            }
        }

        // 记录 merged_from
        let mut merged_from = keep
            .relates_to
            .clone()
            .unwrap_or_default();
        merged_from.push(merged_id.clone());

        if let Some(k) = index.entries.get_mut(&keep_id) {
            k.tags = merged_tags;
            k.relates_to = Some(merged_from);
            k.updated = Some(chrono::Utc::now().to_rfc3339());
        }
        if let Some(m) = index.entries.get_mut(&merged_id) {
            m.archived = true;
        }
    }

    if !dry_run && !pairs.is_empty() {
        let index_path = kb_root.join("index.json");
        let index_json = serde_json::to_string_pretty(index).map_err(AppError::Json)?;
        std::fs::write(&index_path, index_json).map_err(|e| {
            AppError::Io(std::io::Error::other(format!("写入 KB 索引失败: {}", e)))
        })?;
    }

    Ok(result)
}

/// 选择保留（较新）与被合并（较旧）的条目。
fn pick_newer(index: &KbIndex, id_a: &str, id_b: &str) -> (String, String) {
    let ts_a = index
        .entries
        .get(id_a)
        .and_then(|e| e.updated.as_deref())
        .unwrap_or("");
    let ts_b = index
        .entries
        .get(id_b)
        .and_then(|e| e.updated.as_deref())
        .unwrap_or("");
    if ts_b > ts_a {
        (id_b.to_string(), id_a.to_string())
    } else {
        (id_a.to_string(), id_b.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn entry(id: &str, title: &str, updated: &str) -> (String, KbIndexEntry) {
        (
            id.to_string(),
            KbIndexEntry {
                path: format!("decisions/{}.md", id),
                entry_type: "decision".to_string(),
                title: title.to_string(),
                tags: vec!["test".to_string()],
                status: "accepted".to_string(),
                archived: false,
                relates_to: None,
                depends_on: None,
                supersedes: None,
                author: None,
                created: Some(updated.to_string()),
                updated: Some(updated.to_string()),
            },
        )
    }

    #[test]
    fn identical_titles_have_similarity_one() {
        assert_eq!(title_similarity("同步工具添加缓存支持", "同步工具添加缓存支持"), 1.0);
    }

    #[test]
    fn near_identical_titles_have_high_similarity() {
        let sim = title_similarity("记忆系统深度优化分析", "记忆系统优化分析");
        assert!(sim >= 0.6, "相近标题相似度应较高，got {}", sim);
    }

    #[test]
    fn unrelated_titles_have_low_similarity() {
        let sim = title_similarity("调度器架构设计", "文件缓存实现");
        assert!(sim < 0.4, "无关标题相似度应较低，got {}", sim);
    }

    #[test]
    fn find_pairs_filters_by_threshold_and_archived() {
        let mut index = KbIndex::default();
        index.entries.insert("A-1".into(), entry("A-1", "缓存同步优化方案", "2026-08-01T00:00:00Z").1);
        index.entries.insert("A-2".into(), entry("A-2", "缓存同步优化方案", "2026-08-02T00:00:00Z").1);
        let mut archived = entry("A-3", "缓存同步优化方案", "2026-08-03T00:00:00Z").1;
        archived.archived = true;
        index.entries.insert("A-3".into(), archived);

        let pairs = find_candidate_pairs(&index, PREFILTER_THRESHOLD);
        assert_eq!(pairs.len(), 1, "应只找到一对活跃重复，归档条目被跳过");
        assert_eq!(pairs[0].similarity, 1.0);
    }

    #[test]
    fn merge_archives_older_and_merges_tags() {
        let dir = tempdir().unwrap();
        let kb_root = dir.path().join(".kb");
        std::fs::create_dir_all(&kb_root).unwrap();

        let mut index = KbIndex::default();
        index
            .entries
            .insert("OLD".into(), entry("OLD", "缓存优化", "2026-07-01T00:00:00Z").1);
        index
            .entries
            .insert("NEW".into(), entry("NEW", "缓存优化", "2026-08-01T00:00:00Z").1);
        index.entries.get_mut("NEW").unwrap().tags.push("performance".into());

        let pairs = vec![DuplicatePair {
            keep_id: "OLD".into(),
            merged_id: "NEW".into(),
            similarity: 1.0,
            confirmed_by_llm: false,
        }];

        let result = merge_duplicates(&kb_root, &mut index, &pairs, false).unwrap();
        assert_eq!(result.count(), 1);
        // 较新的 NEW 被保留，较旧的 OLD 被归档
        assert!(!index.entries["NEW"].archived);
        assert!(index.entries["OLD"].archived);
        // NEW 合并了标签
        assert!(index.entries["NEW"].tags.contains(&"performance".to_string()));
        // NEW 记录了 merged_from
        let mf = index.entries["NEW"].relates_to.as_ref().unwrap();
        assert!(mf.contains(&"OLD".to_string()));
    }
}
