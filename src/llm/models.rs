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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_config_resolve_env_vars() {
        std::env::set_var("TEST_API_KEY", "sk-test123");
        let mut config = ProviderConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            api_url: "http://localhost:8080".to_string(),
            api_key: Some("${TEST_API_KEY}".to_string()),
            model: "gpt-4o".to_string(),
            temperature: Some(0.0),
            max_tokens: Some(100),
        };
        config.resolve_env_vars();
        assert_eq!(config.api_key, Some("sk-test123".to_string()));
        std::env::remove_var("TEST_API_KEY");
    }

    #[test]
    fn test_provider_config_resolve_unknown_env_var() {
        // 未设置的环境变量应保持原样
        let mut config = ProviderConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            api_url: "http://${UNDEFINED_VAR}:8080".to_string(),
            api_key: Some("real-key".to_string()),
            model: "gpt-4o".to_string(),
            temperature: Some(0.0),
            max_tokens: Some(100),
        };
        config.resolve_env_vars();
        assert_eq!(config.api_url, "http://${UNDEFINED_VAR}:8080");
        assert_eq!(config.api_key, Some("real-key".to_string()));
    }

    #[test]
    fn test_llm_message_serde() {
        let msg = LlmMessage {
            role: "user".to_string(),
            content: Some("hello".to_string()),
            tool_calls: None,
            tool_call_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"user\""));
        assert!(json.contains("\"hello\""));

        let deserialized: LlmMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.role, "user");
        assert_eq!(deserialized.content, Some("hello".to_string()));
    }

    #[test]
    fn test_llm_message_with_tool_calls() {
        let msg = LlmMessage {
            role: "assistant".to_string(),
            content: Some("calling tool".to_string()),
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                function: ToolCallFunction {
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "test.txt"}),
                },
            }]),
            tool_call_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"tool_calls\""));
        assert!(json.contains("\"read_file\""));

        let deserialized: LlmMessage = serde_json::from_str(&json).unwrap();
        assert!(deserialized.tool_calls.is_some());
        let calls = deserialized.tool_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "read_file");
    }

    #[test]
    fn test_tool_schema_serde() {
        let schema = ToolSchema {
            tool_type: "function".to_string(),
            function: ToolFunctionSchema {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"}
                    }
                }),
            },
        };
        let json = serde_json::to_string(&schema).unwrap();
        assert!(json.contains("\"read_file\""));

        let deserialized: ToolSchema = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.function.name, "read_file");
    }

    #[test]
    fn test_llm_request_serde() {
        let request = LlmRequest {
            model: "gpt-4o".to_string(),
            messages: vec![LlmMessage {
                role: "user".to_string(),
                content: Some("hello".to_string()),
                tool_calls: None,
                tool_call_id: None,
            }],
            tools: Some(vec![]),
            temperature: 0.5,
            max_tokens: 100,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"gpt-4o\""));
        assert!(json.contains("\"temperature\":0.5"));

        let deserialized: LlmRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.model, "gpt-4o");
        assert_eq!(deserialized.messages.len(), 1);
    }

    #[test]
    fn test_llm_response_variants() {
        let text = LlmResponse::Text("hello".to_string());
        match text {
            LlmResponse::Text(content) => assert_eq!(content, "hello"),
            _ => panic!("Expected Text variant"),
        }

        let tool_calls = LlmResponse::ToolCalls(vec![
            ToolCall {
                id: "call_1".to_string(),
                function: ToolCallFunction {
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({}),
                },
            },
        ]);
        match tool_calls {
            LlmResponse::ToolCalls(calls) => assert_eq!(calls.len(), 1),
            _ => panic!("Expected ToolCalls variant"),
        }
    }

    #[test]
    fn test_models_config_deser() {
        let toml_str = r#"
[[models]]
name = "gpt-4"
provider = "openai"
api_url = "https://api.openai.com/v1"
api_key = "sk-xxx"
model = "gpt-4-turbo"
temperature = 0.1
max_tokens = 4096

[[models]]
name = "claude-3"
provider = "anthropic"
api_url = "https://api.anthropic.com/v1"
api_key = "sk-yyy"
model = "claude-3-opus"
"#;
        let config: ModelsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.models.len(), 2);
        assert_eq!(config.models[0].name, "gpt-4");
        assert_eq!(config.models[1].name, "claude-3");
        // max_tokens 只对第一个模型设置了
        assert_eq!(config.models[0].max_tokens, Some(4096));
        assert_eq!(config.models[1].max_tokens, None);
    }
}
