use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process;

use clap::Parser;
use tracing::info;
use tracing_subscriber::fmt;

use crate::agent::{Agent, AgentConfig, AgentStep, ContextManager};
use crate::config::{load_agent_config, load_models};
use crate::llm::LlmClient;
use crate::security::SecurityPolicy;
use crate::session::SessionLogger;
use crate::tools::ToolRegistry;
use crate::ui::{UIMessageOutput, CliMessageOutput};
use crate::utils::message_level::MessageLevel;
use crate::utils::message_output::MessageOutput;
use crate::utils::error::AppError;

mod agent;
mod config;
mod llm;
mod security;
mod session;
mod skills;
mod tools;
mod ui;
mod utils;

const STATE_FILE: &str = ".dev-assistant-state.json";

#[derive(Parser, Debug)]
#[command(name = "dev-assistant", about = "Rust native AI programming assistant")]
struct Cli {
    #[arg(short, long, default_value = ".")]
    project: String,

    #[arg(short = 'M', long)]
    model: Option<String>,

    #[arg(long, default_value = "openai")]
    provider: String,

    #[arg(long)]
    no_approval: bool,

    #[arg(long)]
    max_iterations: Option<usize>,

    #[arg(long, default_value = "4096")]
    max_tokens: usize,

    #[arg(short, long)]
    message: Option<String>,

    #[arg(long)]
    resume: bool,

    #[arg(short, long, default_value = "false")]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
    dotenv::dotenv().ok();

    let cli = Cli::parse();

    // Initialize tracing subscriber — logs go to stderr so they don't
    // interfere with the split-pane UI rendered on stdout.
    let level = if cli.verbose {
        tracing_subscriber::filter::LevelFilter::DEBUG
    } else {
        tracing_subscriber::filter::LevelFilter::WARN
    };
    fmt::Subscriber::builder()
        .with_max_level(level)
        .with_target(false)
        .with_thread_ids(false)
        .with_file(cli.verbose)
        .with_line_number(cli.verbose)
        .with_writer(std::io::stderr)
        .init();

    let working_dir = PathBuf::from(&cli.project);
    if !working_dir.exists() {
        return Err(AppError::Config(format!(
            "Project directory does not exist: {}",
            cli.project
        )));
    }

    // 加载模型配置（TOML 优先，fallback 到环境变量）
    let mut provider_configs = load_models(&working_dir)?;

    // CLI 参数覆盖：--model 和 --provider
    if let Some(ref model) = cli.model {
        if let Some(first) = provider_configs.first_mut() {
            first.model = model.clone();
        }
    }
    if cli.provider != "openai" {
        if let Some(first) = provider_configs.first_mut() {
            first.provider = cli.provider.clone();
        }
    }

    let security = SecurityPolicy::new(&working_dir, !cli.no_approval);
    let tools = ToolRegistry::new(working_dir.clone(), &security);

    let llm_client = LlmClient::from_configs(provider_configs)?;

    // Build system prompt dynamically from registered tool schemas
    let tool_schemas = tools.get_tool_schemas();
    let mut tool_descriptions = String::new();
    for schema in &tool_schemas {
        tool_descriptions.push_str(&format!(
            "- {}: {}\n",
            schema.function.name, schema.function.description
        ));
    }

    // Discover skills from the project's skills/ directory
    let skills_dir = crate::skills::default_skills_dir(&working_dir);
    let discovered_skills = crate::skills::discover_skills(&skills_dir).unwrap_or_default();
    let skills_prompt = crate::skills::format_skills_for_prompt(&discovered_skills);

    if !discovered_skills.is_empty() {
        info!(count = discovered_skills.len(), "Discovered skills");
    }

