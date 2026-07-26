pub mod compressor;
pub mod context;
pub mod display;
pub mod history;
pub mod identity;
pub mod token_counter;

pub use context::ContextManager;
pub use identity::AgentIdentity;

use std::sync::Arc;

use crate::llm::{LlmClient, LlmResponse, ToolCall};
use crate::persist::SessionStore;
use crate::skills::Skill;
use crate::tools::{async_tool::AsyncToolRegistry, ToolRegistry, ToolResult};
use crate::utils::message_output::MessageOutput;
use crate::utils::error::AppError;
use tracing::debug;

/// 最大子代理深度。超过此深度时，返回 `SubagentDepthLimit` 错误。
const MAX_SUBAGENT_DEPTH: usize = 3;

// ---------------------------------------------------------------------------
// Agent 结果
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub struct AgentConfig {
    pub max_iterations: usize,
}

#[allow(dead_code)]
pub struct AgentResult {
    pub success: bool,
    pub message: String,
    pub restart_requested: bool,
}

/// Agent 单步执行的结果。主循环根据此类型决定是继续下一轮还是结束。
pub enum AgentStep {
    /// 继续下一轮迭代
    Continue,
    /// Agent 已完成（成功或失败）
    Done(AgentResult),
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

pub struct Agent {
    context: ContextManager,
    tools: ToolRegistry,
    async_tools: Option<AsyncToolRegistry>,
    llm: Arc<LlmClient>,
    max_iterations: usize,
    depth: usize,
    skills: Vec<Skill>,
    /// 可选的会话持久化存储。存在时，所有对话事件和工具调用都会被记录。
    session_store: Option<SessionStore>,
}

impl Agent {
    pub fn new(
        context: ContextManager,
        tools: ToolRegistry,
        async_tools: Option<AsyncToolRegistry>,
        llm: Arc<LlmClient>,
        config: AgentConfig,
        skills: Vec<Skill>,
        session_store: Option<SessionStore>,
    ) -> Self {
        Self {
            context,
            tools,
            async_tools,
            llm,
            max_iterations: config.max_iterations,
            depth: 0,
            skills,
            session_store,
        }
    }

    /// 获取所有工具的 schemas（同步工具 + 异步工具）
    pub fn get_all_tool_schemas(&self) -> Vec<crate::llm::ToolSchema> {
        let mut schemas = self.tools.get_tool_schemas();
        if let Some(ref async_tools) = self.async_tools {
            schemas.extend(async_tools.get_tool_schemas());
        }
        schemas
    }

    // ----- UI 展示缓冲区委托 -----

    pub fn add_display_message(&mut self, level: crate::utils::message_level::MessageLevel, msg: &str) {
        self.context.add_display_message(level, msg);
    }

    pub fn get_display_messages(&self) -> Vec<(String, String)> {
        self.context.get_display_messages()
    }

    /// UI 渲染所需的瞬时展示消息列表（不参与 LLM 上下文）。
    pub fn display_messages(&self) -> &[(String, String)] {
        &self.context.display.messages
    }

    /// 清空所有展示消息，保留 `history_start`。
    ///
    /// 当前 REPL 流程使用 [`reset_display_for_new_turn`] / [`clear_display_to`]，
    /// 此方法保留为未来外部独立清空展示缓冲区的扩展点。
    #[allow(dead_code)]
    pub fn clear_display_messages(&mut self) {
        self.context.display.clear_messages();
    }

    /// 重置展示缓冲区到新的 turn 起点：清空消息并把 `history_start` 推到当前 history 末尾。
    pub fn reset_display_for_new_turn(&mut self) {
        self.context.display.messages.clear();
        self.context.display.history_start = self.context.history.len();
    }

    /// 清空展示消息并把 `history_start` 推到指定位置（`/clear` 命令使用）。
    pub fn clear_display_to(&mut self, history_start: usize) {
        self.context.display.clear_messages();
        self.context.display.history_start = history_start;
    }

    // ----- 对话历史委托 -----

    pub fn history_len(&self) -> usize {
        self.context.history.len()
    }

    pub fn add_message(
        &mut self,
        role: crate::agent::context::Role,
        content: String,
        tool_calls: Option<Vec<crate::llm::ToolCall>>,
        tool_call_id: Option<String>,
    ) {
        self.context.add_message(role, content, tool_calls, tool_call_id);
    }

