use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Debug, Clone)]
pub struct LlmConfig {
    #[allow(dead_code)]
    pub provider: String,
    pub api_url: String,
    pub api_key: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: usize,
}
