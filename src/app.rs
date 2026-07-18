//! 应用协调层：组装各组件、提供 App 入口。

use std::path::PathBuf;

use crate::agent::{Agent, AgentConfig, AgentStep, ContextManager};
use crate::config::{load_agent_config, load_models};
use crate::llm::LlmClient;
use crate::prompt::build_system_prompt;
use crate::security::SecurityPolicy;
use crate::session::SessionLogger;
use crate::skills::{default_skills_dir, discover_skills, Skill};
use crate::tools::ToolRegistry;
use crate::ui::{self, CliMessageOutput, UIMessageOutput};
use crate::utils::message_level::MessageLevel;
use crate::utils::message_output::MessageOutput;
use crate::utils::error::AppError;

/// 应用配置：由 CLI 参数 + 环境变量组装。
pub struct AppConfig {
    pub working_dir: PathBuf,
    pub verbose: bool,
    pub max_iterations: usize,
    pub max_tokens: usize,
    pub no_approval: bool,
    pub provider: String,
    pub model: Option<String>,
    pub message: Option<String>,
    pub resume: bool,
    /// 传给 restart 子进程的 CLI 参数列表（不含 argv[0]）
    pub restart_args: Vec<String>,
}

/// 应用实例：持有 Agent 和配置，提供运行入口。
pub struct App {
    agent: Agent<'static>,
    config: AppConfig,
    skills: Vec<Skill>,
    system_prompt: String,
}

