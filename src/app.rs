//! 应用协调层：组装各组件、提供 App 入口。

use std::path::PathBuf;
use std::sync::Arc;

use crate::agent::{Agent, AgentConfig, ContextManager};
use crate::config::{load_agent_config, load_models};
use crate::hooks::HookManager;
use crate::llm::LlmClient;
use crate::orchestrator::{TaskOrchestrator, BackgroundConfig};
use crate::persist::SessionStore;
use crate::prompt::build_system_prompt;
use crate::scheduler::engine::{Scheduler, SchedulerConfig};
use crate::security::SecurityPolicy;
use crate::skills::{discover_all_skills, Skill};
use crate::tools::{async_tool::AsyncToolRegistry, ToolRegistry};
use crate::ui::{self, CliMessageOutput, MarkdownRenderer};
use crate::utils::message_output::MessageOutput;
use crate::utils::error::AppError;

/// 应用配置：由 CLI 参数 + 环境变量组装。
pub struct AppConfig {
    pub working_dir: PathBuf,
    /// 模型配置文件路径（--config 指定，None 时按默认查找顺序）
    pub config: Option<PathBuf>,
    pub verbose: bool,
    pub max_iterations: usize,
    pub max_tokens: usize,
    pub no_approval: bool,
    pub provider: String,
    pub model: Option<String>,
    pub message: Option<String>,
    pub resume: bool,
    /// 是否启用 hook 机制（--no-hooks 时关闭）
    pub hooks_enabled: bool,
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
    /// 定时任务调度器。
    scheduler: Arc<Scheduler>,
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

        // 加载模型配置（--config 指定路径 → 可执行目录 TOML → 环境变量）
        let mut provider_configs = load_models(config.config.as_deref())?;

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
        
        // 创建 Resources 依赖注入容器
        let mut resources = crate::tools::resources::Resources::new();
        
        // 初始化工作目录
        resources.insert(crate::tools::resources::Cwd(config.working_dir.clone()));
        resources.insert(crate::tools::resources::DisplayCwd(config.working_dir.clone()));
        
        // 默认启用 gitignore 过滤
        resources.insert(crate::tools::resources::RespectGitignore(true));
        
        // 初始化 GitignoreFilter
        let gitignore_filter = crate::tools::resources::GitignoreFilter::from_path(&config.working_dir);
        resources.insert(gitignore_filter);
        
        let shared_resources = resources.into_shared();
        
        // 创建共享的 ApprovalManager
        let approval_manager = Arc::new(crate::security::approval::ApprovalManager::new());
        
        let tools = ToolRegistry::new_with_resources(
            config.working_dir.clone(), 
            security.clone(), 
            shared_resources.clone(),
            approval_manager.clone(),
        );
        
        // 初始化全局 TaskManager（供 task_status/pause_task/resume_task/cancel_task 工具使用）
        let task_manager = crate::tools::task_tools::TaskManager::new(crate::orchestrator::DependencyGraph::new());
        crate::tools::task_tools::set_global_task_manager(task_manager);
        
        let llm_client = Arc::new(LlmClient::from_configs(provider_configs)?);

        // 发现全局 + 项目技能
        let discovered_skills = discover_all_skills(&config.working_dir).unwrap_or_default();

        // 构建 system prompt
        let system_prompt = build_system_prompt(&discovered_skills);

        // 初始化 HookManager 并执行 session-start hooks
        let hook_manager = HookManager::load(&config.working_dir, config.hooks_enabled);
        let hook_output = hook_manager.execute_session_start();

        // 创建或恢复 ContextManager
        let mut context = if config.resume {
            Self::load_state_or_fresh(&config, &system_prompt)
        } else {
            ContextManager::new(system_prompt.clone(), config.max_tokens)
        };
        // 将 hook 输出作为独立的 system 消息注入（紧跟在系统提示词之后）
        if !hook_output.is_empty() {
            context.add_message(
                crate::agent::context::Role::System,
                hook_output,
                None,
                None,
            );
        }

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
        
