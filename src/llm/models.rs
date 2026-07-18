use serde::{Deserialize, Serialize};
use serde_json::Value;
use regex;

/// 单个模型的配置，来自 TOML 文件或环境变量
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub provider: String,
    pub api_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    pub model: String,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<usize>,
}

impl ProviderConfig {
    /// 用环境变量替换 `${VAR}` 占位符
    pub fn resolve_env_vars(&mut self) {
        self.api_url = resolve_env_var(&self.api_url);
        self.api_key = self.api_key.as_ref().map(|k| resolve_env_var(k));
        self.model = resolve_env_var(&self.model);
    }
}

fn resolve_env_var(s: &str) -> String {
    let re = regex::Regex::new(r"\$\{([^}]+)\}").unwrap();
    let mut result = s.to_string();
    for cap in re.captures_iter(s) {
        let var_name = &cap[1];
        if let Ok(val) = std::env::var(var_name) {
            result = result.replace(&format!("${{{}}}", var_name), &val);
        }
    }
    result
}

/// TOML 文件根结构
#[derive(Debug, Deserialize)]
pub struct ModelsConfig {
    pub models: Vec<ProviderConfig>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LlmMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub function: ToolCallFunction,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: Value,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolSchema {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ToolFunctionSchema,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolFunctionSchema {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LlmRequest {
    pub model: String,
    pub messages: Vec<LlmMessage>,
    pub tools: Option<Vec<ToolSchema>>,
    pub temperature: f32,
    pub max_tokens: usize,
}

#[derive(Debug, Clone)]
pub enum LlmResponse {
    Text(String),
    ToolCalls(Vec<ToolCall>),
    #[allow(dead_code)]
    Error(String),
}

/// 单模型配置（旧接口，保留向后兼容）
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LlmConfig {
    #[allow(dead_code)]
    pub provider: String,
    pub api_url: String,
    pub api_key: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: usize,
}