    // ----- 状态持久化委托 -----

    pub fn save_state(&self, path: &std::path::Path) -> Result<(), AppError> {
        self.context.save_state(path)
    }

    /// 向持久化存储记录一条助手消息（用于 REPL 中追加最终结果）。
    pub fn record_assistant_message_to_store(&mut self, content: &str) {
        if let Some(ref mut store) = self.session_store {
            store.record_assistant_message(content);
        }
    }

    // ----- 活跃模型管理 -----

    /// 设置当前活跃模型名称（用于切换模型后持久化）。
    pub fn set_active_model(&mut self, name: String) {
        self.context.active_model = Some(name);
    }

    /// 获取当前活跃模型名称（用于切换模型后持久化）。
    pub fn active_model_name(&self) -> Option<&str> {
        self.context.active_model.as_deref()
    }

    /// 开始一轮新的对话，处理用户消息并匹配技能。
    pub fn start_turn(&mut self, user_message: String, output: &mut dyn MessageOutput) {
        // 技能激活：检查用户消息是否匹配某个技能
        let matched_skill = self.match_skill(&user_message);
        if let Some(skill) = matched_skill {
            output.info(&format!("激活技能: {}", skill.meta.name));
            let skill_name = skill.meta.name.clone();
            let skill_desc = skill.meta.description.clone();
            let skill_body = skill.body.clone();
            self.context.add_display_message(
                crate::utils::message_level::MessageLevel::Info,
                &format!("[技能] 已激活: {} — {}", skill_name, skill_desc),
            );
            let skill_instructions = format!(
                "【技能激活: {}】\n{}\n\n请严格按照上述技能流程执行任务。",
                skill_name,
                skill_body
            );
            self.context.add_message(
                crate::agent::context::Role::System,
                skill_instructions,
                None,
                None,
            );
            // 持久化：记录技能激活消息
            if let Some(ref mut store) = self.session_store {
                store.record_system_message(&format!("技能激活: {} — {}", skill_name, skill_desc));
            }
        }

        self.context
            .add_message(crate::agent::context::Role::User, user_message.clone(), None, None);

        // 持久化：记录用户消息
        if let Some(ref mut store) = self.session_store {
            store.record_user_message(&user_message);
        }
    }

    /// 执行一轮 Agent 迭代（一次 LLM 调用 + 响应处理）。
    /// 返回 `AgentStep::Continue` 表示需要继续下一轮，`AgentStep::Done` 表示已完成。
    pub async fn step(&mut self, output: &mut dyn MessageOutput) -> Result<AgentStep, AppError> {
        output.info(&format!("↻ 第 {} 轮：向 LLM 发送请求...", self.context.consecutive_no_tool_rounds + 1));

        let messages = self.context.build_messages();
        let tool_schemas = self.get_all_tool_schemas();

        let response = self.llm.call(messages, tool_schemas).await?;

        match response {
            LlmResponse::Text(content) => {
                debug!(content = %content, "LLM responded directly");
                self.context.add_message(
                    crate::agent::context::Role::Assistant,
                    content.clone(),
                    None,
                    None,
                );
                self.context.increment_no_tool_rounds();

                // 持久化：记录助手文本回复
                if let Some(ref mut store) = self.session_store {
                    store.record_assistant_message(&content);
                }

                // Only return early if LLM has given a substantive response
                if self.context.get_consecutive_no_tool_rounds() >= 2 {
                    return Ok(AgentStep::Done(AgentResult {
                        success: true,
                        message: content,
                        restart_requested: false,
                    }));
                }
                Ok(AgentStep::Continue)
            }
            LlmResponse::ToolCalls(tool_calls) => {
                output.info(&format!("LLM 请求调用 {} 个工具", tool_calls.len()));
                self.context.reset_no_tool_rounds();

                // 持久化：记录工具调用请求
                for tc in &tool_calls {
                    if let Some(ref mut store) = self.session_store {
                        store.record_tool_call(
                            &tc.id,
                            &tc.function.name,
                            tc.function.arguments.clone(),
                        );
                    }
                }

                let results = self.process_tool_calls(&tool_calls, output).await?;

                for (tool_call, result) in tool_calls.iter().zip(results.iter()) {
                    // 持久化：记录工具执行结果
                    if let Some(ref mut store) = self.session_store {
                        store.record_tool_result(
                            &tool_call.id,
                            &tool_call.function.name,
                            result.success,
                            &result.content,
                        );
                    }

                    if tool_call.function.name == "finish" {
                        return Ok(AgentStep::Done(AgentResult {
                            success: true,
                            message: result.content.clone(),
                            restart_requested: result.restart_requested,
                        }));
                    }

                    if result.restart_requested {
                        return Ok(AgentStep::Done(AgentResult {
                            success: true,
                            message: result.content.clone(),
                            restart_requested: true,
                        }));
                    }

                    self.context.add_tool_result(tool_call, &result.content);
                }

                // 压缩上下文，防止 token 无限制增长
                let compression_info = self.context.compress()?;

                // 持久化：记录压缩事件
                if compression_info.did_compress {
                    if let Some(ref mut store) = self.session_store {
                        store.record_compression(
                            compression_info.original_messages,
                            compression_info.after_messages,
                            compression_info.kept_rounds,
                            compression_info.original_tokens,
                            compression_info.after_tokens,
                        );
                    }
                    output.info(&format!(
                        "上下文压缩: {} → {} 条消息 (保留 {} 轮, {} → {} tokens)",
                        compression_info.original_messages,
                        compression_info.after_messages,
                        compression_info.kept_rounds,
                        compression_info.original_tokens,
                        compression_info.after_tokens,
                    ));
                }

                Ok(AgentStep::Continue)
            }
            LlmResponse::Error(err) => {
                output.error(&format!("LLM 错误: {}", err));
                Err(AppError::Llm(format!("LLM error: {}", err)))
            }
        }
    }

