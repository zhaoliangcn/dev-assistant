pub mod context;

pub use context::ContextManager;

use crate::llm::{LlmClient, LlmResponse, ToolCall};
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

pub struct Agent<'a> {
    pub context: ContextManager,
    tools: ToolRegistry<'a>,
    llm: LlmClient,
    max_iterations: usize,
    skills: Vec<Skill>,
}

impl<'a> Agent<'a> {
    pub fn new(
        context: ContextManager,
        tools: ToolRegistry<'a>,
        llm: LlmClient,
        config: AgentConfig,
        skills: Vec<Skill>,
    ) -> Self {
        Self {
            context,
            tools,
            llm,
            max_iterations: config.max_iterations,
            skills,
        }
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
        }

        self.context
            .add_message(crate::agent::context::Role::User, user_message, None, None);
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

                let results = self.process_tool_calls(&tool_calls, output)?;

                for (tool_call, result) in tool_calls.iter().zip(results.iter()) {
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
                self.context.compress().await?;

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

            // For finish, execute directly without security check
            if tool_call.function.name == "finish" {
                let result = match self.tools.execute_approved(
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
                output.success(&format!("工具 {} 执行成功", tool_call.function.name));
                results.push(result);
                continue;
            }

            let result = match self.tools.execute(
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

            // Handle High/Medium security evaluations that require user approval
            if let Some(ref eval) = result.security_evaluation {
                if matches!(
                    eval.danger_level,
                    crate::security::DangerLevel::High | crate::security::DangerLevel::Medium
                ) && self.tools.security.requires_approval(&eval.danger_level)
                {
                    output.warning(&format!(
                        "{} 需要审批 (级别: {}): {}",
                        tool_call.function.name,
                        eval.danger_level.as_str(),
                        eval.reason
                    ));
                    results.push(ToolResult {
                        success: false,
                        content: format!(
                            "[security] ⚠️  {} wants to {} (level: {}). \
                             Tell the user: type 'approve' to allow, or 'cancel' to skip.",
                            tool_call.function.name,
                            eval.reason,
                            eval.danger_level.as_str()
                        ),
                        security_evaluation: Some(eval.clone()),
                        restart_requested: false,
                    });
                    continue;
                }
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