        // 创建异步工具注册表并注册异步文件工具（共享同一个 ApprovalManager）
        let mut async_tools = AsyncToolRegistry::new(
            config.working_dir.clone(), 
            security.clone(),
            approval_manager,
        );
        async_tools.register_tool(Arc::new(crate::tools::file::async_read::AsyncReadFileTool));
        async_tools.register_tool(Arc::new(crate::tools::file::async_read::AsyncBatchReadFilesTool));
        async_tools.register_tool(Arc::new(crate::tools::file::async_write::AsyncWriteFileTool));
        async_tools.register_tool(Arc::new(crate::tools::file::async_write::AsyncEditFileTool));
        
        let mut agent = Agent::new(context, tools, Some(async_tools), llm_client, agent_config, discovered_skills.clone(), session_store);

        // 恢复持久化的活跃模型
        let saved_model = agent.active_model_name().map(String::from);
        if let Some(ref model_name) = saved_model {
            if let Err(e) = agent.switch_model(model_name) {
                tracing::info!(model = %model_name, error = %e, "Failed to restore saved model, using default");
            } else {
                tracing::info!(model = %model_name, "Restored saved model");
            }
        }

        // 创建定时任务调度器
        let scheduler_config = SchedulerConfig {
            store_dir: config.working_dir.join(".dev-assistant").join("scheduler"),
            working_dir: config.working_dir.clone(),
            ..Default::default()
        };
        let scheduler = Arc::new(Scheduler::new(scheduler_config)?);
        // 注册到全局，供 slash 命令和工具处理器访问
        crate::scheduler::tools_handlers::set_global_scheduler(scheduler.clone());

        // 内置 cron：注册每日 dream 记忆整理任务（纯规则模式，零 LLM 成本）。
        // 固定 ID "builtin-dream"，已存在则跳过（防止重启后重复注册）。
        {
            use crate::scheduler::task::{ScheduleType, ScheduledTask, TaskExecutionMode};
            if scheduler.get_task("builtin-dream").unwrap_or(None).is_none() {
                let dream_task = ScheduledTask::new(
                    "builtin-dream".to_string(),
                    "Dream 记忆整理".to_string(),
                    ScheduleType::Cron("0 3 * * *".to_string()),
                    TaskExecutionMode::Agent {
                        instruction: "dream:memory".to_string(),
                    },
                    1,
                    vec!["dream".to_string(), "memory".to_string()],
                    600, // 10 分钟超时
                );
                if let Err(e) = scheduler.schedule_task(&dream_task) {
                    tracing::warn!(error = %e, "注册内置 dream 定时任务失败");
                } else {
                    tracing::info!("已注册内置 dream 定时任务（每日 03:00 记忆整理）");
                }
            }
        }