    /// 完整运行 Agent 直到完成（非交互模式使用）。
    pub async fn run(&mut self, user_message: String, output: &mut dyn MessageOutput) -> Result<AgentResult, AppError> {
        self.start_turn(user_message, output);

        for _ in 0..self.max_iterations {
            match self.step(output).await? {
                AgentStep::Done(result) => return Ok(result),
                AgentStep::Continue => continue,
            }
        }

        output.warning("达到最大迭代次数，任务可能未完成");
        Ok(AgentResult {
            success: false,
            message: "Maximum iterations reached. Task may not be complete.".to_string(),
            restart_requested: false,
        })
    }

    // -----------------------------------------------------------------------
    // 子代理创建
    // -----------------------------------------------------------------------

    /// 创建一个子 Agent。
    ///
    /// 子 Agent 拥有：
    /// - 独立的 `ContextManager`（新鲜的对话上下文，只包含 system prompt 和任务描述）
    /// - 受限的 `ToolRegistry`（只有文件工具和 finish，没有 spawn_subagent 和 restart）
    /// - 共享的 `Arc<LlmClient>`（与父 Agent 使用同一个 LLM 客户端）
    /// - `depth = parent_depth + 1`
    ///
    /// 如果深度超过 `MAX_SUBAGENT_DEPTH`，返回 `SubagentDepthLimit` 错误。
    pub fn new_subagent(
        llm: Arc<LlmClient>,
        tools: ToolRegistry,
        depth: usize,
        task: &str,
        context: &str,
        max_iterations: usize,
        max_tokens: usize,
        agent_type: Option<AgentIdentity>,
    ) -> Result<Self, AppError> {
        if depth > MAX_SUBAGENT_DEPTH {
            return Err(AppError::SubagentDepthLimit(MAX_SUBAGENT_DEPTH));
        }

        let identity = agent_type.unwrap_or(AgentIdentity::General);
        let system_prompt = identity.system_prompt();

        let context_manager = ContextManager::new(system_prompt, max_tokens);

        let mut ctx = context_manager;
        let task_description = if context.is_empty() {
            format!("任务目标：{}", task)
        } else {
            format!("任务目标：{}\n\n上下文信息：\n{}", task, context)
        };
        ctx.add_message(
            crate::agent::context::Role::User,
            task_description,
            None,
            None,
        );

        Ok(Self {
            context: ctx,
            tools,
            async_tools: None,
            llm,
            max_iterations,
            depth,
            skills: Vec::new(),
            session_store: None,
        })
    }

    /// 获取当前 Agent 的深度
    #[allow(dead_code)]
    pub fn depth(&self) -> usize {
        self.depth
    }

