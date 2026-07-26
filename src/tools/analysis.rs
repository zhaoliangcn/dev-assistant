use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info};

use crate::tools::{common, ToolArgs, ToolContext, ToolDefinition, ToolResult};
use crate::utils::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AnalysisType {
    Structure,
    Logic,
    Security,
    Performance,
    Design,
}

impl AnalysisType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "structure" => Some(Self::Structure),
            "logic" => Some(Self::Logic),
            "security" => Some(Self::Security),
            "performance" => Some(Self::Performance),
            "design" => Some(Self::Design),
            _ => None,
        }
    }

    pub fn to_str(&self) -> &str {
        match self {
            Self::Structure => "structure",
            Self::Logic => "logic",
            Self::Security => "security",
            Self::Performance => "performance",
            Self::Design => "design",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

impl Severity {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "critical" => Some(Self::Critical),
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            _ => None,
        }
    }

    pub fn to_str(&self) -> &str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Issue {
    pub severity: Severity,
    pub description: String,
    pub location: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnalysisRecord {
    pub file_path: String,
    pub analysis_type: AnalysisType,
    pub summary: String,
    pub details: Value,
    pub issues: Vec<Issue>,
    pub timestamp: SystemTime,
    pub batch_number: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)] // reserved for future analysis summary aggregation
pub struct AnalysisSummary {
    pub analysis_records: Vec<AnalysisRecord>,
    pub task_summary: TaskSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)] // reserved for future analysis summary aggregation
pub struct TaskSummary {
    pub total_files: usize,
    pub analyzed_files: usize,
    pub issues_found: HashMap<String, usize>,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // reserved for future file batch processing
pub struct FileBatch {
    pub files: Vec<String>,
    pub batch_number: usize,
    pub total_batches: usize,
    pub directory: String,
    pub estimated_tokens: usize,
}

pub fn create_file_batches(
    files: Vec<String>,
    batch_size: usize,
    max_tokens_per_batch: usize,
) -> Vec<FileBatch> {
    let mut batches = Vec::new();
    let mut current_batch = Vec::new();
    let mut current_tokens = 0;
    let mut batch_number = 1;
    let total_files = files.len();
    let total_batches = total_files.div_ceil(batch_size);

    for file in files {
        let file_tokens = estimate_file_tokens(&file);
        if current_batch.len() >= batch_size || (current_tokens + file_tokens > max_tokens_per_batch && !current_batch.is_empty()) {
            batches.push(FileBatch {
                files: std::mem::take(&mut current_batch),
                batch_number,
                total_batches,
                directory: extract_directory(&file),
                estimated_tokens: current_tokens,
            });
            batch_number += 1;
            current_tokens = 0;
        }
        current_batch.push(file);
        current_tokens += file_tokens;
    }

    if !current_batch.is_empty() {
        batches.push(FileBatch {
            files: current_batch,
            batch_number,
            total_batches,
            directory: "".to_string(),
            estimated_tokens: current_tokens,
        });
    }

    batches
}

fn estimate_file_tokens(file_path: &str) -> usize {
    Path::new(file_path)
        .metadata()
        .map(|m| (m.len() / 4) as usize)
        .unwrap_or(1024)
}

fn extract_directory(file_path: &str) -> String {
    Path::new(file_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or(".".to_string())
}

fn get_analysis_dir(working_dir: &Path) -> PathBuf {
    working_dir.join(".kb").join("analysis")
}

fn save_analysis_record(working_dir: &Path, record: &AnalysisRecord) -> Result<(), AppError> {
    let analysis_dir = get_analysis_dir(working_dir);
    fs::create_dir_all(&analysis_dir).map_err(AppError::Io)?;

    let file_name = format!(
        "{}-{}-{}.json",
        record.batch_number,
        record.analysis_type.to_str(),
        sanitize_filename(&record.file_path)
    );
    let file_path = analysis_dir.join(file_name);

    let content = serde_json::to_string_pretty(record).map_err(AppError::Json)?;
    fs::write(&file_path, content).map_err(AppError::Io)?;

    debug!(path = %file_path.display(), "Saved analysis record");
    Ok(())
}

fn load_all_analysis_records(working_dir: &Path) -> Result<Vec<AnalysisRecord>, AppError> {
    let analysis_dir = get_analysis_dir(working_dir);
    if !analysis_dir.exists() {
        return Ok(Vec::new());
    }

    let mut records = Vec::new();
    for entry in fs::read_dir(&analysis_dir).map_err(AppError::Io)? {
        let entry = entry.map_err(AppError::Io)?;
        let path = entry.path();
        if path.is_file() && path.extension().map(|e| e == "json").unwrap_or(false) {
            let content = fs::read_to_string(&path).map_err(AppError::Io)?;
            let record: AnalysisRecord = serde_json::from_str(&content).map_err(AppError::Json)?;
            records.push(record);
        }
    }

    records.sort_by_key(|r| r.batch_number);
    Ok(records)
}

fn sanitize_filename(name: &str) -> String {
    name.replace(|c: char| !c.is_alphanumeric() && c != '.' && c != '_', "_")
}

pub fn record_analysis_tool() -> ToolDefinition {
    ToolDefinition {
        name: "record_analysis".to_string(),
        description: "Record code analysis results to persistent storage for later aggregation and reporting.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path of the file being analyzed"
                },
                "analysis_type": {
                    "type": "string",
                    "enum": ["structure", "logic", "security", "performance", "design"],
                    "description": "Type of analysis"
                },
                "summary": {
                    "type": "string",
                    "description": "Analysis summary"
                },
                "details": {
                    "type": "object",
                    "description": "Detailed analysis results (JSON)"
                },
                "issues": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "severity": { "type": "string", "enum": ["critical", "high", "medium", "low"] },
                            "description": { "type": "string" },
                            "location": { "type": "string" }
                        }
                    },
                    "description": "List of issues found"
                },
                "batch_number": {
                    "type": "integer",
                    "description": "Batch number this analysis belongs to",
                    "default": 1
                }
            },
            "required": ["file_path", "analysis_type", "summary"]
        }),
        skip_security: true,
        handler: Box::new(record_analysis_handler),
    }
}

