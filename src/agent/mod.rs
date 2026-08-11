pub mod compressor;
pub mod context;
pub mod display;
pub mod history;
pub mod identity;
pub mod pipeline_context;
pub mod pipeline_stages;
pub mod summary;
pub mod token_counter;

pub use context::ContextManager;
pub use identity::{AgentIdentity, PipelineStage};
pub use pipeline_context::{PipelineContext, PipelineContextStore, StageStatus};
pub use pipeline_stages::STAGE_TEMPLATES;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;

use crate::llm::{LlmClient, LlmStreamEvent, ToolCall};
use crate::persist::SessionStore;
use crate::skills::Skill;
use crate::tools::{async_tool::AsyncToolRegistry, ToolRegistry, ToolResult};
use crate::utils::message_output::MessageOutput;
use crate::utils::error::AppError;
use tracing::{debug, info, warn};

/// 最大子代理深度。超过此深度时，返回 `SubagentDepthLimit` 错误。
pub(crate) const MAX_SUBAGENT_DEPTH: usize = 3;

// ---------------------------------------------------------------------------
// 子代理配置
// ---------------------------------------------------------------------------

/// 子代理创建参数。
///
/// 将 `new_subagent` 的参数分组，避免参数列表过长。
pub struct SubagentConfig {
    pub llm: Arc<LlmClient>,
    pub tools: ToolRegistry,
    pub depth: usize,
    pub task: String,
    pub context: String,
    pub max_iterations: usize,
    pub max_tokens: usize,
    pub agent_type: Option<AgentIdentity>,
    /// 父代理的上下文预算信息（可选）。传递给子代理，让子代理感知父代理的
    /// 上下文压力，从而尽快完成并控制输出规模（结果需能容纳回父代理上下文）。
    pub parent_budget: Option<crate::agent::context::ContextBudget>,
    /// 崩溃恢复时重建的上下文（可选）。提供时直接使用（已包含恢复通知、
    /// 分层摘要与继续指令），跳过任务消息注入，用于检查点恢复场景。
    pub restored_context: Option<ContextManager>,
}

// ---------------------------------------------------------------------------
// Agent 结果
// ---------------------------------------------------------------------------

#[derive(Clone)]
#[allow(dead_code)] // reserved for future agent configuration API
pub struct AgentConfig {
    pub max_iterations: usize,
}

#[allow(dead_code)] // used by tests and pipeline
pub struct AgentResult {
    pub success: bool,
    pub message: String,
    pub restart_requested: bool,
    /// 是否为调用 `finish` 工具产生的结构化终止结果。
    /// 用于调用方区分"finish 交付"与普通回复，避免依赖字符串前缀判断。
    pub finished: bool,
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
    /// 工作目录，从 ToolRegistry 获取并存储，避免依赖 std::env::current_dir()
    working_dir: PathBuf,
}