    // -----------------------------------------------------------------------
    // 内部方法
    // -----------------------------------------------------------------------

    async fn process_tool_calls(&mut self, tool_calls: &[ToolCall], output: &mut dyn MessageOutput) -> Result<Vec<ToolResult>, AppError> {
        let mut results = Vec::new();

        for tool_call in tool_calls {
            output.info(&format!("执行工具: {} (id: {})", tool_call.function.name, tool_call.id));
            debug!(tool = %tool_call.function.name, args = %tool_call.function.arguments, "Tool arguments");

            // ── 拦截 spawn_subagent 工具调用 ──
            if tool_call.function.name == "spawn_subagent" {
                let result = self.handle_spawn_subagent(tool_call, output).await?;
                results.push(result);
                continue;
            }

            // 首先检查异步工具注册表
            let result = if let Some(ref async_tools) = self.async_tools {
                match async_tools.execute_with_policy(
                    &tool_call.function.name,
                    tool_call.function.arguments.clone(),
                ).await {
                    Ok(r) => r,
                    Err(AppError::ToolNotFound(_)) => {
                        // 异步工具不存在，回退到同步工具
                        match self.tools.execute_with_policy(
                            &tool_call.function.name,
                            tool_call.function.arguments.clone(),
                        ) {
                            Ok(r) => r,
                            Err(e) => ToolResult {
                                success: false,
                                content: format!("[error] Tool '{}' execution failed: {}", tool_call.function.name, e),
                                security_evaluation: None,
                                restart_requested: false,
                            },
                        }
                    }
                    Err(e) => ToolResult {
                        success: false,
                        content: format!("[error] Tool '{}' execution failed: {}", tool_call.function.name, e),
                        security_evaluation: None,
                        restart_requested: false,
                    },
                }
            } else {
                // 没有异步工具注册表，使用同步工具
                match self.tools.execute_with_policy(
                    &tool_call.function.name,
                    tool_call.function.arguments.clone(),
                ) {
                    Ok(r) => r,
                    Err(e) => ToolResult {
                        success: false,
                        content: format!("[error] Tool '{}' execution failed: {}", tool_call.function.name, e),
                        security_evaluation: None,
                        restart_requested: false,
                    },
                }
            };

            // Security 评估由 ToolRegistry::execute 处理：Critical/High/Medium
            // 时返回带 security_evaluation 的失败结果，Agent 仅负责日志和透传。
            if let Some(ref eval) = result.security_evaluation {
                output.warning(&format!(
                    "{} 安全评估 ({}): {}",
                    tool_call.function.name,
                    eval.danger_level.as_str(),
                    eval.reason
                ));
            }

            if result.success {
                output.success(&format!("工具 {} 执行成功", tool_call.function.name));
            } else {
                output.error(&format!("工具 {} 执行失败", tool_call.function.name));
            }
            results.push(result);
        }

        Ok(results)
    }