fn record_analysis_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    let file_path = args.arguments["file_path"]
        .as_str()
        .ok_or_else(|| AppError::Llm("file_path is required".to_string()))?;

    let analysis_type_str = args.arguments["analysis_type"]
        .as_str()
        .ok_or_else(|| AppError::Llm("analysis_type is required".to_string()))?;
    let analysis_type = AnalysisType::from_str(analysis_type_str)
        .ok_or_else(|| AppError::Llm(format!("Invalid analysis_type: {}", analysis_type_str)))?;

    let summary = args.arguments["summary"]
        .as_str()
        .ok_or_else(|| AppError::Llm("summary is required".to_string()))?;

    let details = args.arguments["details"].clone();

    let issues: Vec<Issue> = args.arguments["issues"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|obj| {
                    let severity_str = obj["severity"].as_str()?;
                    let severity = Severity::from_str(severity_str)?;
                    let description = obj["description"].as_str()?.to_string();
                    let location = obj["location"].as_str()?.to_string();
                    let suggestion = obj["suggestion"].as_str().map(|s| s.to_string());
                    Some(Issue { severity, description, location, suggestion })
                })
                .collect()
        })
        .unwrap_or_default();

    let batch_number = args.arguments["batch_number"]
        .as_u64()
        .map(|n| n as usize)
        .unwrap_or(1);

    let record = AnalysisRecord {
        file_path: file_path.to_string(),
        analysis_type,
        summary: summary.to_string(),
        details,
        issues,
        timestamp: SystemTime::now(),
        batch_number,
    };

    save_analysis_record(&context.working_dir, &record)?;

    Ok(ToolResult {
        success: true,
        security_evaluation: None,
        restart_requested: false,
                error_category: None,
        content: format!("[record_analysis] 分析记录已保存: {}", file_path),
    })
}