impl Agent {
    pub fn new(
        mut context: ContextManager,
        tools: ToolRegistry,
        async_tools: Option<AsyncToolRegistry>,
        llm: Arc<LlmClient>,
        config: AgentConfig,
        skills: Vec<Skill>,
        session_store: Option<SessionStore>,
    ) -> Self {
        let wd = tools.working_dir().clone();

        // 跨会话记忆注入：新会话（空历史）时加载历史分层摘要。
        // 摘要以 system 消息注入上下文头部，实现重启后仍记得上次会话的关键信息。
        // 已恢复的会话（load_state）历史非空，注入方法内部会跳过。
        context.inject_historical_summaries(&wd.join(".kb"));

        Self {
            context,
            tools,
            async_tools,
            llm,
            max_iterations: config.max_iterations,
            depth: 0,
            skills,
            session_store,
            working_dir: wd,
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
    #[allow(dead_code)] // reserved for future display buffer extension
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

    /// 将持久化存储缓冲区中的事件立即刷盘。
    ///
    /// 回合结束、会话结束或 exec 重启之前调用，保证 JSONL 文件中
    /// 数据完整可见（并发读者包括 Web 会话详情与背景 ingest）。
    pub fn flush_persistence(&mut self) {
        if let Some(ref mut store) = self.session_store {
            store.flush();
        }
    }

    /// 获取会话持久化存储的文件路径（用于按需从 JSONL 生成可读日志）。
    pub fn session_store_path(&self) -> Option<&std::path::Path> {
        self.session_store.as_ref().map(|s| s.path())
    }

    // ----- 活跃模型管理 -----

    /// 获取当前活跃模型名称（用于切换模型后持久化）。
    pub fn active_model_name(&self) -> Option<&str> {
        self.context.active_model.as_deref()
    }

    /// 开始一轮新的对话，处理用户消息并匹配技能。
    pub fn start_turn(&mut self, user_message: String, output: &mut dyn MessageOutput) {
        // 技能激活：检查用户消息是否匹配某个技能
        let matched_skill = self.match_skill(&user_message);
        let final_message = if let Some(skill) = matched_skill {
            output.info(&format!("激活技能: {}", skill.meta.name));
            let skill_name = skill.meta.name.clone();
            let skill_desc = skill.meta.description.clone();
            let skill_body = skill.body.clone();
            self.context.add_display_message(
                crate::utils::message_level::MessageLevel::Info,
                &format!("[技能] 已激活: {} — {}", skill_name, skill_desc),
            );

            // 将技能指令与用户消息合并为一条 User 消息，
            // 避免以 System 角色注入与主系统提示词冲突。
            let combined = format!(
                "【技能激活: {}】\n{}\n\n---\n用户请求：\n{}",
                skill_name, skill_body, user_message
            );

            // 持久化：记录技能激活消息
            if let Some(ref mut store) = self.session_store {
                store.record_system_message(&format!("技能激活: {} — {}", skill_name, skill_desc));
            }

            combined
        } else {
            user_message.clone()
        };

        self.context
            .add_message(crate::agent::context::Role::User, final_message, None, None);

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

        // 记录工具 schema 占用的 token 数，用于预算报告准确计算
        // （system_prompt + memory + tool_schema + history 之和等于总使用量）。
        let schema_tokens = crate::agent::token_counter::TokenCounter::estimate_messages(
            &tool_schemas
                .iter()
                .map(|s| crate::llm::LlmMessage {
                    role: "system".to_string(),
                    content: serde_json::to_string(s).ok(),
                    tool_calls: None,
                    tool_call_id: None,
                })
                .collect::<Vec<_>>(),
        );
        self.context.set_tool_schema_tokens(schema_tokens);

        const MAX_EMPTY_RETRIES: u32 = 3;
        let mut empty_attempt = 0u32;

        loop {
            // 使用流式响应
            let mut stream = self.llm.call_streaming(messages.clone(), tool_schemas.clone()).await?;
            let mut assistant_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();

            // 循环读取流式事件
            while let Some(event_result) = stream.next().await {
                match event_result {
                    Ok(LlmStreamEvent::Chunk(text)) => {
                        assistant_content.push_str(&text);
                        // 实时渲染流式内容
                        output.streaming_assistant(&assistant_content, false);
                        debug!(content = %text, "Received streaming chunk");
                    }
                    Ok(LlmStreamEvent::ToolCallDelta(tc)) => {
                        // 收集工具调用
                        tool_calls.push(tc);
                    }
                    Ok(LlmStreamEvent::Usage(usage)) => {
                        output.report_token_usage(usage.prompt_tokens, usage.completion_tokens, usage.total_tokens);
                    }
                    Ok(LlmStreamEvent::Done) => {
                        // 最终渲染（移除闪烁光标）
                        output.streaming_assistant(&assistant_content, true);
                        debug!("Streaming complete");
                        break;
                    }
                    Err(e) => {
                        output.error(&format!("LLM 流式错误: {}", e));
                        return Err(e);
                    }
                }
            }

            // 根据是否包含工具调用来处理响应
            if !tool_calls.is_empty() {
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

                    // 先把工具结果写入历史，再处理 finish/restart 等终止语义，
                    // 否则这些工具的结果会丢失在上下文中。
                    self.context.add_tool_result(tool_call, &result.content);

                    if tool_call.function.name == "finish" {
                        return Ok(AgentStep::Done(AgentResult {
                            success: true,
                            message: result.content.clone(),
                            restart_requested: result.restart_requested,
                            finished: true,
                        }));
                    }

                    if result.restart_requested {
                        return Ok(AgentStep::Done(AgentResult {
                            success: true,
                            message: result.content.clone(),
                            restart_requested: true,
                            finished: false,
                        }));
                    }
                }

                // 在压缩前自动保存摘要（若上下文压力达到 Critical）
                let budget = self.context.get_budget_report();
                if matches!(
                    budget.pressure,
                    crate::agent::context::ContextPressure::Critical
                        | crate::agent::context::ContextPressure::Exhausted
                ) {
                    // 自动保存当前轮次摘要到分层摘要系统
                    let _ = self.auto_save_round_summary().await;
                }

                // 压缩上下文，防止 token 无限制增长。
                // Warning 压力用 LLM 摘要（保留语义），Critical/Exhausted 用截断
                // （关键信息已通过 auto_save_round_summary 落盘）。
                let compression_info =
                    crate::agent::compressor::ContextCompressor::compress_if_needed_async(
                        &mut self.context.history,
                        self.context.max_tokens,
                        &self.llm,
                        budget.pressure,
                    )
                    .await?;

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

                return Ok(AgentStep::Continue);
            } else if !assistant_content.is_empty() {
                debug!(content = %assistant_content, "LLM responded directly");
                self.context.add_message(
                    crate::agent::context::Role::Assistant,
                    assistant_content.clone(),
                    None,
                    None,
                );
                self.context.increment_no_tool_rounds();

                // 持久化：记录助手文本回复
                if let Some(ref mut store) = self.session_store {
                    store.record_assistant_message(&assistant_content);
                }

                // 主 Agent 连续 2 轮纯文本回应即视为任务完成（交互式 REPL 场景）。
                // 子 Agent（depth > 0）不受此影响：必须显式调用 `finish` 工具终止，
                // 否则会撞上 max_iterations 上限——这是设计意图，确保子 Agent 产出
                // 明确的阶段交付物（finish 的 summary 参数），而非中途的纯文本解释。
                if self.depth == 0 && self.context.get_consecutive_no_tool_rounds() >= 2 {
                    return Ok(AgentStep::Done(AgentResult {
                        success: true,
                        message: assistant_content,
                        restart_requested: false,
                        finished: false,
                    }));
                }
                return Ok(AgentStep::Continue);
            } else {
                // LLM 返回空响应
                empty_attempt += 1;
                if empty_attempt < MAX_EMPTY_RETRIES {
                    let delay = Duration::from_secs(2u64.pow(empty_attempt));
                    output.warning(&format!(
                        "LLM 返回空响应，{}/{} 次重试，{}s 后重试...",
                        empty_attempt, MAX_EMPTY_RETRIES, delay.as_secs()
                    ));
                    tokio::time::sleep(delay).await;
                    continue;
                }
                output.error(&format!(
                    "LLM 返回空响应（已重试 {} 次）",
                    MAX_EMPTY_RETRIES
                ));
                return Err(AppError::Llm(format!(
                    "LLM returned empty response after {} retries",
                    MAX_EMPTY_RETRIES
                )));
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

        let hint = if self.depth > 0 {
            "（子代理未调用 `finish` 工具，必须显式调用 `finish` 才能正常终止阶段）"
        } else {
            ""
        };
        let message = format!(
            "已达到最大迭代次数 ({})，任务可能未完成{}",
            self.max_iterations, hint
        );
        output.warning(&message);
        Ok(AgentResult {
            success: false,
            message,
            restart_requested: false,
            finished: false,
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
    pub fn new_subagent(config: SubagentConfig) -> Result<Self, AppError> {
        if config.depth > MAX_SUBAGENT_DEPTH {
            return Err(AppError::SubagentDepthLimit(MAX_SUBAGENT_DEPTH));
        }

        let identity = config.agent_type.unwrap_or(AgentIdentity::General);
        let system_prompt = identity.system_prompt();

        // 崩溃恢复场景：直接使用重建的上下文（已包含恢复通知、分层摘要与继续指令），
        // 跳过任务消息注入，避免与恢复上下文中的"请从以下任务继续"指令冲突。
        if let Some(restored) = config.restored_context {
            let wd = config.tools.working_dir().clone();
            return Ok(Self {
                context: restored,
                tools: config.tools,
                async_tools: None,
                llm: config.llm,
                max_iterations: config.max_iterations,
                depth: config.depth,
                skills: Vec::new(),
                session_store: None,
                working_dir: wd,
            });
        }

        let context_manager = ContextManager::new(system_prompt, config.max_tokens);

        let mut ctx = context_manager;
        let mut task_description = if config.context.is_empty() {
            format!("任务目标：{}", config.task)
        } else {
            format!("任务目标：{}\n\n上下文信息：\n{}", config.task, config.context)
        };

        // 若父代理提供了上下文预算信息，附加到任务描述中，
        // 让子代理感知父代理的压力从而控制输出规模。
        if let Some(ref pb) = config.parent_budget {
            let pressure_str = match pb.pressure {
                crate::agent::context::ContextPressure::Normal => "Normal（充足）",
                crate::agent::context::ContextPressure::Warning => "Warning（注意）",
                crate::agent::context::ContextPressure::Critical => "Critical（紧张）",
                crate::agent::context::ContextPressure::Exhausted => "Exhausted（即将溢出）",
            };
            let utilization_pct = pb.utilization * 100.0;
            task_description.push_str(&format!(
                "\n\n### 父代理上下文状态\n\
                 父代理上下文压力：{pressure_str}\n\
                 父代理使用率：{utilization_pct:.0}%\n\
                 请尽快完成你的任务。父代理的上下文空间有限，\
                 你的结果需要能容纳回父代理的上下文中，请控制输出规模，\
                 只返回必要的关键信息。",
                pressure_str = pressure_str,
                utilization_pct = utilization_pct,
            ));
        }

        ctx.add_message(
            crate::agent::context::Role::User,
            task_description,
            None,
            None,
        );

        let wd = config.tools.working_dir().clone();
        Ok(Self {
            context: ctx,
            tools: config.tools,
            async_tools: None,
            llm: config.llm,
            max_iterations: config.max_iterations,
            depth: config.depth,
            skills: Vec::new(),
            session_store: None,
            working_dir: wd,
        })
    }

    /// 获取当前 Agent 的深度
    #[allow(dead_code)] // used by tests
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// 构建 stage 模板的 finish 提示后缀。
    ///
    /// 所有 6 个阶段共享相同的"完成工作后调用 finish 工具终止本阶段"提示格式，
    /// 仅输出描述和示例摘要不同。提取为辅助函数减少重复。
    fn finish_warning(output_desc: &str, example_summary: &str) -> String {
        format!(
            "✅ 完成所有上述工作后，请调用 `finish` 工具终止本阶段，\n\
             并将{}摘要作为 `summary` 参数传入，例如：\n\
             `finish(summary=\"{}\")`。",
            output_desc, example_summary
        )
    }

    /// 运行流水线：按 设计→编码→测试→审查→修复→记录 六个阶段顺序执行。
    ///
    /// 每个阶段创建一个对应身份的子 Agent，上下文通过文件系统存储。
    /// 支持断点续传（通过 `resume` 参数）。
    ///
    /// # 参数
    ///
    /// * `task` - 任务描述
    /// * `verbose` - 是否启用详细日志
    /// * `resume` - 是否从上次检查点恢复
    pub async fn run_pipeline(
        &mut self,
        task: &str,
        verbose: bool,
        resume: bool,
    ) -> Result<(), AppError> {
        // 初始化文件存储
        let pipeline_store = PipelineContextStore::new(&self.working_dir())?;

        // 从 .env 读取 MAX_ITERATIONS（默认 120），按阶段复杂度比例分配。
        // 权重比例 设计:编码:测试:审查:修复:记录 = 2:4:2:2:3:1（共 14 份）
        let env_max_iter: usize = std::env::var("MAX_ITERATIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(120);
        // 阶段权重（与下方 stages 一一对应）
        const STAGE_WEIGHTS: [usize; 6] = [2, 4, 2, 2, 3, 1];
        const TOTAL_WEIGHT: usize = 14;
        let alloc = |w: usize| -> usize {
            // 至少 5 轮，避免过小导致任务无法完成
            ((env_max_iter * w).div_ceil(TOTAL_WEIGHT)).max(5)
        };

        let stages: Vec<PipelineStage> = STAGE_TEMPLATES
            .iter()
            .enumerate()
            .map(|(i, (name, agent_type, template, output_desc, example_summary))| {
                let task_template = template
                    .replace("{task_ref}", task)
                    .replace("{finish}", &Self::finish_warning(output_desc, example_summary));
                PipelineStage {
                    name: name.to_string(),
                    agent_type: agent_type.clone(),
                    task_template,
                    max_iterations: alloc(STAGE_WEIGHTS[i]),
                }
            })
            .collect();

        let total = stages.len();
        let pipeline_start = std::time::Instant::now();

        // ── 恢复或初始化 Pipeline 上下文 ──
        let mut pipeline_ctx = if resume {
            match pipeline_store.load_checkpoint()? {
                Some(ctx) => {
                    info!("恢复 pipeline 从阶段 {} (已完成 {} 个阶段)",
                        ctx.current_stage,
                        ctx.stages.iter().filter(|s| s.status == StageStatus::Completed).count());
                    self.add_display_message(
                        crate::utils::message_level::MessageLevel::Info,
                        &format!("🔄 恢复 pipeline 从阶段 {} 开始...", ctx.current_stage + 1),
                    );
                    ctx
                }
                None => {
                    warn!("未找到 pipeline 检查点，从头开始");
                    let mut ctx = PipelineContext::new(task, total);
                    for (i, stage) in stages.iter().enumerate() {
                        ctx.stages[i].stage_name = stage.name.clone();
                    }
                    ctx
                }
            }
        } else {
            // 清除旧的 pipeline 数据
            let _ = pipeline_store.clear();
            let mut ctx = PipelineContext::new(task, total);
            for (i, stage) in stages.iter().enumerate() {
                ctx.stages[i].stage_name = stage.name.clone();
            }
            pipeline_store.save_pipeline_context(&ctx)?;
            ctx
        };

        // 初始进度条
        let _ = crate::ui::render_progress_bar(
            pipeline_ctx.current_stage,
            total,
            &stages[pipeline_ctx.current_stage.min(total - 1)].name,
            &pipeline_start,
            "准备就绪",
        );

        // ── 执行阶段循环 ──
        while pipeline_ctx.current_stage < total {
            let stage_idx = pipeline_ctx.current_stage;
            let stage = &stages[stage_idx];

            // 更新阶段状态为进行中
            let stage_ctx = &mut pipeline_ctx.stages[stage_idx];
            stage_ctx.status = StageStatus::InProgress;

            // 更新进度条
            let _ = crate::ui::render_progress_bar(
                stage_idx,
                total,
                &stage.name,
                &pipeline_start,
                "进行中...",
            );

            self.add_display_message(
                crate::utils::message_level::MessageLevel::Info,
                &format!("🔄 阶段 {}/{}: {}...", stage_idx + 1, total, stage.name),
            );

            // 构建上下文提示词（从已完成的阶段）
            let context_prompt = pipeline_ctx.build_context_prompt();
            let stage_task = stage.task_template.replace("{context}", &context_prompt);

            let subagent_tools = self.tools
                .new_subagent_registry_with_identity(&stage.agent_type);

            let mut subagent = match Agent::new_subagent(SubagentConfig {
                llm: self.llm.clone(),
                tools: subagent_tools,
                depth: self.depth + 1,
                task: stage_task.clone(),
                context: String::new(),
                max_iterations: stage.max_iterations,
                max_tokens: self.context.max_tokens,
                agent_type: Some(stage.agent_type.clone()),
                parent_budget: Some(self.context.get_budget_report()),
                restored_context: None,
            }) {
                Ok(agent) => agent,
                Err(e) => {
                    let stage_ctx = &mut pipeline_ctx.stages[stage_idx];
                    stage_ctx.status = StageStatus::Failed;
                    stage_ctx.error = Some(format!("创建子代理失败: {}", e));
                    let _ = pipeline_store.save_stage_context(stage_ctx);
                    let _ = pipeline_store.save_checkpoint(&pipeline_ctx);

                    self.add_display_message(
                        crate::utils::message_level::MessageLevel::Error,
                        &format!("❌ 创建阶段 \"{}\" 的子代理失败: {}", stage.name, e),
                    );
                    return Err(e);
                }
            };

            let stage_type_name = stage.agent_type.to_str();
            let mut sub_output = crate::ui::RealtimeOutput::new(verbose, self.depth + 1, stage_type_name);
            let result = Box::pin(subagent.run(stage_task, &mut sub_output)).await;

            // 收集子代理输出消息到主 Agent 的显示缓冲区
            for (level, msg) in sub_output.drain() {
                self.add_display_message(level, &msg);
            }

            let stage_ctx = &mut pipeline_ctx.stages[stage_idx];

            match result {
                Ok(agent_result) if agent_result.success => {
                    stage_ctx.status = StageStatus::Completed;
                    stage_ctx.summary = agent_result.message.clone();
                    // 检测修改的文件（通过 git diff 或 kb_store 记录）
                    if let Ok(files) = detect_modified_files(&self.working_dir()) {
                        stage_ctx.modified_files = files;
                    }
                    // 记录产物引用（与阶段模板中指示 LLM 保存的目录保持一致）
                    stage_ctx.artifacts.push(format!("pipeline/stage-{}", stage_idx));

                    // 保存阶段上下文到文件
                    let _ = pipeline_store.save_stage_context(stage_ctx);
                    // 更新 pipeline 上下文索引
                    let _ = pipeline_store.save_pipeline_context(&pipeline_ctx);
                    // 保存检查点
                    let _ = pipeline_store.save_checkpoint(&pipeline_ctx);

                    self.add_display_message(
                        crate::utils::message_level::MessageLevel::Success,
                        &format!("✅ 阶段 \"{}\" 完成", stage.name),
                    );

                    pipeline_ctx.current_stage += 1;
                }
                Ok(agent_result) => {
                    stage_ctx.status = StageStatus::Failed;
                    stage_ctx.error = Some(agent_result.message.clone());

                    // 保存失败信息
                    let _ = pipeline_store.save_stage_context(stage_ctx);
                    let _ = pipeline_store.save_checkpoint(&pipeline_ctx);

                    self.add_display_message(
                        crate::utils::message_level::MessageLevel::Error,
                        &format!("❌ 阶段 \"{}\" 失败: {}", stage.name, agent_result.message),
                    );
                    return Err(AppError::Config(format!(
                        "流水线阶段 '{}' 失败. 使用 --resume-pipeline 可从当前阶段恢复。\n错误: {}",
                        stage.name, agent_result.message
                    )));
                }
                Err(e) => {
                    stage_ctx.status = StageStatus::Failed;
                    stage_ctx.error = Some(format!("{}", e));

                    // 保存失败信息
                    let _ = pipeline_store.save_stage_context(stage_ctx);
                    let _ = pipeline_store.save_checkpoint(&pipeline_ctx);

                    self.add_display_message(
                        crate::utils::message_level::MessageLevel::Error,
                        &format!("❌ 阶段 \"{}\" 出错: {}", stage.name, e),
                    );
                    return Err(AppError::Config(format!(
                        "流水线阶段 '{}' 出错: {}\n使用 --resume-pipeline 可从当前阶段恢复。",
                        stage.name, e
                    )));
                }
            }
        }

        // 完成：100% 进度
        let _ = crate::ui::render_progress_bar(total, total, "全部完成", &pipeline_start, "🎉 流水线执行完成！");

        // 清理检查点（pipeline 已成功完成）
        let _ = pipeline_store.clear();

        self.add_display_message(
            crate::utils::message_level::MessageLevel::Success,
            "🎉 流水线执行完成！所有阶段已成功完成。",
        );

        Ok(())
    }

    /// 获取工作目录的引用。
    /// 直接从 Agent 存储的字段中返回，避免依赖 `std::env::current_dir()`。
    fn working_dir(&self) -> std::path::PathBuf {
        self.working_dir.clone()
    }

    /// 获取历史消息列表（用于 UI 展示等）
    #[allow(dead_code)] // reserved for future UI extension
    pub fn history_messages(&self) -> &[crate::llm::LlmMessage] {
        &self.context.history.messages
    }

    // -----------------------------------------------------------------------
    // 内部方法
    // -----------------------------------------------------------------------

    /// 将 AppError 分类为错误类别，用于重试逻辑判断
    fn categorize_error(e: &AppError) -> crate::tools::ErrorCategory {
        match e {
            // 可重试的临时性错误
            AppError::RateLimited { .. } => crate::tools::ErrorCategory::Transient,
            AppError::Llm(_) => crate::tools::ErrorCategory::Llm,
            AppError::Http(e) if e.is_timeout() || e.is_connect() => crate::tools::ErrorCategory::Transient,
            AppError::Io(e) if e.kind() == std::io::ErrorKind::Interrupted => crate::tools::ErrorCategory::Transient,
            // 不可重试的永久性错误
            AppError::ToolNotFound(_) => crate::tools::ErrorCategory::Permanent,
            AppError::Security(_) => crate::tools::ErrorCategory::Permanent,
            AppError::Config(_) => crate::tools::ErrorCategory::Permanent,
            AppError::SubagentDepthLimit(_) => crate::tools::ErrorCategory::Permanent,
            AppError::Json(_) => crate::tools::ErrorCategory::Permanent,
            AppError::Env(_) => crate::tools::ErrorCategory::Permanent,
            AppError::Glob(_) => crate::tools::ErrorCategory::Permanent,
            AppError::Walkdir(_) => crate::tools::ErrorCategory::Permanent,
            // 其他错误默认为临时性
            _ => crate::tools::ErrorCategory::Transient,
        }
    }

    async fn process_tool_calls(&mut self, tool_calls: &[ToolCall], output: &mut dyn MessageOutput) -> Result<Vec<ToolResult>, AppError> {
        let mut results = Vec::new();

        for tool_call in tool_calls {
            // 工具调用开始：显示工具名称和参数摘要
            let display_name = if tool_call.function.name.is_empty() {
                "未知工具"
            } else {
                &tool_call.function.name
            };
            let args_summary = crate::ui::blocks::MessageBlock::summarize_tool_args(&tool_call.function.name, &tool_call.function.arguments);
            output.info(&format!("🔧 {} ({})", display_name, args_summary));
            debug!(tool = %tool_call.function.name, args = %tool_call.function.arguments, "Tool arguments");

            // ── 提前拦截空名工具调用 ──
            if tool_call.function.name.is_empty() {
                output.error("工具名称不能为空，请检查 LLM 输出");
                let result = ToolResult::failure(
                    "工具名称不能为空：LLM 返回了空的 tool_call.function.name".into(),
                    crate::tools::ErrorCategory::Permanent,
                );
                results.push(result);
                continue;
            }

            // ── 拦截 spawn_subagent 工具调用 ──
            if tool_call.function.name == "spawn_subagent" {
                let result = self.handle_spawn_subagent(tool_call, output).await?;
                results.push(result);
                continue;
            }

            // ── 拦截上下文管理工具调用（context_budget / compress_context / save_summary）──
            if matches!(
                tool_call.function.name.as_str(),
                "context_budget" | "compress_context" | "save_summary"
            ) {
                let result = self.handle_context_tool(tool_call).await?;
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
                            Err(e) => {
                                let category = Self::categorize_error(&e);
                                ToolResult::failure(
                                    format!("[error] Tool '{}' execution failed: {}", tool_call.function.name, e),
                                    category,
                                )
                            }
                        }
                    }
                    Err(e) => {
                        let category = Self::categorize_error(&e);
                        ToolResult::failure(
                            format!("[error] Tool '{}' execution failed: {}", tool_call.function.name, e),
                            category,
                        )
                    }
                }
            } else {
                // 没有异步工具注册表，使用同步工具
                match self.tools.execute_with_policy(
                    &tool_call.function.name,
                    tool_call.function.arguments.clone(),
                ) {
                    Ok(r) => r,
                    Err(e) => {
                        let category = Self::categorize_error(&e);
                        ToolResult::failure(
                            format!("[error] Tool '{}' execution failed: {}", tool_call.function.name, e),
                            category,
                        )
                    }
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

            let tool_name = &tool_call.function.name;

            if result.success {
                // 将执行成功的状态与结果内容合并为一条消息（避免 output.info 被 verbose 过滤）
                let preview = result.content.lines().next().unwrap_or(&result.content);
                let preview = if preview.len() > 200 {
                    // 找到第 200 个字符边界，避免切到多字节 UTF-8 字符中间
                    let end = preview.char_indices().nth(200).map(|(i, _)| i).unwrap_or(preview.len());
                    format!("{}...", &preview[..end])
                } else {
                    preview.to_string()
                };
                if !preview.is_empty() && preview != "()" {
                    output.success(&format!("工具 {} 执行成功：{}", tool_name, preview));
                } else {
                    output.success(&format!("工具 {} 执行成功", tool_name));
                }
            } else {
                output.error(&format!("工具 {} 执行失败", tool_name));
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
            // 继承父 agent 的 max_iterations，但至少保证 30 轮，
            // 避免子 agent 在复杂任务中因轮次不足而失败。
            .unwrap_or_else(|| self.max_iterations.max(30));
        let sub_max_tokens = tool_call.function.arguments["max_tokens"]
            .as_u64()
            .map(|n| n as usize)
            .unwrap_or(262144);
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
                error_category: None,
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
            if task.len() > 80 {
                let truncated: String = task.chars().take(80).collect();
                format!("{}...", truncated)
            } else {
                task.to_string()
            }
        ));

        let subagent_tools = if let Some(ref identity) = agent_type {
            self.tools.new_subagent_registry_with_identity(identity)
        } else {
            self.tools.new_subagent_registry()
        };

        let mut subagent = match Agent::new_subagent(SubagentConfig {
            llm: self.llm.clone(),
            tools: subagent_tools,
            depth: child_depth,
            task: task.to_string(),
            context: context.to_string(),
            max_iterations: sub_max_iterations,
            max_tokens: sub_max_tokens,
            agent_type: agent_type.clone(),
            parent_budget: Some(self.context.get_budget_report()),
            restored_context: None,
        }) {
            Ok(agent) => agent,
            Err(e) => {
                output.error(&format!("创建子代理失败: {}", e));
                return Ok(ToolResult {
                    success: false,
                    security_evaluation: None,
                    restart_requested: false,
                    error_category: None,
                    content: format!("[spawn_subagent] ❌ 创建子代理失败: {}", e),
                });
            }
        };

        let mut sub_output = crate::ui::RealtimeOutput::new(false, child_depth, agent_type_str);
        let result = Box::pin(subagent.run(task.to_string(), &mut sub_output)).await;

        match result {
            Ok(agent_result) => {
                if agent_result.success {
                    output.success("子代理任务完成");
                    Ok(ToolResult {
                        success: true,
                        security_evaluation: None,
                        restart_requested: false,
                        error_category: None,
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
                        error_category: None,
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
                    error_category: None,
                    content: format!("[spawn_subagent] ❌ 子代理执行出错: {}", e),
                })
            }
        }
    }

    /// 处理上下文管理工具调用（context_budget / compress_context / save_summary）。
    ///
    /// 与 `spawn_subagent` 相同的拦截模式：这些工具需要访问 Agent 的上下文，
    /// 因此由 Agent 层直接处理，而非通过 ToolRegistry 的通用 handler。
    async fn handle_context_tool(&mut self, tool_call: &ToolCall) -> Result<ToolResult, AppError> {
        let name = tool_call.function.name.as_str();
        match name {
            "context_budget" => {
                let report = self.context.budget_report_json();
                Ok(ToolResult::success(format!(
                    "当前上下文预算使用情况：\n{}",
                    report
                )))
            }
            "compress_context" => {
                // 解析策略参数：auto=根据压力等级自动选择，summarize=摘要压缩，truncate=截断
                let strategy = tool_call.function.arguments["strategy"]
                    .as_str()
                    .unwrap_or("auto");
                let before = self.context.history.used_tokens;

                // 选择压缩策略
                use crate::agent::compressor::{CompressionStrategy, ContextCompressor};
                let budget = self.context.get_budget_report();
                let use_summarize = match strategy {
                    "summarize" => true,
                    "truncate" => false,
                    // auto：仅在压力等级较高时使用摘要压缩（保留语义），
                    // 正常压力下截断更快速且足够。
                    _ => matches!(
                        budget.pressure,
                        crate::agent::context::ContextPressure::Critical
                            | crate::agent::context::ContextPressure::Exhausted
                    ),
                };

                let info = if use_summarize {
                    ContextCompressor::summarize(&mut self.context.history, &self.llm).await?
                } else {
                    self.context.compress()?
                };

                if info.did_compress {
                    let strategy_name = match info.strategy.unwrap_or(CompressionStrategy::Truncate) {
                        CompressionStrategy::Summarize => "summarize",
                        CompressionStrategy::Truncate => "truncate",
                    };
                    Ok(ToolResult::success(format!(
                        "上下文已压缩（策略: {}）：{} → {} tokens（保留 {} 轮）",
                        strategy_name, before, info.after_tokens, info.kept_rounds
                    )))
                } else {
                    Ok(ToolResult::success(
                        "上下文未达到压缩阈值，无需压缩。可使用 context_budget 查看当前使用情况。"
                            .to_string(),
                    ))
                }
            }
            "save_summary" => {
                let content = tool_call.function.arguments["content"]
                    .as_str()
                    .unwrap_or("");
                if content.trim().is_empty() {
                    return Ok(ToolResult::failure(
                        "save_summary: 'content' 参数不能为空".to_string(),
                        crate::tools::ErrorCategory::Permanent,
                    ));
                }
                let msg = self.save_layered_summary(content).await?;
                Ok(ToolResult::success(msg))
            }
            _ => Ok(ToolResult::failure(
                format!("unknown context tool: {}", name),
                crate::tools::ErrorCategory::Permanent,
            )),
        }
    }

    /// 保存一条摘要到分层摘要系统，并自动聚合阶段/会话摘要。
    ///
    /// 供 `save_summary` 工具和自动摘要（`auto_save_round_summary`）共用。
    /// 逻辑：
    /// 1. 保存轮次摘要（层级 1，round-{n}.md）
    /// 2. 若凑满一个阶段（每 ROUNDS_PER_PHASE 轮），聚合为阶段摘要（层级 2）
    /// 3. 若已积累多个阶段摘要，聚合为会话摘要（层级 3，final.md）
    ///
    /// 返回格式化后的保存结果消息。
    async fn save_layered_summary(&mut self, content: &str) -> Result<String, AppError> {
        use crate::agent::summary::{aggregate_summaries, phase_number, SummaryStore};
        let kb_root = self.working_dir().join(".kb");
        let store = SummaryStore::new(&self.context.session_id, &kb_root);

        // 1. 计算下一个轮次编号（已有轮次 + 1）
        let existing_rounds = store.load_rounds()?;
        let next_round = existing_rounds
            .iter()
            .map(|r| r.round)
            .max()
            .unwrap_or(0)
            + 1;

        // 2. 保存轮次摘要（层级 1）
        store.save_round(next_round, content)?;

        // 3. 若凑满一个阶段（每 ROUNDS_PER_PHASE 轮），聚合为阶段摘要（层级 2）
        let mut phase_aggregated = None;
        let phase = phase_number(next_round);
        let phase_start = (phase - 1) * crate::agent::summary::ROUNDS_PER_PHASE + 1;
        let phase_end = phase * crate::agent::summary::ROUNDS_PER_PHASE;
        let phase_rounds: Vec<String> = existing_rounds
            .iter()
            .filter(|r| r.round >= phase_start && r.round <= phase_end)
            .map(|r| r.content.clone())
            .collect();
        // 若当前轮恰好是阶段末轮，且已有足够轮次，则聚合阶段摘要
        if next_round == phase_end && phase_rounds.len() + 1 >= crate::agent::summary::ROUNDS_PER_PHASE {
            let mut items = phase_rounds;
            items.push(content.to_string());
            let phase_summary = aggregate_summaries(&self.llm, "轮次", &items, 500).await?;
            if !phase_summary.is_empty() {
                store.save_phase(phase_start, phase_end, &phase_summary)?;
                phase_aggregated = Some(phase);
            }
        }

        // 4. 若已积累多个阶段摘要，聚合为会话摘要（层级 3，final.md）
        let mut final_aggregated = None;
        let phases = store.load_phases()?;
        if phases.len() >= 2 {
            let items: Vec<String> = phases.iter().map(|p| p.content.clone()).collect();
            let final_summary = aggregate_summaries(&self.llm, "阶段", &items, 1000).await?;
            if !final_summary.is_empty() {
                store.save_final(&final_summary)?;
                final_aggregated = Some(phases.len());
            }
        }

        let mut msg = format!(
            "摘要已保存为轮次 {}（.kb/summaries/{}/round-{}.md，约 {} tokens）",
            next_round,
            self.context.session_id,
            next_round,
            crate::agent::token_counter::TokenCounter::estimate(content)
        );
        if let Some(p) = phase_aggregated {
            msg.push_str(&format!("\n已聚合为阶段摘要 phase-{}", p));
        }
        if let Some(n) = final_aggregated {
            msg.push_str(&format!("\n已聚合为会话摘要 final.md（{} 个阶段）", n));
        }
        Ok(msg)
    }

    /// 自动保存当前轮次摘要到分层摘要系统。
    ///
    /// 在上下文压缩前调用（当上下文压力达到 Critical/Exhausted 时），
    /// 确保被压缩丢弃的历史信息仍可通过分层摘要回溯，避免信息永久丢失。
    async fn auto_save_round_summary(&mut self) -> Result<(), AppError> {
        let content = self.build_auto_summary();
        let _ = self.save_layered_summary(&content).await?;
        tracing::info!(
            session = %self.context.session_id,
            "上下文压力达 Critical，已自动保存轮次摘要到分层摘要系统"
        );
        Ok(())
    }

    /// 从对话历史中构建自动摘要内容（最近若干轮的关键消息）。
    fn build_auto_summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for msg in self.context.history.messages.iter().rev() {
            match msg.role.as_str() {
                "user" => parts.push(format!("- 用户: {}", msg.content.as_deref().unwrap_or(""))),
                "assistant" => parts.push(format!("- 助手: {}", msg.content.as_deref().unwrap_or(""))),
                _ => {}
            }
            if parts.len() >= 8 {
                break;
            }
        }
        parts.reverse();
        if parts.is_empty() {
            "（自动摘要：无可提取的对话内容）".to_string()
        } else {
            parts.join("\n")
        }
    }

    /// 匹配用户消息与已注册技能。使用预计算的关键词索引进行快速匹配。
    ///
    /// 改进：
    /// - 使用正则表达式进行单词边界匹配，避免子串误激活
    /// - 检测否定模式（如"不要"、"不需要"、"取消"、"stop"）避免反向触发
    fn match_skill(&self, message: &str) -> Option<&Skill> {
        let msg_lower = message.to_lowercase();

        // 否定模式：如果消息中包含这些词，降低匹配优先级
        // 检查关键词周围是否有否定词
        fn has_negation_nearby(msg: &str, keyword_pos: usize) -> bool {
            let start = keyword_pos.saturating_sub(20);
            let end = (keyword_pos + 20).min(msg.len());
            let context = &msg[start..end];
            let negations = ["不", "不要", "不需要", "取消", "跳过", "忽略", "no", "don't", "not", "skip", "stop", "不需要", "不用"];
            negations.iter().any(|n| context.contains(n))
        }

        self.skills.iter().find(|skill| {
            skill.keywords.iter().any(|kw| {
                let kw_lower = kw.to_lowercase();
                // 查找关键词在消息中的位置
                if let Some(pos) = msg_lower.find(&kw_lower) {
                    // 检查关键词周围是否有否定词
                    if has_negation_nearby(&msg_lower, pos) {
                        return false; // 否定模式，不激活
                    }
                    // 对于英文关键词，检查单词边界
                    if kw_lower.bytes().all(|b| b.is_ascii_alphabetic() || b == b' ' || b == b'-') {
                        // 确保关键词前后不是字母数字（单词边界）
                        let before = pos.checked_sub(1).map(|i| msg_lower.as_bytes()[i]).unwrap_or(b' ');
                        let after = msg_lower.as_bytes().get(pos + kw_lower.len()).copied().unwrap_or(b' ');
                        
                        !before.is_ascii_alphanumeric() && !after.is_ascii_alphanumeric()
                    } else {
                        // 中文关键词直接匹配
                        true
                    }
                } else {
                    false
                }
            })
        })
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

    /// 获取共享的 LLM 客户端引用（供 dream 等外部模块复用）。
    pub fn llm_client(&self) -> &Arc<LlmClient> {
        &self.llm
    }
}

/// 检测工作目录下被修改的文件列表（通过 git diff）。
/// 用于 pipeline 各阶段自动记录修改的文件。
fn detect_modified_files(working_dir: &std::path::Path) -> Result<Vec<String>, AppError> {
    let output = std::process::Command::new("git")
        .args(["diff", "--name-only"])
        .current_dir(working_dir)
        .output()
        .map_err(|e| {
            AppError::Io(std::io::Error::new(e.kind(), format!("git diff 失败: {}", e)))
        })?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let files: Vec<String> = stdout
            .lines()
            .map(|l| l.to_string())
            .filter(|l| !l.is_empty())
            .collect();
        Ok(files)
    } else {
        // git diff 失败时返回空列表（非关键错误）
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmClient;
    use crate::security::SecurityPolicy;
    use crate::tools::ToolRegistry;
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

    /// 创建一个测试用的 ToolRegistry（使用临时目录避免路径遍历问题）
    fn test_tool_registry() -> ToolRegistry {
        let dir = tempfile::tempdir().unwrap();
        let policy = Arc::new(SecurityPolicy::new(dir.path(), true));
        ToolRegistry::new(dir.path().to_path_buf(), policy)
    }

    /// 创建测试用的 SubagentConfig
    fn test_subagent_config(depth: usize, task: &str, context: &str) -> SubagentConfig {
        SubagentConfig {
            llm: test_llm_client(),
            tools: test_tool_registry(),
            depth,
            task: task.to_string(),
            context: context.to_string(),
            max_iterations: 10,
            max_tokens: 4096,
            agent_type: None,
            parent_budget: None,
            restored_context: None,
        }
    }

    #[test]
    fn new_subagent_depth_limit_exceeded() {
        // 深度超过 MAX_SUBAGENT_DEPTH 时应返回错误
        let result = Agent::new_subagent(test_subagent_config(
            MAX_SUBAGENT_DEPTH + 1,
            "test task",
            "",
        ));

        assert!(result.is_err());
        match result {
            Err(AppError::SubagentDepthLimit(max)) => assert_eq!(max, MAX_SUBAGENT_DEPTH),
            _ => panic!("Expected SubagentDepthLimit error"),
        }
    }

    #[test]
    fn new_subagent_creates_at_depth_limit() {
        // 深度等于 MAX_SUBAGENT_DEPTH 时应成功
        let result = Agent::new_subagent(test_subagent_config(
            MAX_SUBAGENT_DEPTH,
            "test task",
            "",
        ));

        assert!(result.is_ok());
        let agent = result.unwrap();
        assert_eq!(agent.depth(), MAX_SUBAGENT_DEPTH);
    }

    #[test]
    fn new_subagent_has_no_skills() {
        let agent = Agent::new_subagent(test_subagent_config(1, "test task", "")).unwrap();

        // 子代理不应有技能
        assert!(agent.skills.is_empty());
    }

    #[test]
    fn new_subagent_has_no_session_store() {
        let agent = Agent::new_subagent(test_subagent_config(1, "test task", "")).unwrap();

        // 子代理不应有 session_store
        assert!(agent.session_store.is_none());
    }

    #[test]
    fn new_subagent_context_contains_task() {
        let agent = Agent::new_subagent(test_subagent_config(1, "特定任务描述", "")).unwrap();

        // 验证任务描述出现在上下文中
        let messages = agent.context.build_messages();
        let user_msg = messages.iter().find(|m| m.role == "user");
        assert!(user_msg.is_some(), "sub-agent should have a user message with task");
        let content = user_msg.unwrap().content.as_ref().unwrap();
        assert!(content.contains("特定任务描述"), "task description should be in user message");
    }

    #[test]
    fn new_subagent_context_includes_context_param() {
        let agent = Agent::new_subagent(test_subagent_config(1, "test task", "额外上下文信息")).unwrap();

        let messages = agent.context.build_messages();
        let user_msg = messages.iter().find(|m| m.role == "user").unwrap();
        let content = user_msg.content.as_ref().unwrap();
        assert!(content.contains("额外上下文信息"), "context param should be in user message");
    }

    #[test]
    fn new_subagent_registry_excludes_spawn_subagent() {
        let dir = tempfile::tempdir().unwrap();
        let policy = Arc::new(SecurityPolicy::new(dir.path(), true));
        let parent_registry = ToolRegistry::new(dir.path().to_path_buf(), policy);

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
