pub mod compressor;
pub mod context;
pub mod display;
pub mod history;
pub mod token_counter;

pub use context::ContextManager;

use crate::llm::{LlmClient, LlmResponse, ToolCall};
use crate::persist::SessionStore;
use crate::skills::Skill;
use crate::tools::{ToolRegistry, ToolResult};
use crate::utils::message_output::MessageOutput;
use crate::utils::error::AppError;
use tracing::debug;

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
    llm: LlmClient,
    max_iterations: usize,
    skills: Vec<Skill>,
    /// 可选的会话持久化存储。存在时，所有对话事件和工具调用都会被记录。
    session_store: Option<SessionStore>,
}

impl Agent {
    pub fn new(
        context: ContextManager,
        tools: ToolRegistry,
        llm: LlmClient,
        config: AgentConfig,
        skills: Vec<Skill>,
        session_store: Option<SessionStore>,
    ) -> Self {
        Self {
            context,
            tools,
            llm,
            max_iterations: config.max_iterations,
            skills,
            session_store,
        }
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
        let tool_schemas = self.tools.get_tool_schemas();

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

                let results = self.process_tool_calls(&tool_calls, output)?;

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
                return Err(AppError::Llm(format!("LLM error: {}", err)));
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
    // 内部方法
    // -----------------------------------------------------------------------

    fn process_tool_calls(&mut self, tool_calls: &[ToolCall], output: &mut dyn MessageOutput) -> Result<Vec<ToolResult>, AppError> {
        let mut results = Vec::new();

        for tool_call in tool_calls {
            output.info(&format!("执行工具: {} (id: {})", tool_call.function.name, tool_call.id));
            debug!(tool = %tool_call.function.name, args = %tool_call.function.arguments, "Tool arguments");

            // 根据工具的 skip_security 标记决定走 execute_approved 还是 execute。
            // finish/restart 等元工具在 ToolDefinition 中标了 skip_security: true。
            let result = match self.tools.execute_with_policy(
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
