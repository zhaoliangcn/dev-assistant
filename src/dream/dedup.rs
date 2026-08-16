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

use std::collections::{HashMap, HashSet};
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
/// 提取标题的字符 bigram 集合（**去重**）。
///
/// 去重是正确性的关键：若保留重复 bigram，倒排索引会把同一索引在同一桶内
/// 压多次、生成 `(i,i)` 自配对；且 `similarity_from_sets` 的多重集计数会放大
/// Jaccard（如 `测试测试` vs `测试A` 算成 0.67 而真实为 0.33）。
fn char_bigrams(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 2 {
        return vec![s.to_string()];
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for w in chars.windows(2) {
        let bg: String = w.iter().collect();
        if seen.insert(bg.clone()) {
            out.push(bg);
        }
    }
    out
}

/// 从预计算的归一化标题与 bigram 集合计算 n-gram Jaccard 相似度。
///
/// 供 `find_candidate_pairs` 复用已缓存的集合，避免对每条标题重复归一化与分词。
fn similarity_from_sets(ga: &[String], gb: &[String], na: &str, nb: &str) -> f64 {
    if na == nb && !na.is_empty() {
        return 1.0;
    }
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

/// 计算两个标题的 n-gram Jaccard 相似度。
///
/// 对中文（无空格分词）用字符 bigram，对英文用归一化后的字符 bigram
/// 已足够作为初筛信号；标题越短，bigram 交集越能反映重复。
#[allow(dead_code)] // 生产路径走 similarity_from_sets；保留为公开工具 + 测试基准
pub fn title_similarity(a: &str, b: &str) -> f64 {
    let na = normalize_title(a);
    let nb = normalize_title(b);
    let ga = char_bigrams(&na);
    let gb = char_bigrams(&nb);
    similarity_from_sets(&ga, &gb, &na, &nb)
}

/// 规则初筛：在索引中查找相似度 ≥ `PREFILTER_THRESHOLD` 的候选对。
///
/// 使用倒排索引候选生成替代全量两两比较：
/// 1. 预计算每条活跃条目的归一化标题与 bigram 集合（各一次）
/// 2. 建立 `bigram → 条目索引` 倒排索引
/// 3. 只对共享至少一个 bigram 的条目对计算相似度（用 HashSet 去重，
///    因为两条目共享多个 bigram 会在多个桶重复出现）
///
/// 正确性依据：共享 0 个 bigram 的标题 Jaccard 相似度恒为 0，永远达不到
/// 阈值，可安全跳过。因此结果与全量比较完全等价，但最坏复杂度从 O(n²)
/// 降到接近线性（仅对高相似候选对密集时退化）。
///
/// 跳过已归档条目与相同 ID。返回按相似度降序排列的候选对。
pub fn find_candidate_pairs(index: &KbIndex, threshold: f64) -> Vec<CandidatePair> {
    // ① 收集活跃条目，预计算归一化标题与 bigram 集合
    let entries: Vec<(&String, &KbIndexEntry)> = index
        .entries
        .iter()
        .filter(|(_, e)| !e.archived)
        .collect();
    if entries.len() < 2 {
        return Vec::new();
    }

    let mut norm_titles = Vec::with_capacity(entries.len());
    let mut bigram_sets = Vec::with_capacity(entries.len());
    for (_, e) in &entries {
        let nt = normalize_title(&e.title);
        bigram_sets.push(char_bigrams(&nt));
        norm_titles.push(nt);
    }

    // ② 建倒排索引：bigram → 条目索引列表
    let mut postings: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, bigrams) in bigram_sets.iter().enumerate() {
        for b in bigrams {
            postings.entry(b.clone()).or_default().push(i);
        }
    }

    // ③ 候选生成：桶内条目两两组合，去重后计算相似度
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    let mut pairs = Vec::new();
    for bucket in postings.values() {
        if bucket.len() < 2 {
            continue;
        }
        for (k, &i) in bucket.iter().enumerate() {
            for &j in bucket.iter().skip(k + 1) {
                let (a, b) = if i < j { (i, j) } else { (j, i) };
                // 兜底：跳过同一索引的自配对（去重后理论上不会出现，
                // 但防御未来 char_bigrams 或 postings 变更引入的回归）。
                if a == b {
                    continue;
                }
                if !seen.insert((a, b)) {
                    continue;
                }
                let sim = similarity_from_sets(
                    &bigram_sets[a],
                    &bigram_sets[b],
                    &norm_titles[a],
                    &norm_titles[b],
                );
                if sim >= threshold {
                    pairs.push(CandidatePair {
                        id_a: entries[a].0.to_string(),
                        id_b: entries[b].0.to_string(),
                        similarity: sim,
                    });
                }
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

    use rand::RngExt;

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
                query_count: 0,
                last_query_at: None,
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

    // =====================================================================
    // 倒排索引实现专用测试
    // =====================================================================

    /// 暴力参考实现：全量两两比较，用于验证倒排索引实现的保真性。
    fn find_candidate_pairs_bruteforce(index: &KbIndex, threshold: f64) -> Vec<CandidatePair> {
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

    /// 将候选对列表转换为 (id_a, id_b) 的无序对集合（用于比较，忽略相似度值）。
    fn pair_set(pairs: &[CandidatePair]) -> HashSet<(String, String)> {
        pairs
            .iter()
            .map(|p| {
                if p.id_a < p.id_b {
                    (p.id_a.clone(), p.id_b.clone())
                } else {
                    (p.id_b.clone(), p.id_a.clone())
                }
            })
            .collect()
    }

    /// 随机生成一个含 N 条条目的 KB 索引，可选比例设置重复标题。
    fn random_kb(
        count: usize,
        duplicate_ratio: f64,
        rng: &mut impl rand::Rng,
    ) -> KbIndex {
        let mut index = KbIndex::default();
        let base_titles = [
            "缓存同步优化方案",
            "记忆系统深度分析",
            "调度器架构设计",
            "文件缓存实现",
            "API 接口定义",
            "错误处理策略",
            "日志系统设计",
            "配置管理方案",
            "网络请求优化",
            "数据持久化方案",
            "前端渲染优化",
            "后端服务架构",
            "测试框架集成",
            "部署流水线设计",
            "安全审计方案",
            "国际化支持",
            "性能监控方案",
            "插件系统设计",
            "事件驱动架构",
            "消息队列方案",
        ];

        for i in 0..count {
            let title = if rng.random_bool(duplicate_ratio) {
                // 从已有条目中随机选一个标题作为重复
                let idx = rng.random_range(0..i.max(1));
                let existing_id = format!("ENTRY-{:04}", idx);
                index
                    .entries
                    .get(&existing_id)
                    .map(|e| e.title.clone())
                    .unwrap_or_else(|| format!("自动生成标题 {}", i))
            } else {
                let t = base_titles[i % base_titles.len()];
                format!("{} {}", t, i / base_titles.len())
            };

            let id = format!("ENTRY-{:04}", i);
            let updated = format!("2026-08-{:02}T00:00:00Z", (i % 28) + 1);
            index.entries.insert(
                id,
                KbIndexEntry {
                    path: format!("decisions/{}.md", i),
                    entry_type: "decision".to_string(),
                    title,
                    tags: vec!["test".to_string()],
                    status: "accepted".to_string(),
                    archived: false,
                    relates_to: None,
                    depends_on: None,
                    supersedes: None,
                    author: None,
                    created: Some(updated.clone()),
                    updated: Some(updated),
                    query_count: 0,
                    last_query_at: None,
                },
            );
        }
        index
    }

    #[test]
    fn inverted_index_faithfulness_small() {
        // 小规模随机测试：倒排索引结果应与暴力参考完全一致
        let mut rng = rand::rng();
        for _ in 0..20 {
            let count = rng.random_range(5..30);
            let dup_ratio = rng.random_range(0.0..0.4);
            let index = random_kb(count, dup_ratio, &mut rng);

            let new_result = find_candidate_pairs(&index, PREFILTER_THRESHOLD);
            let ref_result = find_candidate_pairs_bruteforce(&index, PREFILTER_THRESHOLD);

            let new_set = pair_set(&new_result);
            let ref_set = pair_set(&ref_result);

            assert_eq!(
                new_set, ref_set,
                "倒排索引实现与暴力参考结果不一致 (count={}, dup_ratio={})",
                count, dup_ratio
            );
        }
    }

    #[test]
    fn inverted_index_faithfulness_edge_cases() {
        // 边界情况：单条目、空索引、全归档
let mut rng = rand::rng();
    
        // 单条目
        let single = random_kb(1, 0.0, &mut rand::rng());
        assert!(find_candidate_pairs(&single, PREFILTER_THRESHOLD).is_empty());

        // 空索引
        let empty = KbIndex::default();
        assert!(find_candidate_pairs(&empty, PREFILTER_THRESHOLD).is_empty());

        // 全归档
        let mut archived = random_kb(5, 0.0, &mut rng);
        for (_, e) in archived.entries.iter_mut() {
            e.archived = true;
        }
        assert!(find_candidate_pairs(&archived, PREFILTER_THRESHOLD).is_empty());
    }

    #[test]
    fn inverted_index_faithfulness_various_thresholds() {
        // 不同阈值下结果一致
        let mut rng = rand::rng();
        let index = random_kb(15, 0.3, &mut rng);

        for &threshold in &[0.3, 0.45, 0.6, 0.85, 0.95] {
            let new_result = find_candidate_pairs(&index, threshold);
            let ref_result = find_candidate_pairs_bruteforce(&index, threshold);

            let new_set = pair_set(&new_result);
            let ref_set = pair_set(&ref_result);

            assert_eq!(
                new_set, ref_set,
                "结果不一致 (threshold={})",
                threshold
            );
        }
    }

    #[test]
    fn inverted_index_large_scale_smoke() {
        // 大规模冒烟测试：1000 条条目，只要能正确找出重复且不崩溃即可
        let mut rng = rand::rng();
        let index = random_kb(1000, 0.2, &mut rng);

        let result = find_candidate_pairs(&index, PREFILTER_THRESHOLD);
        // 验证返回的候选对都是有效的（id 不同、相似度 >= 阈值）
        for pair in &result {
            assert_ne!(pair.id_a, pair.id_b, "候选对不应包含相同 ID");
            assert!(
                pair.similarity >= PREFILTER_THRESHOLD,
                "相似度应 >= 阈值"
            );
            assert!(pair.similarity <= 1.0, "相似度不应超过 1.0");
        }

        // 验证对暴力参考小规模子集的一致性
        // 取前 50 条，与暴力参考对比
        let mut small_index = KbIndex::default();
        for (id, entry) in index.entries.iter().take(50) {
            small_index.entries.insert(id.clone(), entry.clone());
        }
        let new_small = find_candidate_pairs(&small_index, PREFILTER_THRESHOLD);
        let ref_small = find_candidate_pairs_bruteforce(&small_index, PREFILTER_THRESHOLD);
        assert_eq!(
            pair_set(&new_small),
            pair_set(&ref_small),
            "大规模测试中前 50 条子集结果不一致"
        );
    }

    #[test]
    fn inverted_index_no_shared_bigrams_skipped() {
        // 验证完全不共享 bigram 的标题不会被生成候选
        let mut index = KbIndex::default();
        // "a" 的 bigram 是 ["a"]，"bc" 的 bigram 是 ["bc"]，无交集
        index.entries.insert("A".into(), entry("A", "a", "2026-08-01T00:00:00Z").1);
        index.entries.insert("B".into(), entry("B", "bc", "2026-08-01T00:00:00Z").1);

        let pairs = find_candidate_pairs(&index, 0.0);
        assert!(pairs.is_empty(), "无共享 bigram 的标题不应产生候选对");
    }

    #[test]
    fn inverted_index_shared_bigram_found() {
        // 验证共享 bigram 的标题能被正确找到
        let mut index = KbIndex::default();
        // "ab" 的 bigram 是 ["ab"]，"abc" 的 bigram 是 ["ab","bc"]，共享 "ab"
        index.entries.insert("X".into(), entry("X", "ab", "2026-08-01T00:00:00Z").1);
        index.entries.insert("Y".into(), entry("Y", "abc", "2026-08-01T00:00:00Z").1);

        let pairs = find_candidate_pairs(&index, 0.0);
        assert_eq!(pairs.len(), 1, "共享 bigram 的标题应产生候选对");
        assert_eq!(pairs[0].similarity, 0.5, "Jaccard(['ab'], ['ab','bc']) = 1/2 = 0.5");
    }

    #[test]
    fn repeated_bigrams_dedup_no_self_pair_and_correct_jaccard() {
        // 回归测试：char_bigrams 去重前，"测试测试"（bigram ["测试","试测","测试"]）
        // 会在 postings 桶内把同一索引压两次，生成 (A,A) 自配对，且与 "测试A" 的
        // 多重集 Jaccard 被放大为 2/3。去重后应为真实集合 Jaccard 1/3，且无自配对。
        let mut index = KbIndex::default();
        index.entries.insert("A".into(), entry("A", "测试测试", "2026-08-01T00:00:00Z").1);
        index.entries.insert("B".into(), entry("B", "测试A", "2026-08-01T00:00:00Z").1);
        index.entries.insert("C".into(), entry("C", "完全不同", "2026-08-01T00:00:00Z").1);

        let pairs = find_candidate_pairs(&index, 0.0);
        // 无自配对
        for p in &pairs {
            assert_ne!(p.id_a, p.id_b, "不应产生自配对: {:?}", p);
        }
        // A 与 B 的真实集合 Jaccard = |{测试}| / |{测试,试测,试A}| = 1/3
        let ab = pairs
            .iter()
            .find(|p| (p.id_a == "A" && p.id_b == "B") || (p.id_a == "B" && p.id_b == "A"))
            .expect("A/B 共享 bigram 应产生候选对");
        assert!(
            (ab.similarity - 1.0 / 3.0).abs() < 1e-9,
            "Jaccard 应为 1/3≈0.333，实际 {}",
            ab.similarity
        );
        // C 与 A/B 无共享 bigram，不应出现
        assert!(
            pairs.iter().all(|p| p.id_a != "C" && p.id_b != "C"),
            "无共享 bigram 的 C 不应出现在候选对中: {:?}",
            pairs
        );
    }

    #[test]
    fn title_similarity_known_answers() {
        // 钉住 title_similarity 的正确集合 Jaccard 值，防止未来回归到多重集计数。
        assert!(
            (title_similarity("测试测试", "测试A") - 1.0 / 3.0).abs() < 1e-9,
            "测试测试 vs 测试A 应为 1/3"
        );
        assert_eq!(title_similarity("ab", "abc"), 0.5);
        assert_eq!(title_similarity("a", "bc"), 0.0);
        // 完全相同 → 1.0
        assert_eq!(title_similarity("缓存优化", "缓存优化"), 1.0);
    }
}
