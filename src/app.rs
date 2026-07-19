//! 应用协调层：组装各组件、提供 App 入口。

use std::path::PathBuf;
use std::sync::Arc;

use crate::agent::{Agent, AgentConfig, ContextManager};
use crate::config::{load_agent_config, load_models};
use crate::llm::LlmClient;
use crate::orchestrator::{TaskOrchestrator, BackgroundConfig};
use crate::persist::SessionStore;
use crate::prompt::build_system_prompt;
use crate::security::SecurityPolicy;
use crate::session::SessionLogger;
use crate::skills::{default_skills_dir, discover_skills, Skill};
use crate::tools::ToolRegistry;
use crate::ui::{self, CliMessageOutput};
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
    /// 后台模式：长时间运行任务
    pub background: bool,
    /// 传给 restart 子进程的 CLI 参数列表（不含 argv[0]）
    pub restart_args: Vec<String>,
}

/// 应用实例：持有 Agent 和配置，提供运行入口。
pub struct App {
    agent: Agent,
    config: AppConfig,
    /// 发现的项目技能。保留为未来 `/skill` slash 命令或动态激活的扩展点。
    #[allow(dead_code)]
    skills: Vec<Skill>,
    /// 构建好的系统提示词。保留为未来运行时刷新 prompt 的扩展点。
    #[allow(dead_code)]
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

        // SECURITY: 使用 `Arc<SecurityPolicy>` 共享所有权，避免 `Box::leak` 内存泄漏。
        let security = Arc::new(SecurityPolicy::new(
            &config.working_dir,
            !config.no_approval,
        ));
        let tools = ToolRegistry::new(config.working_dir.clone(), security);
        let llm_client = Arc::new(LlmClient::from_configs(provider_configs)?);

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
        let session_store = SessionStore::create(&config.working_dir)
            .map_err(|e| {
                tracing::warn!(error = %e, "无法创建会话持久化存储，将跳过持久化");
            })
            .ok();
        let mut agent = Agent::new(context, tools, llm_client, agent_config, discovered_skills.clone(), session_store);

        // 恢复持久化的活跃模型
        let saved_model = agent.active_model_name().map(String::from);
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
                ctx.history.messages.retain(|msg| {
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

    /// 运行应用：根据配置选择交互 REPL、一次性模式或后台模式。
    pub async fn run(&mut self) -> Result<(), AppError> {
        if self.config.background {
            self.run_background_mode().await
        } else if let Some(message) = self.config.message.clone() {
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

    /// 后台模式：执行长时间运行的任务。
    async fn run_background_mode(&mut self) -> Result<(), AppError> {
        let mut output = CliMessageOutput::new(self.config.verbose);
        output.info("启动后台模式...");

        let llm = Arc::new(crate::llm::LlmClient::from_configs(vec![
            crate::llm::ProviderConfig {
                name: "background".to_string(),
                provider: "openai".to_string(),
                api_url: "http://localhost:9999/v1".to_string(),
                api_key: Some("test".to_string()),
                model: "test-model".to_string(),
                temperature: Some(0.7),
                max_tokens: Some(8192),
            }
        ])?);

        let security = Arc::new(SecurityPolicy::new(
            &self.config.working_dir,
            !self.config.no_approval,
        ));
        let tools = ToolRegistry::new(self.config.working_dir.clone(), security);
        let mut orchestrator = TaskOrchestrator::new(
            self.config.working_dir.clone(),
            llm,
            tools,
        );

        let config = BackgroundConfig {
            checkpoint_interval: 5,
            max_concurrent: 4,
            progress_logging: true,
        };

        let result = crate::orchestrator::run_background(&mut orchestrator, config).await?;

        output.info(&format!("后台任务完成: {}", result.summary));
        output.info(&format!("已完成: {}, 失败: {}, 跳过: {}", result.completed, result.failed, result.skipped));

        Ok(())
    }

    /// 交互模式：进入 REPL。
    async fn run_interactive(&mut self) -> Result<(), AppError> {
        use crate::repl::{handle_restart, handle_slash, process_user_message, ReplAction, SlashOutcome};

        let working_dir = self.config.working_dir.clone();
        let restart_args = self.config.restart_args.clone();
        let verbose = self.config.verbose;

        // 先打印欢迎信息和创建会话日志
        println!("🚀 Dev-Assistant Rust CLI");
        println!("Project: {}", self.config.working_dir.display());
        println!("Type '/quit' or '/exit' to quit.\n");

        let mut session_log = SessionLogger::create(&self.config.working_dir)?;
        session_log.log_status("信息", &format!("项目目录: {}", self.config.working_dir.display()));
        session_log.log_status("信息", &format!("模型: {}", self.agent.active_model()));

        loop {
            // Render the split-pane UI: messages on top, input at bottom
            let messages = self.agent.get_display_messages();
            ui::render(&messages, &self.agent.display_messages(), None, verbose)?;

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
                match handle_slash(&input, &mut self.agent, &working_dir) {
                    Some(SlashOutcome::Quit) => break,
                    Some(SlashOutcome::Continue) => {
                        let messages = self.agent.get_display_messages();
                        ui::render(&messages, &self.agent.display_messages(), None, verbose)?;
                        continue;
                    }
                    None => {}
                }
            }

            if input.is_empty() {
                continue;
            }

            // 处理一次用户消息
            let action = process_user_message(
                &mut self.agent,
                &input,
                &mut session_log,
                &working_dir,
                &restart_args,
                verbose,
            )
            .await?;

            // 处理 restart 请求
            if matches!(action, ReplAction::Continue) {
                // 检查是否需要 restart（process_user_message 内部已处理 restart_requested，
                // 这里只处理 restart 失败后继续 REPL 的情况）
            }
            let action = if self.needs_restart_check() {
                handle_restart(&mut self.agent, &working_dir, &restart_args, verbose)?
            } else {
                action
            };

            match action {
                ReplAction::Continue => continue,
                ReplAction::Quit => break,
            }
        }

        Ok(())
    }

    /// 占位：process_user_message 已内联 restart 逻辑，
    /// 此方法保留为未来扩展点（如显式 /restart 命令）。
    fn needs_restart_check(&self) -> bool {
        false
    }
}