pub fn get_analysis_summary_tool() -> ToolDefinition {
    ToolDefinition {
        name: "get_analysis_summary".to_string(),
        description: "Get summary information about the current codebase analysis, including analyzed files count and issue statistics.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "group_by": {
                    "type": "string",
                    "enum": ["file", "type", "severity", "directory"],
                    "description": "Dimension to group by",
                    "default": "type"
                }
            }
        }),
        skip_security: true,
        handler: Box::new(get_analysis_summary_handler),
    }
}

fn get_analysis_summary_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    let group_by = args.arguments["group_by"].as_str().unwrap_or("type");
    let records = load_all_analysis_records(&context.working_dir)?;

    if records.is_empty() {
        return Ok(ToolResult {
            success: true,
            security_evaluation: None,
            restart_requested: false,
                error_category: None,
            content: "[get_analysis_summary] 暂无分析记录".to_string(),
        });
    }

    let summary = match group_by {
        "file" => group_by_file(&records),
        "type" => group_by_type(&records),
        "severity" => group_by_severity(&records),
        "directory" => group_by_directory(&records),
        _ => group_by_type(&records),
    };

    Ok(ToolResult {
        success: true,
        security_evaluation: None,
        restart_requested: false,
                error_category: None,
        content: format!("[get_analysis_summary]\n{}", summary),
    })
}

fn group_by_file(records: &[AnalysisRecord]) -> String {
    let mut result = String::new();
    for record in records {
        result.push_str(&format!(
            "- {} ({}): {} issues\n",
            record.file_path,
            record.analysis_type.to_str(),
            record.issues.len()
        ));
    }
    result
}

fn group_by_type(records: &[AnalysisRecord]) -> String {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut issue_counts: HashMap<String, usize> = HashMap::new();
    for record in records {
        let key = record.analysis_type.to_str();
        *counts.entry(key.to_string()).or_insert(0) += 1;
        *issue_counts.entry(key.to_string()).or_insert(0) += record.issues.len();
    }

    let mut result = String::new();
    for (ty, count) in counts {
        let issues = issue_counts.get(&ty).unwrap_or(&0);
        result.push_str(&format!("- {}: {} files analyzed, {} issues\n", ty, count, issues));
    }
    result
}

fn group_by_severity(records: &[AnalysisRecord]) -> String {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for record in records {
        for issue in &record.issues {
            *counts.entry(issue.severity.to_str().to_string()).or_insert(0) += 1;
        }
    }

    let mut result = String::new();
    for severity in ["critical", "high", "medium", "low"] {
        let count = counts.get(severity).unwrap_or(&0);
        result.push_str(&format!("- {}: {} issues\n", severity, count));
    }
    result
}

fn group_by_directory(records: &[AnalysisRecord]) -> String {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for record in records {
        let dir = extract_directory(&record.file_path);
        *counts.entry(dir).or_insert(0) += 1;
    }

    let mut result = String::new();
    for (dir, count) in counts {
        result.push_str(&format!("- {}: {} files analyzed\n", dir, count));
    }
    result
}

pub fn analyze_codebase_tool() -> ToolDefinition {
    ToolDefinition {
        name: "analyze_codebase".to_string(),
        description: "Start automated analysis of a large codebase. This task will traverse the codebase, analyze files in batches, and generate a comprehensive analysis report.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "include_patterns": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "File patterns to include (glob), e.g., ['**/*.rs', 'tests/**/*.rs']",
                    "default": ["**/*.rs"]
                },
                "exclude_patterns": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "File patterns to exclude, e.g., ['target/**', '.git/**']",
                    "default": ["target/**", ".git/**", "node_modules/**"]
                },
                "batch_size": {
                    "type": "integer",
                    "description": "Number of files per batch",
                    "default": 5
                },
                "analysis_depth": {
                    "type": "string",
                    "enum": ["quick", "standard", "deep"],
                    "description": "Analysis depth: quick=structure only, standard=structure+logic, deep=complete analysis",
                    "default": "standard"
                }
            }
        }),
        skip_security: true,
        handler: Box::new(analyze_codebase_handler),
    }
}