    let system_prompt = format!(
        r#"你是一个软件工程师助手。使用工具完成用户任务。

可用工具：
{tools}

技能说明（当任务匹配以下技能时，按照技能流程执行，使用上面的工具完成）：
{skills}

规则：
1. 先了解项目结构再进行操作
2. 对危险操作（rm -rf, sudo等）要谨慎
3. 完成任务后**必须**使用 finish 工具结束，提供任务完成总结
4. 用户可以输入 /quit 或 /exit 退出程序
5. restart 工具用于修改代码后自动编译验证。调用后进程会重启并自动恢复对话。**重启后不要再调用 restart 工具**，直接继续执行用户任务。

工具使用建议（以下全是工具名称，可以调用）：
- exec_command: 直接执行程序，command 为可执行文件名，args 为参数列表（如 command=\"cargo\", args=[\"build\"]）。**不支持** shell 语法（管道 |、重定向 >、&&、|| 等），也不支持 sh -c。每个调用只能执行一个命令。
- batch_read_files: 批量读取多个文件（支持 glob 模式，自动生成摘要，适合代码审查等需要读取大量文件的场景）
- restart: 修改源代码后自动运行 cargo build 并重启（仅在 dev-assistant-rs 项目自身上可用），验证修改是否编译通过
- read_file: 读取文件内容（支持 offset/limit 分块读取）
- write_file: 写入新文件（如果文件不存在）
- edit_file: 编辑现有文件（如果文件已存在，需要提供准确的 old_content）
- glob: 查找文件（如果不确定文件路径，先使用 glob）

技能使用说明（技能不是工具，不能直接调用；激活后按照其流程使用工具执行）：
- code-review: 代码审查技能，激活后按照其流程读取文件并输出审查报告
- 其他技能：当任务描述匹配技能触发条件时，自动激活

重要提醒：
- 每次读取文件后，记录你看到了什么
- 读取文件时，**绝不读取 target/、node_modules/、.git/ 等构建/依赖目录中的文件**，这些目录包含二进制产物，不是源代码
- 对于"审查"类任务（如代码审查、文档审查），需要：
  1. 读取关键文件
  2. 总结发现的问题或优点
  3. 使用 finish 工具输出审查报告
- 不要无限循环读取文件，应在读取足够信息后给出结论
- 修改 Rust 源代码后，使用 restart 工具自动编译验证
"#,
        tools = tool_descriptions.trim(),
        skills = skills_prompt.trim()
    );

    // Try to resume from saved state, or create fresh context
    let (context, saved_model) = if cli.resume {
        let state_path = working_dir.join(STATE_FILE);
        info!(path = %state_path.display(), "Attempting to resume from saved state");
        match ContextManager::load_state(&state_path) {
            Ok(mut ctx) => {
                info!("Successfully resumed conversation state");

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
                ctx.display_messages.clear();
                ctx.history_display_start = ctx.history.len();

                // Inject a system-level directive after restart so the LLM
                // knows a restart just occurred and must not call restart again.
                ctx.add_message(
                    crate::agent::context::Role::System,
                    "【系统】注意：程序刚刚通过 restart 工具完成重启并恢复对话。请继续执行用户任务，不要再次调用 restart 工具。".to_string(),
                    None,
                    None,
                );

                let saved_model = ctx.active_model.clone();
                let _ = std::fs::remove_file(&state_path);
                (ctx, saved_model)
            }
            Err(e) => {
                info!(error = %e, "Failed to resume state, starting fresh");
                (ContextManager::new(system_prompt, cli.max_tokens), None)
            }
        }
    } else {
        (ContextManager::new(system_prompt, cli.max_tokens), None)
    };

    let env_config = load_agent_config();
    let max_iterations = cli
        .max_iterations
        .or(Some(env_config.max_iterations))
        .unwrap_or(8);

    let agent_config = AgentConfig { max_iterations };

    let mut agent = Agent::new(context, tools, llm_client, agent_config, discovered_skills);

    // 恢复持久化的活跃模型
    if let Some(ref model_name) = saved_model {
        if let Err(e) = agent.switch_model(model_name) {
            info!(model = %model_name, error = %e, "Failed to restore saved model, using default");
        } else {
            info!(model = %model_name, "Restored saved model");
        }
    }