    /// 处理 `spawn_subagent` 工具调用：创建子 Agent、执行任务、返回结果。
    async fn handle_spawn_subagent(&mut self, tool_call: &ToolCall, output: &mut dyn MessageOutput) -> Result<ToolResult, AppError> {
        let task = tool_call.function.arguments["task"]
            .as_str()
            .unwrap_or("");

        let context = tool_call.function.arguments["context"]
            .as_str()
            .unwrap_or("");

        // 解析子代理配置参数（带默认值）
        let sub_max_iterations = tool_call.function.arguments["max_iterations"]
            .as_u64()
            .map(|n| n as usize)
            .unwrap_or(15);
        let sub_max_tokens = tool_call.function.arguments["max_tokens"]
            .as_u64()
            .map(|n| n as usize)
            .unwrap_or(8192);
        let agent_type = tool_call.function.arguments["agent_type"]
            .as_str()
            .and_then(AgentIdentity::from_str);

        // 检查深度限制
        let child_depth = self.depth + 1;
        if child_depth > MAX_SUBAGENT_DEPTH {
            output.warning(&format!(
                "子代理深度限制 (最大: {}), 当前深度: {}",
                MAX_SUBAGENT_DEPTH, self.depth
            ));
            return Ok(ToolResult {
                success: false,
                security_evaluation: None,
                restart_requested: false,
                content: format!(
                    "[spawn_subagent] ❌ 子代理深度超过限制 (最大深度: {}, 当前深度: {})。\
                     请自行完成此任务，不要继续分解子任务。",
                    MAX_SUBAGENT_DEPTH, self.depth
                ),
            });
        }

        let agent_type_str = agent_type.as_ref().map(|a| a.to_str()).unwrap_or("general");
        output.info(&format!(
            "创建子代理 (深度 {}, 类型: {})，任务: {}",
            child_depth,
            agent_type_str,
            if task.len() > 80 { format!("{}...", &task[..80]) } else { task.to_string() }
        ));

        let subagent_tools = if let Some(ref identity) = agent_type {
            self.tools.new_subagent_registry_with_identity(identity)
        } else {
            self.tools.new_subagent_registry()
        };

        let mut subagent = match Agent::new_subagent(
            self.llm.clone(),
            subagent_tools,
            child_depth,
            task,
            context,
            sub_max_iterations,
            sub_max_tokens,
            agent_type,
        ) {
            Ok(agent) => agent,
            Err(e) => {
                output.error(&format!("创建子代理失败: {}", e));
                return Ok(ToolResult {
                    success: false,
                    security_evaluation: None,
                    restart_requested: false,
                    content: format!("[spawn_subagent] ❌ 创建子代理失败: {}", e),
                });
            }
        };

        let mut sub_output = crate::ui::UIMessageOutput::new(false);
        let result = Box::pin(subagent.run(task.to_string(), &mut sub_output)).await;

        match result {
            Ok(agent_result) => {
                if agent_result.success {
                    output.success("子代理任务完成");
                    Ok(ToolResult {
                        success: true,
                        security_evaluation: None,
                        restart_requested: false,
                        content: format!(
                            "[spawn_subagent] ✅ 子代理任务完成\n深度: {}\n结果: {}",
                            child_depth, agent_result.message
                        ),
                    })
                } else {
                    output.error(&format!("子代理任务失败: {}", agent_result.message));
                    Ok(ToolResult {
                        success: false,
                        security_evaluation: None,
                        restart_requested: false,
                        content: format!(
                            "[spawn_subagent] ❌ 子代理任务失败\n深度: {}\n错误: {}",
                            child_depth, agent_result.message
                        ),
                    })
                }
            }
            Err(e) => {
                output.error(&format!("子代理执行出错: {}", e));
                Ok(ToolResult {
                    success: false,
                    security_evaluation: None,
                    restart_requested: false,
                    content: format!("[spawn_subagent] ❌ 子代理执行出错: {}", e),
                })
            }
        }
    }

    /// 匹配用户消息与已注册技能。优先匹配 `when_to_use` 触发条件，
    /// 其次按名称关键词匹配。
    fn match_skill(&self, message: &str) -> Option<&Skill> {
        let msg_lower = message.to_lowercase();

        for skill in &self.skills {
            if let Some(ref when) = skill.meta.when_to_use {
                let keywords: Vec<String> = when
                    .split(|c: char| c.is_ascii_punctuation() || c == '，' || c == '、')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect();
                if keywords.iter().any(|kw| msg_lower.contains(kw.as_str())) {
                    return Some(skill);
                }
            }

            let name_parts: Vec<&str> = skill
                .meta
                .name
                .split(|c: char| c == '-' || c == '_' || c.is_whitespace())
                .collect();
            if name_parts.iter().any(|part| msg_lower.contains(part)) {
                return Some(skill);
            }
        }

        None
    }

    /// 切换到指定名称的模型
    pub fn switch_model(&mut self, name: &str) -> Result<(), AppError> {
        self.llm.switch_model(name)
    }

    /// 列出所有可用模型
    pub fn list_models(&self) -> Vec<&str> {
        self.llm.list_models()
    }

    /// 当前活跃模型名称
    pub fn active_model(&self) -> &str {
        self.llm.active_model()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmClient;
    use crate::security::SecurityPolicy;
    use crate::tools::ToolRegistry;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// 创建一个测试用的 LlmClient（仅用于构造 Agent，不会被实际调用）
    fn test_llm_client() -> Arc<LlmClient> {
        let config = crate::llm::ProviderConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            api_url: "http://localhost:9999/v1".to_string(),
            api_key: Some("test-key".to_string()),
            model: "test-model".to_string(),
            temperature: Some(0.0),
            max_tokens: Some(100),
        };
        Arc::new(LlmClient::from_configs(vec![config]).unwrap())
    }