impl App {
    /// 从配置构建应用实例。
    ///
    /// `working_dir` 用于工具执行和工作目录约束。
    /// 内部会创建 `ToolRegistry` 和 `Agent`，并发现项目技能。
    pub fn build(config: AppConfig) -> Result<Self, AppError> {
        if !config.working_dir.exists() {
            return Err(AppError::Config(format!(
                "Project directory does not exist: {}",
                config.working_dir.display()
            )));
        }

        // 加载模型配置（TOML 优先，fallback 到环境变量）
        let mut provider_configs = load_models(&config.working_dir)?;

        // CLI 参数覆盖：--model 和 --provider
        if let Some(ref model) = config.model {
            if let Some(first) = provider_configs.first_mut() {
                first.model = model.clone();
            }
        }
        if config.provider != "openai" {
            if let Some(first) = provider_configs.first_mut() {
                first.provider = config.provider.clone();
            }
        }

        // SAFETY: 我们使用 `Box::leak` 把 `SecurityPolicy` 泄漏成 `'static`，
        // 因为 `ToolRegistry<'a>` 的生命周期绑定到 `security` 引用，
        // 而我们希望把 `Agent<'a>` 存进 struct，方便后续调用。
        // 这种泄漏在每个进程生命周期内只发生一次，可以接受。
        let security: &'static SecurityPolicy = Box::leak(Box::new(SecurityPolicy::new(
            &config.working_dir,
            !config.no_approval,
        )));
        let tools = ToolRegistry::new(config.working_dir.clone(), security);
        let llm_client = LlmClient::from_configs(provider_configs)?;

        // 发现项目技能
        let skills_dir = default_skills_dir(&config.working_dir);
        let discovered_skills = discover_skills(&skills_dir).unwrap_or_default();

        // 构建 system prompt
        let tool_schemas = tools.get_tool_schemas();
        let system_prompt = build_system_prompt(&tool_schemas, &discovered_skills);

        // 创建或恢复 ContextManager
        let context = if config.resume {
            Self::load_state_or_fresh(&config, &system_prompt)
        } else {
            ContextManager::new(system_prompt.clone(), config.max_tokens)
        };

        let env_config = load_agent_config();
        let max_iterations: usize = if config.max_iterations > 0 {
            config.max_iterations
        } else {
            env_config.max_iterations
        };

        let agent_config = AgentConfig { max_iterations };
        let mut agent = Agent::new(context, tools, llm_client, agent_config, discovered_skills.clone());

        // 恢复持久化的活跃模型
        let saved_model = agent.context.active_model.clone();
        if let Some(ref model_name) = saved_model {
            if let Err(e) = agent.switch_model(model_name) {
                tracing::info!(model = %model_name, error = %e, "Failed to restore saved model, using default");
            } else {
                tracing::info!(model = %model_name, "Restored saved model");
            }
        }

        Ok(Self {
            agent,
            config,
            skills: discovered_skills,
            system_prompt,
        })
    }

    fn load_state_or_fresh(
        config: &AppConfig,
        system_prompt: &str,
    ) -> ContextManager {
        let state_path = config.working_dir.join(crate::repl::STATE_FILE);
        tracing::info!(path = %state_path.display(), "Attempting to resume from saved state");

        match ContextManager::load_state(&state_path) {
            Ok(mut ctx) => {
                tracing::info!("Successfully resumed conversation state");

                // Remove restart-related messages from history so the LLM
                // doesn't see a stale "restart requested" and try again.
                ctx.history.retain(|msg| {
                    if msg.role == "tool" {
                        if let Some(ref content) = msg.content {
                            if content.starts_with("[restart]") {
                                return false;
                            }
                        }
                    }
                    if msg.role == "assistant" {
                        if let Some(ref content) = msg.content {
                            if content.contains("\"name\":\"restart\"")
                                || content.contains("\"name\": \"restart\"")
                            {
                                return false;
                            }
                        }
                    }
                    true
                });

                // Clear stale display messages from the previous session so the
                // UI doesn't show duplicate or outdated status messages.
                ctx.display.clear_messages();
                ctx.display.history_start = ctx.history.len();

                // Inject a system-level directive after restart so the LLM
                // knows a restart just occurred and must not call restart again.
                ctx.add_message(
                    crate::agent::context::Role::System,
                    "【系统】注意：程序刚刚通过 restart 工具完成重启并恢复对话。请继续执行用户任务，不要再次调用 restart 工具。".to_string(),
                    None,
                    None,
                );

                let _ = std::fs::remove_file(&state_path);
                ctx
            }
            Err(e) => {
                tracing::info!(error = %e, "Failed to resume state, starting fresh");
                ContextManager::new(system_prompt.to_string(), config.max_tokens)
            }
        }
    }

    /// 运行应用：根据是否有 `--message` 选择交互 REPL 或一次性模式。
    pub async fn run(&mut self) -> Result<(), AppError> {
        let message = self.config.message.clone();
        if let Some(message) = message {
            self.run_once(&message).await
        } else {
            self.run_interactive().await
        }
    }

    /// 非交互模式：执行单条消息后退出。
    async fn run_once(&mut self, message: &str) -> Result<(), AppError> {
        let mut output = CliMessageOutput::new(self.config.verbose);
        output.info(&format!("项目目录: {}", self.config.working_dir.display()));
        output.info(&format!("模型: {}", self.agent.active_model()));

        let result = self.agent.run(message.to_string(), &mut output).await?;
        if result.success {
            output.success(&result.message);
        } else {
            output.error(&result.message);
        }
        Ok(())
    }

    /// 交互模式：进入 REPL。
    async fn run_interactive(&mut self) -> Result<(), AppError> {
        // 先打印欢迎信息和创建会话日志
        println!("🚀 Dev-Assistant Rust CLI");
        println!("Project: {}", self.config.working_dir.display());
        println!("Type '/quit' or '/exit' to quit.\n");

        let mut session_log = SessionLogger::create(&self.config.working_dir)?;
        session_log.log_status("信息", &format!("项目目录: {}", self.config.working_dir.display()));
        session_log.log_status("信息", &format!("模型: {}", self.agent.active_model()));

        loop {
            // Render the split-pane UI: messages on top, input at bottom
            let messages = self.agent.context.get_display_messages();
            ui::render(&messages, &self.agent.context.display.messages, None, self.config.verbose)?;

            let mut input = String::new();
            let bytes_read = std::io::stdin()
                .read_line(&mut input)
                .map_err(AppError::Io)?;

            if bytes_read == 0 {
                print!("\x1b[2J\x1b[H");
                println!("👋 Goodbye!");
                break;
            }

            let input = input.trim().to_string();

            // ── Slash 命令分发 ──
            if input.starts_with('/') {
                use crate::repl::{handle_slash, SlashOutcome};
                let working_dir = self.config.working_dir.clone();
                match handle_slash(&input, &mut self.agent, &working_dir) {
                    Some(SlashOutcome::Quit) => break,
                    Some(SlashOutcome::Continue) => {
                        let messages = self.agent.context.get_display_messages();
                        ui::render(&messages, &self.agent.context.display.messages, None, self.config.verbose)?;
                        continue;
                    }
                    None => {}
                }
            }

            if input.is_empty() {
                continue;
            }

            // 处理一次用户消息
            let action = self
                .process_user_message(&input, &mut session_log)
                .await?;
            match action {
                ReplAction::Continue => continue,
                ReplAction::Quit => break,
            }
        }

        Ok(())
    }

    async fn process_user_message(
        &mut self,
        input: &str,
        session_log: &mut SessionLogger,
    ) -> Result<ReplAction, AppError> {
        // Clear stale display messages from previous turn so they
        // don't accumulate and stack on each render.
        self.agent.context.display.messages.clear();
        // 记录当前 history 位置，get_display_messages 只显示此后的消息
        self.agent.context.display.history_start = self.agent.context.history.len();

        // Clear the screen before agent execution so that any tracing
        // logs (which go to stderr) don't appear inside the split-pane UI.
        print!("\x1b[2J\x1b[H");
        {
            use std::io::Write;
            std::io::stdout().flush().map_err(AppError::Io)?;
        }

        // ── Step-by-step agent loop with real-time UI updates ──
        let mut output = UIMessageOutput::new(self.config.verbose);
        session_log.log_user(input);
        self.agent.start_turn(input.to_string(), &mut output);

        let result = loop {
            // Drain buffered messages and re-render UI
            for (level, msg) in output.drain() {
                let label = level.label();
                session_log.log_status(label, &msg);
                self.agent.context.add_display_message(level, &msg);
            }
            let messages = self.agent.context.get_display_messages();
            ui::render(&messages, &self.agent.context.display.messages, None, self.config.verbose)?;

            // Show "thinking" indicator in the input area so it doesn't
            // get drowned out by subsequent messages in the message panel.
            session_log.log_thinking();
            let messages = self.agent.context.get_display_messages();
            ui::render(
                &messages,
                &self.agent.context.display.messages,
                Some("⏳ LLM 正在思考，请稍候..."),
                self.config.verbose,
            )?;

            tokio::select! {
                step_result = self.agent.step(&mut output) => {
                    match step_result {
                        Ok(AgentStep::Done(result)) => break Some(result),
                        Ok(AgentStep::Continue) => continue,
                        Err(e) => {
                            let msg = format!("LLM API 错误: {}", e);
                            output.error(&msg);
                            session_log.log_status("错误", &msg);
                            break None;
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    output.info("操作已取消");
                    session_log.log_status("警告", "用户中断了当前操作");
                    break None;
                }
            }
        };

        // Flush remaining messages
        for (level, msg) in output.drain() {
            let label = level.label();
            session_log.log_status(label, &msg);
            self.agent.context.add_display_message(level, &msg);
        }

        // 处理用户中断的情况：回到输入提示，不处理结果
        let result = match result {
            Some(r) => r,
            None => {
                self.agent.context.add_display_message(
                    MessageLevel::Warning,
                    "⏹ 操作已取消",
                );
                return Ok(ReplAction::Continue);
            }
        };

        // Add result to conversation history so it appears at the end
        // of the message list, not just as a status message at the top.
        self.agent.context.add_message(
            crate::agent::context::Role::Assistant,
            result.message.clone(),
            None,
            None,
        );
        session_log.log_assistant(&result.message);

        // Handle restart request
        if result.restart_requested {
            return self.handle_restart(session_log);
        }

        Ok(ReplAction::Continue)
    }

    fn handle_restart(
        &mut self,
        _session_log: &mut SessionLogger,
    ) -> Result<ReplAction, AppError> {
        use std::io::Write;

        let state_path = self.config.working_dir.join(crate::repl::STATE_FILE);
        if let Err(e) = self.agent.context.save_state(&state_path) {
            self.agent.context.add_display_message(
                MessageLevel::Error,
                &format!("保存状态失败: {}。未重启。", e),
            );
            let messages = self.agent.context.get_display_messages();
            ui::render(&messages, &self.agent.context.display.messages, None, self.config.verbose)?;
            return Ok(ReplAction::Quit);
        }

        self.agent.context.add_display_message(
            MessageLevel::Info,
            "正在运行 cargo build...",
        );
        let messages = self.agent.context.get_display_messages();
        ui::render(&messages, &self.agent.context.display.messages, None, self.config.verbose)?;
        std::io::stdout().flush().ok();

        // perform_restart 会在成功时 exec() 替换进程，永远不会返回；
        // 返回 true 表示构建失败、需要继续 REPL。
        let should_continue = crate::restart::perform_restart(
            &self.config.working_dir,
            &self.config.restart_args,
            &mut |level, msg: String| {
                self.agent.context.add_display_message(level, &msg);
            },
        );

        if should_continue {
            let messages = self.agent.context.get_display_messages();
            ui::render(&messages, &self.agent.context.display.messages, None, self.config.verbose)?;
            Ok(ReplAction::Continue)
        } else {
            Ok(ReplAction::Quit)
        }
    }
}

enum ReplAction {
    Continue,
    Quit,
}