    if let Some(message) = cli.message {
        // ── Non-interactive mode: use CliMessageOutput ──
        let mut output = CliMessageOutput::new(cli.verbose);
        output.info(&format!("项目目录: {}", working_dir.display()));
        output.info(&format!("模型: {}", agent.active_model()));

        let result = agent.run(message, &mut output).await?;
        if result.success {
            output.success(&result.message);
        } else {
            output.error(&result.message);
        }
    } else {
        // ── Interactive REPL with split-pane UI ──
        println!("🚀 Dev-Assistant Rust CLI");
        println!("Project: {}", working_dir.display());
        println!("Type '/quit' or '/exit' to quit.\n");

        // Create a session log file for debugging
        let mut session_log = SessionLogger::create(&working_dir)?;
        session_log.log_status("信息", &format!("项目目录: {}", working_dir.display()));
        session_log.log_status("信息", &format!("模型: {}", agent.active_model()));

        loop {
            // Render the split-pane UI: messages on top, input at bottom
            let messages = agent.context.get_display_messages();
            crate::ui::render(&messages, &agent.context.display_messages, None, cli.verbose)?;

            let mut input = String::new();
            let bytes_read = std::io::stdin()
                .read_line(&mut input)
                .map_err(AppError::Io)?;

            if bytes_read == 0 {
                // Clear screen before goodbye
                print!("\x1b[2J\x1b[H");
                println!("👋 Goodbye!");
                break;
            }

            let input = input.trim().to_string();

            if input == "/exit" || input == "/quit" {
                print!("\x1b[2J\x1b[H");
                println!("👋 Goodbye!");
                break;
            }

            if input == "/clear" {
                agent.context.display_messages.clear();
                agent.context.history_display_start = agent.context.history.len();
                continue;
            }

            if input.starts_with("/model") {
                let parts: Vec<&str> = input.split_whitespace().collect();
                if parts.len() == 1 {
                    let active = agent.active_model().to_string();
                    let models: Vec<String> = agent.list_models().into_iter().map(|s| s.to_string()).collect();
                    agent.context.add_display_message(
                        crate::utils::message_level::MessageLevel::Info,
                        "可用模型:",
                    );
                    for m in &models {
                        let marker = if m.as_str() == active.as_str() { "→" } else { " " };
                        agent.context.add_display_message(
                            crate::utils::message_level::MessageLevel::Info,
                            &format!("{} {}", marker, m),
                        );
                    }
                } else {
                    let model_name = parts[1];
                    match agent.switch_model(model_name) {
                        Ok(()) => {
                            agent.context.add_display_message(
                                crate::utils::message_level::MessageLevel::Success,
                                &format!("切换到模型: {}", model_name),
                            );
                            agent.context.active_model = Some(model_name.to_string());
                            // 立即保存状态
                            let state_path = working_dir.join(STATE_FILE);
                            let _ = agent.context.save_state(&state_path);
                        }
                        Err(e) => {
                            agent.context.add_display_message(
                                crate::utils::message_level::MessageLevel::Error,
                                &format!("切换失败: {}", e),
                            );
                        }
                    }
                }
                // 重新渲染 UI 显示结果
                let messages = agent.context.get_display_messages();
                crate::ui::render(&messages, &agent.context.display_messages, None, cli.verbose)?;
                continue;
            }

            if input.is_empty() {
                continue;
            }

            // Clear stale display messages from previous turn so they
            // don't accumulate and stack on each render.
            agent.context.display_messages.clear();
            // 记录当前 history 位置，get_display_messages 只显示此后的消息
            agent.context.history_display_start = agent.context.history.len();

            // Clear the screen before agent execution so that any tracing
            // logs (which go to stderr) don't appear inside the split-pane UI.
            print!("\x1b[2J\x1b[H");
            std::io::stdout().flush().map_err(AppError::Io)?;

            // ── Step-by-step agent loop with real-time UI updates ──
            let mut output = UIMessageOutput::new(cli.verbose);
            session_log.log_user(&input);
            agent.start_turn(input, &mut output);

            let result = loop {
                // Drain buffered messages and re-render UI
                for (level, msg) in output.drain() {
                    let label = level.label();
                    session_log.log_status(label, &msg);
                    agent.context.add_display_message(level, &msg);
                }
                let messages = agent.context.get_display_messages();
                crate::ui::render(&messages, &agent.context.display_messages, None, cli.verbose)?;

                // Show "thinking" indicator in the input area so it doesn't
                // get drowned out by subsequent messages in the message panel.
                session_log.log_thinking();
                let messages = agent.context.get_display_messages();
                crate::ui::render(&messages, &agent.context.display_messages, Some("⏳ LLM 正在思考，请稍候..."), cli.verbose)?;

                tokio::select! {
                    step_result = agent.step(&mut output) => {
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
                        // 记录中断日志
                        session_log.log_status("警告", "用户中断了当前操作");
                        break None;
                    }
                }
            };

            // Flush remaining messages
            for (level, msg) in output.drain() {
                let label = level.label();
                session_log.log_status(label, &msg);
                agent.context.add_display_message(level, &msg);
            }

            // 处理用户中断的情况：回到输入提示，不处理结果
            let result = match result {
                Some(r) => r,
                None => {
                    agent.context.add_display_message(
                        MessageLevel::Warning,
                        "⏹ 操作已取消",
                    );
                    continue;
                }
            };

            // Add result to conversation history so it appears at the end
            // of the message list, not just as a status message at the top.
            agent.context.add_message(
                crate::agent::context::Role::Assistant,
                result.message.clone(),
                None,
                None,
            );
            session_log.log_assistant(&result.message);

            // Handle restart request
            if result.restart_requested {
                let state_path = working_dir.join(STATE_FILE);
                if let Err(e) = agent.context.save_state(&state_path) {
                    agent.context.add_display_message(
                        crate::utils::message_level::MessageLevel::Error,
                        &format!("保存状态失败: {}。未重启。", e),
                    );
                    let messages = agent.context.get_display_messages();
                    crate::ui::render(&messages, &agent.context.display_messages, None, cli.verbose)?;
                    break;
                }

                agent.context.add_display_message(
                    crate::utils::message_level::MessageLevel::Info,
                    "正在运行 cargo build...",
                );
                let messages = agent.context.get_display_messages();
                crate::ui::render(&messages, &agent.context.display_messages, None, cli.verbose)?;

                let build_result = process::Command::new("cargo")
                    .arg("build")
                    .current_dir(&working_dir)
                    .status();

                match build_result {
                    Ok(status) if status.success() => {
                        let exe = match std::env::current_exe() {
                            Ok(p) => p,
                            Err(e) => {
                                agent.context.add_display_message(
                                    crate::utils::message_level::MessageLevel::Error,
                                    &format!("获取当前可执行文件路径失败: {}。未重启。", e),
                                );
                                let messages = agent.context.get_display_messages();
                                crate::ui::render(&messages, &agent.context.display_messages, None, cli.verbose)?;
                                // Build succeeded but can't exec — continue REPL
                                break;
                            }
                        };
                        let mut args: Vec<String> = vec!["--resume".to_string()];
                        args.push("--project".to_string());
                        args.push(cli.project.clone());
                        if cli.no_approval {
                            args.push("--no-approval".to_string());
                        }
                        if cli.verbose {
                            args.push("--verbose".to_string());
                        }
                        if let Some(model) = cli.model {
                            args.push("--model".to_string());
                            args.push(model);
                        }
                        args.push("--provider".to_string());
                        args.push(cli.provider.clone());
                        if let Some(max_iter) = cli.max_iterations {
                            args.push("--max-iterations".to_string());
                            args.push(max_iter.to_string());
                        }
                        args.push("--max-tokens".to_string());
                        args.push(cli.max_tokens.to_string());

                        agent.context.add_display_message(
                            crate::utils::message_level::MessageLevel::Success,
                            &format!("构建成功，正在重启 (PID 保持不变)..."),
                        );
                        let messages = agent.context.get_display_messages();
                        crate::ui::render(&messages, &agent.context.display_messages, None, cli.verbose)?;

                        // exec() replaces the current process on success (same PID).
                        // It only returns on error.
                        let exec_err = process::Command::new(&exe)
                            .args(&args)
                            .current_dir(&working_dir)
                            .exec();

                        // If we reach here, exec() failed — show error and exit REPL
                        agent.context.add_display_message(
                            crate::utils::message_level::MessageLevel::Error,
                            &format!("重启失败 (exec 返回错误): {}\n\
                                     可执行文件: {}\n\
                                     参数: {:?}\n\
                                     工作目录: {}\n\
                                     请手动运行: {} {}",
                                     exec_err,
                                     exe.display(),
                                     args,
                                     working_dir.display(),
                                     exe.display(),
                                     args.join(" ")),
                        );
                        let messages = agent.context.get_display_messages();
                        crate::ui::render(&messages, &agent.context.display_messages, None, cli.verbose)?;
                        break;
                    }
                    Ok(status) => {
                        let exit_code = status.code().unwrap_or(-1);
                        agent.context.add_display_message(
                            crate::utils::message_level::MessageLevel::Error,
                            &format!("构建失败，退出码: {}。请修复错误后再次尝试重启。", exit_code),
                        );
                        let messages = agent.context.get_display_messages();
                        crate::ui::render(&messages, &agent.context.display_messages, None, cli.verbose)?;
                        // Build failed — continue REPL so user can fix and retry
                    }
                    Err(e) => {
                        agent.context.add_display_message(
                            crate::utils::message_level::MessageLevel::Error,
                            &format!("运行 cargo build 失败: {}。请修复错误后再次尝试重启。", e),
                        );
                        let messages = agent.context.get_display_messages();
                        crate::ui::render(&messages, &agent.context.display_messages, None, cli.verbose)?;
                        // Build command failed — continue REPL so user can fix and retry
                    }
                }
            }

            // Loop continues — next iteration will render updated messages
        }
    }

    Ok(())
}