        Ok(Self {
            agent,
            config,
            skills: discovered_skills,
            system_prompt,
            scheduler,
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
        // 启动定时任务调度器后台 tick 循环，确保 /schedule 和 schedule_task 工具可用
        self.scheduler.start().await;
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
        // 最终结果若已通过流式输出（verbose 下 streaming final）打印过则不再重复输出，
        // 避免同一条助手消息以「信息」和「成功」两种标签出现两次。
        if !output.already_streamed(&result.message) {
            if result.success {
                output.success(&result.message);
            } else {
                output.error(&result.message);
            }
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
                max_tokens: Some(262144),
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
        use crate::repl::{handle_restart, handle_slash, process_user_message, ReplAction};

        let working_dir = self.config.working_dir.clone();
        let restart_args = self.config.restart_args.clone();
        let mut verbose = self.config.verbose;

        // 初始化 UI 和 Markdown 渲染器
        let markdown_renderer = MarkdownRenderer::new();
        // 初始化输入系统（行编辑 + 历史记录）
        let mut input_system = ui::input::InputSystem::new();
        ui::init_ui()?;
        println!("🚀 Dev-Assistant Rust CLI");
        println!("Project: {}", self.config.working_dir.display());
        println!("Type '/exit' or '/quit' to quit.\n");

        let mut round_num: usize = 0;

        loop {
            // T5: 动态提示符 — 紧凑显示模式 / 模型 / 消息数
            let mode = if verbose { "详细" } else { "安静" };
            let model = self.agent.active_model();
            let msg_count = self.agent.display_messages().len();
            let prompt = format!(
                "│ {}│ {} │ {} │ {} 条 > ",
                ui::style::ICON_INFO,
                mode,
                model,
                msg_count
            );

            // 使用 InputSystem 读取输入（支持行编辑、历史记录、Tab 补全）
            let input = match input_system.read_line(&prompt) {
                Ok(Some(input)) => input,
                Ok(None) => {
                    // EOF (Ctrl+D)
                    println!("\n👋 Goodbye!");
                    break;
                }
                Err(e) => {
                    return Err(AppError::Io(std::io::Error::other(
                        format!("输入错误: {}", e),
                    )));
                }
            };

            // ── Slash 命令分发 ──
            if input.starts_with('/') {
                // 优先处理 /pipeline 命令（需要异步上下文）
                if input.starts_with("/pipeline") {
                    let task = input.strip_prefix("/pipeline").unwrap_or("").trim().to_string();
                    if task.is_empty() {
                        let block = ui::MessageBlock::Error {
                            content: "用法: /pipeline <任务描述>\n例如: /pipeline 实现一个缓存系统".to_string(),
                        };
                        ui::render_block(&block, &markdown_renderer)?;
                        continue;
                    }
                    self.agent.add_display_message(
                        crate::utils::message_level::MessageLevel::Info,
                        &format!("🚀 启动流水线: {}", task),
                    );
                    let block = ui::MessageBlock::System {
                        content: format!("🚀 启动流水线: {}", task),
                    };
                    ui::render_block(&block, &markdown_renderer)?;
                    match self.agent.run_pipeline(&task, verbose, false).await {
                        Ok(()) => {}
                        Err(e) => {
                            let block = ui::MessageBlock::Error {
                                content: format!("流水线执行失败: {}", e),
                            };
                            ui::render_block(&block, &markdown_renderer)?;
                        }
                    }
                    continue;
                }

                // 优先处理 /dream 命令（需要异步上下文 + LLM）
                if input.starts_with("/dream") {
                    let args = input.strip_prefix("/dream").unwrap_or("").trim();
                    let dry_run = args.contains("--dry-run")
                        || args.contains("--preview")
                        || args.contains("--dryrun");
                    let block = ui::MessageBlock::System {
                        content: if dry_run {
                            "🧠 Dream 记忆整理（预演模式）开始...".to_string()
                        } else {
                            "🧠 Dream 记忆整理开始...".to_string()
                        },
                    };
                    ui::render_block(&block, &markdown_renderer)?;

                    // LLM 预算：交互命令默认分配 20k tokens（可通过参数覆盖）
                    let budget: usize = args
                        .split_whitespace()
                        .find_map(|a| {
                            a.strip_prefix("--budget=").and_then(|b| b.parse().ok())
                        })
                        .unwrap_or(20_000);
                    let cfg = crate::dream::DreamConfig {
                        working_dir: working_dir.clone(),
                        llm_budget_tokens: if dry_run { 0 } else { budget },
                        dry_run,
                    };
                    match crate::dream::run_dream(&cfg, Some(self.agent.llm_client())).await {
                        Ok(result) => {
                            let block = ui::MessageBlock::System {
                                content: format!("```\n{}\n```", result.summarize(dry_run)),
                            };
                            ui::render_block(&block, &markdown_renderer)?;
                        }
                        Err(e) => {
                            let block = ui::MessageBlock::Error {
                                content: format!("Dream 执行失败: {}", e),
                            };
                            ui::render_block(&block, &markdown_renderer)?;
                        }
                    }
                    continue;
                }

                // 尝试内置 SlashCommand（/help /clear /exit 等）
                if let Some((cmd, args)) = ui::input::SlashCommand::parse(&input) {
                    // /expand 特殊处理：需要调用 ui::get_last_truncated_content
                    if matches!(cmd, ui::input::SlashCommand::Expand) {
                        if let Some(full_content) = ui::get_last_truncated_content() {
                            let block = ui::MessageBlock::System {
                                content: format!("📖 展开完整内容:\n{}", full_content),
                            };
                            ui::render_block(&block, &markdown_renderer)?;
                        } else {
                            let block = ui::MessageBlock::System {
                                content: "ℹ️ 没有找到被折叠的内容".to_string(),
                            };
                            ui::render_block(&block, &markdown_renderer)?;
                        }
                        continue;
                    }

                    // /grep 特殊处理：需要搜索文件内容
                    if matches!(cmd, ui::input::SlashCommand::Grep) {
                        let pattern = args.join(" ");
                        let results = run_grep(&working_dir, &pattern)?;
                        if results.is_empty() {
                            let block = ui::MessageBlock::System {
                                content: format!("🔍 未找到匹配 \"{}\" 的内容", pattern),
                            };
                            ui::render_block(&block, &markdown_renderer)?;
                        } else {
                            let content = format!("🔍 搜索 \"{}\" 找到 {} 个匹配:\n\n{}", pattern, results.len(), results.join("\n"));
                            let block = ui::MessageBlock::System { content };
                            ui::render_block(&block, &markdown_renderer)?;
                        }
                        continue;
                    }

                    // /diff 特殊处理：查看工作区改动（git diff）
                    if matches!(cmd, ui::input::SlashCommand::Diff) {
                        match run_diff(&working_dir, &args) {
                            Ok(Some(diff_content)) => {
                                let file_path = if args.is_empty() {
                                    "工作区".to_string()
                                } else {
                                    args.join(" ")
                                };
                                let block = ui::MessageBlock::Diff {
                                    file_path,
                                    diff_content,
                                    summary: Some("git diff".to_string()),
                                };
                                ui::render_block(&block, &markdown_renderer)?;
                            }
                            Ok(None) => {
                                let block = ui::MessageBlock::System {
                                    content: "ℹ️ 工作区没有未提交的改动".to_string(),
                                };
                                ui::render_block(&block, &markdown_renderer)?;
                            }
                            Err(e) => {
                                let block = ui::MessageBlock::Error {
                                    content: format!("❌ git diff 失败: {}", e),
                                };
                                ui::render_block(&block, &markdown_renderer)?;
                            }
                        }
                        continue;
                    }

                    // /model 特殊处理：查看/切换 LLM 模型（交互式）
                    if matches!(cmd, ui::input::SlashCommand::Model) {
                        // 收集为自有 String，释放对 self.agent 的借用，便于后续可变借用
                        let models: Vec<String> = self
                            .agent
                            .list_models()
                            .iter()
                            .map(|s| s.to_string())
                            .collect();
                        let active = self.agent.active_model().to_string();

                        // 带参数：直接按名称切换
                        if !args.is_empty() {
                            let target = args.join(" ");
                            match self.agent.switch_model(&target) {
                                Ok(_) => {
                                    let block = ui::MessageBlock::System {
                                        content: format!("✅ 已切换到模型: {}", target),
                                    };
                                    ui::render_block(&block, &markdown_renderer)?;
                                }
                                Err(e) => {
                                    let block = ui::MessageBlock::Error {
                                        content: format!("❌ 切换失败: {}", e),
                                    };
                                    ui::render_block(&block, &markdown_renderer)?;
                                }
                            }
                            continue;
                        }

                        if models.is_empty() {
                            let block = ui::MessageBlock::System {
                                content: "ℹ️ 当前没有可用的模型配置".to_string(),
                            };
                            ui::render_block(&block, &markdown_renderer)?;
                            continue;
                        }

                        // 列出所有模型，等待用户输入编号选择
                        let mut lines: Vec<String> = Vec::new();
                        for (i, name) in models.iter().enumerate() {
                            let marker = if *name == active { "👉" } else { "  " };
                            lines.push(format!("  {}. {}{}", i + 1, marker, name));
                        }
                        let content = format!(
                            "📦 可用模型（当前: {}）:\n{}\n\n  输入编号切换（直接回车取消）: ",
                            active,
                            lines.join("\n")
                        );
                        let block = ui::MessageBlock::System { content };
                        ui::render_block(&block, &markdown_renderer)?;

                        // 交互式读取编号
                        match input_system.read_line("  > ") {
                            Ok(Some(choice)) => {
                                let choice = choice.trim();
                                if choice.is_empty() {
                                    let block = ui::MessageBlock::System {
                                        content: "ℹ️ 已取消切换".to_string(),
                                    };
                                    ui::render_block(&block, &markdown_renderer)?;
                                    continue;
                                }
                                match choice.parse::<usize>() {
                                    Ok(idx) if idx >= 1 && idx <= models.len() => {
                                        let target = &models[idx - 1];
                                        match self.agent.switch_model(target) {
                                            Ok(_) => {
                                                let block = ui::MessageBlock::System {
                                                    content: format!("✅ 已切换到模型: {}", target),
                                                };
                                                ui::render_block(&block, &markdown_renderer)?;
                                            }
                                            Err(e) => {
                                                let block = ui::MessageBlock::Error {
                                                    content: format!("❌ 切换失败: {}", e),
                                                };
                                                ui::render_block(&block, &markdown_renderer)?;
                                            }
                                        }
                                    }
                                    _ => {
                                        let block = ui::MessageBlock::Error {
                                            content: format!("❌ 无效编号: {}", choice),
                                        };
                                        ui::render_block(&block, &markdown_renderer)?;
                                    }
                                }
                            }
                            _ => {
                                let block = ui::MessageBlock::System {
                                    content: "ℹ️ 已取消切换".to_string(),
                                };
                                ui::render_block(&block, &markdown_renderer)?;
                            }
                        }
                        continue;
                    }

                    // /history 特殊处理：需要访问 agent 的历史数据
                    if matches!(cmd, ui::input::SlashCommand::History) {
                        let messages = self.agent.history_messages();
                        if messages.is_empty() {
                            let block = ui::MessageBlock::System {
                                content: "📋 暂无对话历史".to_string(),
                            };
                            ui::render_block(&block, &markdown_renderer)?;
                        } else {
                            let mut history_lines: Vec<String> = Vec::new();
                            for (i, msg) in messages.iter().enumerate() {
                                let role_str = match msg.role.as_str() {
                                    "system" => "⚙ 系统",
                                    "user" => "👤 用户",
                                    "assistant" => "🤖 助手",
                                    "tool" => "🔧 工具",
                                    _ => "📝 未知",
                                };
                                let content = msg.content.as_deref().unwrap_or("");
                                let preview: String = content.chars().take(120).collect();
                                if content.chars().count() > 120 {
                                    history_lines.push(format!("  #{} {}: {}...", i + 1, role_str, preview));
                                } else {
                                    history_lines.push(format!("  #{} {}: {}", i + 1, role_str, preview));
                                }
                            }
                            let content = format!(
                                "📋 对话历史（共 {} 条）:\n{}",
                                messages.len(),
                                history_lines.join("\n"),
                            );
                            let block = ui::MessageBlock::System { content };
                            ui::render_block(&block, &markdown_renderer)?;
                        }
                        continue;
                    }

                    match cmd.execute() {
                        ui::input::SlashAction::Exit => {
                            println!("👋 Goodbye!");
                            break;
                        }
                        ui::input::SlashAction::Continue => {
                            // /clear 时同时清除 agent 显示缓冲区
                            if matches!(cmd, ui::input::SlashCommand::Clear) {
                                self.agent.clear_display_to(self.agent.history_len());
                            }
                            continue;
                        }
                        ui::input::SlashAction::ChangeMode(new_verbose) => {
                            verbose = new_verbose;
                            // 显示模式切换消息
                            let msg = if new_verbose { "详细模式" } else { "安静模式" };
                            self.agent.add_display_message(
                                crate::utils::message_level::MessageLevel::Info,
                                &format!("🔊 切换到{}", msg),
                            );
                            continue;
                        }
                    }
                }

                // 委托给现有的应用级 handle_slash（/model /status /background 等）
                if let Some(_outcome) = handle_slash(&input, &mut self.agent, &working_dir) {
                    // 命令已处理并渲染到终端，继续下一轮
                    continue;
                } else {
                    // 未知命令
                    let block = ui::MessageBlock::Error {
                        content: format!("未知命令: {}\n输入 /help 查看可用命令", input),
                    };
                    ui::render_block(&block, &markdown_renderer)?;
                    continue;
                }
            }

            if input.is_empty() {
                continue;
            }

            // ── 轮次分隔 ──
            round_num += 1;
            let round_header = format!(
                "\x1b[1m── 第 {} 轮 ──\x1b[0m",
                round_num
            );
            ui::render_block(
                &ui::MessageBlock::System { content: round_header },
                &markdown_renderer,
            )?;

            // 处理一次用户消息
            let action = process_user_message(
                &mut self.agent,
                &input,
                &working_dir,
                &restart_args,
                verbose,
                &markdown_renderer,
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

        // 会话结束：按需从 SessionStore 的 JSONL 生成可读日志（统一持久化方案，
        // 不再由 SessionLogger 独立并行写入）。
        if let Some(store_path) = self.agent.session_store_path() {
            match crate::session::generate_readable_log(store_path) {
                Ok(log) => {
                    let log_dir = working_dir.join(".dev-assistant-store").join("logs");
                    if let Err(e) = std::fs::create_dir_all(&log_dir) {
                        tracing::warn!(error = %e, "创建会话日志目录失败");
                    } else {
                        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
                        let log_path = log_dir.join(format!(".dev-assistant-session-{}.log", ts));
                        if let Err(e) = std::fs::write(&log_path, log) {
                            tracing::warn!(error = %e, "写入可读会话日志失败");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "生成可读会话日志失败");
                }
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

// ── /grep 命令 ───────────────────────────────────────────────────────

/// 在项目目录中搜索文本，返回带高亮的匹配行列表。
///
/// 每行格式：`文件路径:行号: 匹配内容`（匹配词用黄色反色高亮）
fn run_grep(working_dir: &std::path::Path, pattern: &str) -> Result<Vec<String>, AppError> {
    use std::io::BufRead;

    if pattern.is_empty() {
        return Ok(Vec::new());
    }

    let regex = regex::Regex::new(pattern).map_err(|e| {
        AppError::Config(format!("无效的正则表达式 '{}': {}", pattern, e))
    })?;

    let mut results: Vec<String> = Vec::new();
    let max_results = 50; // 最多显示 50 条结果
    let max_file_size: u64 = 1024 * 1024; // 跳过大于 1MB 的文件

    // 递归遍历 .rs 和 .md 以及 Cargo.toml 等文本文件
    let entries = walkdir::WalkDir::new(working_dir)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            // 跳过 .git, target, node_modules 等目录
            if e.file_type().is_dir() {
                return name != ".git" && name != "target" && name != "node_modules"
                    && name != ".kb" && name != ".mimocode";
            }
            true
        })
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.path().extension().is_some_and(|ext| {
                    matches!(ext.to_str(), Some("rs" | "md" | "toml" | "json" | "yaml" | "yml" | "sh" | "txt"))
                })
        });

    for entry in entries {
        if results.len() >= max_results {
            break;
        }

        let path = entry.path();
        // 跳过过大文件
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() > max_file_size {
                continue;
            }
        }

        // 计算相对路径
        let rel_path = path.strip_prefix(working_dir)
            .unwrap_or(path);

        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = std::io::BufReader::new(file);

        for (line_num, line_result) in reader.lines().enumerate() {
            if results.len() >= max_results {
                break;
            }
            let line = match line_result {
                Ok(l) => l,
                Err(_) => continue,
            };

            if regex.is_match(&line) {
                // 高亮显示匹配词：黄色反色
                let highlighted = regex.replace_all(&line, |caps: &regex::Captures| {
                    format!("\x1b[7;33m{}\x1b[0m", &caps[0])
                });
                results.push(format!(
                    "\x1b[2m{}:{}\x1b[0m: {}",
                    rel_path.display(),
                    line_num + 1,
                    highlighted
                ));
            }
        }
    }

    Ok(results)
}

// ── /diff 命令 ───────────────────────────────────────────────────────

/// 运行 `git diff` 查看工作区改动，返回 unified diff 文本。
///
/// 无参数时查看全部改动；可指定一个或多个路径限制范围。
/// 返回 `Ok(None)` 表示工作区没有未提交的改动。
fn run_diff(working_dir: &std::path::Path, paths: &[String]) -> Result<Option<String>, AppError> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("diff").current_dir(working_dir);
    for p in paths {
        cmd.arg(p);
    }
    let output = cmd.output().map_err(|e| {
        AppError::Io(std::io::Error::new(
            e.kind(),
            format!("git diff 执行失败: {}", e),
        ))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Config(format!(
            "git diff 执行失败: {}",
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = stdout.trim().to_string();
    if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}