fn analyze_codebase_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    let include_patterns: Vec<String> = args.arguments["include_patterns"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_else(|| vec!["**/*.rs".to_string()]);

    let exclude_patterns: Vec<String> = args.arguments["exclude_patterns"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_else(|| vec!["target/**".to_string(), ".git/**".to_string(), "node_modules/**".to_string()]);

    let batch_size = common::get_lenient_usize(&args.arguments["batch_size"], "batch_size", 5)
        .map_err(AppError::Llm)?;

    let analysis_depth = args.arguments["analysis_depth"].as_str().unwrap_or("standard");

    info!(
        "Starting codebase analysis: {} patterns, {} exclusions, batch_size={}, depth={}",
        include_patterns.len(),
        exclude_patterns.len(),
        batch_size,
        analysis_depth
    );

    let files = find_files(&context.working_dir, &include_patterns, &exclude_patterns)?;

    if files.is_empty() {
        return Ok(ToolResult {
            success: false,
            security_evaluation: None,
            restart_requested: false,
                error_category: None,
            content: "[analyze_codebase] ❌ 未找到匹配的文件".to_string(),
        });
    }

    let batches = create_file_batches(files.clone(), batch_size, 8000);

    let mut result = format!(
        "[analyze_codebase] 发现 {} 个文件，分为 {} 批\n\n",
        files.len(),
        batches.len()
    );

    for (i, batch) in batches.iter().enumerate().take(5) {
        result.push_str(&format!(
            "批 {}: {} 个文件 (目录: {})\n",
            i + 1,
            batch.files.len(),
            batch.directory
        ));
    }

    if batches.len() > 5 {
        result.push_str(&format!("... 还有 {} 批\n", batches.len() - 5));
    }

    result.push_str(&format!(
        "\n请使用 batch_read_files 读取第一批文件，然后使用 record_analysis 记录分析结果。\n\
        分析深度: {}\n\
        当所有文件分析完成后，请使用 finish_analysis 生成综合报告。",
        analysis_depth
    ));

    Ok(ToolResult {
        success: true,
        security_evaluation: None,
        restart_requested: false,
                error_category: None,
        content: result,
    })
}

fn find_files(working_dir: &Path, include_patterns: &[String], exclude_patterns: &[String]) -> Result<Vec<String>, AppError> {
    use globset::{Glob, GlobSetBuilder};

    let mut include_builder = GlobSetBuilder::new();
    for pattern in include_patterns {
        include_builder.add(Glob::new(pattern).map_err(|e| AppError::Llm(format!("Invalid include pattern: {}", e)))?);
    }
    let include_set = include_builder.build().map_err(|e| AppError::Llm(format!("Failed to build include pattern set: {}", e)))?;

    let mut exclude_builder = GlobSetBuilder::new();
    for pattern in exclude_patterns {
        exclude_builder.add(Glob::new(pattern).map_err(|e| AppError::Llm(format!("Invalid exclude pattern: {}", e)))?);
    }
    let exclude_set = exclude_builder.build().map_err(|e| AppError::Llm(format!("Failed to build exclude pattern set: {}", e)))?;

    let mut files: Vec<String> = Vec::new();
    for entry in walkdir::WalkDir::new(working_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            if let Some(name) = entry_path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }
            continue;
        }

        if !include_set.is_match(entry_path) {
            continue;
        }

        if exclude_set.is_match(entry_path) {
            continue;
        }

        if let Ok(relative) = entry.path().strip_prefix(working_dir) {
            files.push(relative.to_string_lossy().to_string());
        }
    }

    files.sort();
    Ok(files)
}

pub fn finish_analysis_tool() -> ToolDefinition {
    ToolDefinition {
        name: "finish_analysis".to_string(),
        description: "Complete the codebase analysis task, aggregate all analysis results, and generate a comprehensive report.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "report_type": {
                    "type": "string",
                    "enum": ["summary", "detailed", "security", "performance", "architecture"],
                    "description": "Type of report to generate",
                    "default": "detailed"
                },
                "output_file": {
                    "type": "string",
                    "description": "Optional output file path for the report"
                }
            }
        }),
        skip_security: true,
        handler: Box::new(finish_analysis_handler),
    }
}