    /// 创建一个测试用的 ToolRegistry
    fn test_tool_registry() -> ToolRegistry {
        let policy = Arc::new(SecurityPolicy::new(PathBuf::new().as_path(), true));
        ToolRegistry::new(PathBuf::new(), policy)
    }

    #[test]
    fn new_subagent_depth_limit_exceeded() {
        let llm = test_llm_client();
        let tools = test_tool_registry();

        // 深度超过 MAX_SUBAGENT_DEPTH 时应返回错误
        let result = Agent::new_subagent(
            llm,
            tools,
            MAX_SUBAGENT_DEPTH + 1, // 超出深度
            "test task",
            "",
            10,
            4096,
            None,
        );

        assert!(result.is_err());
        match result {
            Err(AppError::SubagentDepthLimit(max)) => assert_eq!(max, MAX_SUBAGENT_DEPTH),
            _ => panic!("Expected SubagentDepthLimit error"),
        }
    }

    #[test]
    fn new_subagent_creates_at_depth_limit() {
        let llm = test_llm_client();
        let tools = test_tool_registry();

        // 深度等于 MAX_SUBAGENT_DEPTH 时应成功
        let result = Agent::new_subagent(
            llm,
            tools,
            MAX_SUBAGENT_DEPTH, // 等于最大深度
            "test task",
            "",
            10,
            4096,
            None,
        );

        assert!(result.is_ok());
        let agent = result.unwrap();
        assert_eq!(agent.depth(), MAX_SUBAGENT_DEPTH);
    }

    #[test]
    fn new_subagent_has_no_skills() {
        let llm = test_llm_client();
        let tools = test_tool_registry();

        let agent = Agent::new_subagent(llm, tools, 1, "test task", "", 10, 4096, None).unwrap();

        // 子代理不应有技能
        assert!(agent.skills.is_empty());
    }

    #[test]
    fn new_subagent_has_no_session_store() {
        let llm = test_llm_client();
        let tools = test_tool_registry();

        let agent = Agent::new_subagent(llm, tools, 1, "test task", "", 10, 4096, None).unwrap();

        // 子代理不应有 session_store
        assert!(agent.session_store.is_none());
    }

    #[test]
    fn new_subagent_context_contains_task() {
        let llm = test_llm_client();
        let tools = test_tool_registry();

        let agent = Agent::new_subagent(llm, tools, 1, "特定任务描述", "", 10, 4096, None).unwrap();

        // 验证任务描述出现在上下文中
        let messages = agent.context.build_messages();
        let user_msg = messages.iter().find(|m| m.role == "user");
        assert!(user_msg.is_some(), "sub-agent should have a user message with task");
        let content = user_msg.unwrap().content.as_ref().unwrap();
        assert!(content.contains("特定任务描述"), "task description should be in user message");
    }

    #[test]
    fn new_subagent_context_includes_context_param() {
        let llm = test_llm_client();
        let tools = test_tool_registry();

        let agent = Agent::new_subagent(
            llm, tools, 1, "test task", "额外上下文信息", 10, 4096, None,
        ).unwrap();

        let messages = agent.context.build_messages();
        let user_msg = messages.iter().find(|m| m.role == "user").unwrap();
        let content = user_msg.content.as_ref().unwrap();
        assert!(content.contains("额外上下文信息"), "context param should be in user message");
    }

    #[test]
    fn new_subagent_registry_excludes_spawn_subagent() {
        let policy = Arc::new(SecurityPolicy::new(PathBuf::new().as_path(), true));
        let parent_registry = ToolRegistry::new(PathBuf::new(), policy);

        let sub_registry = parent_registry.new_subagent_registry();

        // 子代理注册表不应包含 spawn_subagent
        assert!(sub_registry.get_tool("spawn_subagent").is_none(),
            "sub-agent registry should not contain spawn_subagent");

        // 子代理注册表不应包含 restart
        assert!(sub_registry.get_tool("restart").is_none(),
            "sub-agent registry should not contain restart");

        // 子代理注册表应包含基本工具
        assert!(sub_registry.get_tool("read_file").is_some());
        assert!(sub_registry.get_tool("write_file").is_some());
        assert!(sub_registry.get_tool("finish").is_some());
        assert!(sub_registry.get_tool("exec_command").is_some());
    }
}