fn finish_analysis_handler(args: &ToolArgs, context: &ToolContext) -> Result<ToolResult, AppError> {
    let report_type = args.arguments["report_type"].as_str().unwrap_or("detailed");
    let output_file = args.arguments["output_file"].as_str();

    let records = load_all_analysis_records(&context.working_dir)?;

    if records.is_empty() {
        return Ok(ToolResult {
            success: false,
            security_evaluation: None,
            restart_requested: false,
                error_category: None,
            content: "[finish_analysis] ❌ 暂无分析记录，无法生成报告".to_string(),
        });
    }

    let report = generate_report(&records, report_type);

    if let Some(output_path) = output_file {
        let full_path = context.working_dir.join(output_path);
        fs::write(&full_path, &report).map_err(AppError::Io)?;
        info!(path = %full_path.display(), "Analysis report saved");
    }

    Ok(ToolResult {
        success: true,
        security_evaluation: None,
        restart_requested: false,
                error_category: None,
        content: format!("[finish_analysis]\n\n{}", report),
    })
}

fn generate_report(records: &[AnalysisRecord], report_type: &str) -> String {
    let mut report = String::new();

    let total_files = records.len();
    let mut total_issues = 0;
    let mut issue_counts: HashMap<String, usize> = HashMap::new();

    for record in records {
        total_issues += record.issues.len();
        for issue in &record.issues {
            *issue_counts.entry(issue.severity.to_str().to_string()).or_insert(0) += 1;
        }
    }

    report.push_str(&format!(
        "# 代码库分析报告\n\n\
        **报告类型**: {}\n\
        **分析文件数**: {}\n\
        **发现问题数**: {}\n\n",
        report_type, total_files, total_issues
    ));

    report.push_str("## 问题统计\n\n");
    for severity in ["critical", "high", "medium", "low"] {
        let count = issue_counts.get(severity).unwrap_or(&0);
        report.push_str(&format!("- {}: {} 个\n", severity, count));
    }

    report.push_str("\n## 分析摘要\n\n");
    for record in records {
        report.push_str(&format!(
            "### {}\n\n\
            **分析类型**: {}\n\
            **摘要**: {}\n",
            record.file_path,
            record.analysis_type.to_str(),
            record.summary
        ));

        if !record.issues.is_empty() {
            report.push_str("\n**问题列表**:\n");
            for issue in &record.issues {
                report.push_str(&format!(
                    "- [{}] {} (位置: {})\n",
                    issue.severity.to_str(),
                    issue.description,
                    issue.location
                ));
            }
        }

        report.push_str("\n---\n\n");
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn analysis_type_from_str() {
        assert_eq!(AnalysisType::from_str("structure"), Some(AnalysisType::Structure));
        assert_eq!(AnalysisType::from_str("LOGIC"), Some(AnalysisType::Logic));
        assert_eq!(AnalysisType::from_str("unknown"), None);
    }

    #[test]
    fn severity_from_str() {
        assert_eq!(Severity::from_str("critical"), Some(Severity::Critical));
        assert_eq!(Severity::from_str("HIGH"), Some(Severity::High));
        assert_eq!(Severity::from_str("unknown"), None);
    }

    #[test]
    fn create_file_batches_basic() {
        let files = vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()];
        let batches = create_file_batches(files, 2, 8000);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].files.len(), 2);
        assert_eq!(batches[1].files.len(), 1);
    }

    #[test]
    fn analyze_codebase_handler_finds_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _ = std::fs::write(temp_dir.path().join("test.rs"), "fn main() {}");

        let args = ToolArgs {
            arguments: serde_json::json!({
                "include_patterns": ["**/*.rs"],
                "exclude_patterns": []
            })
        };
        let context = ToolContext {
            working_dir: temp_dir.path().to_path_buf(),
            resources: None,
        };

        let result = analyze_codebase_handler(&args, &context).unwrap();
        assert!(result.success);
        assert!(result.content.contains("1 个文件"));
    }
}