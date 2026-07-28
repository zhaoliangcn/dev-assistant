//! Dev-Assistant Rust CLI 入口。
//!
//! 职责仅限于：
//! 1. 加载 `.env`
//! 2. 解析 CLI 参数
//! 3. 初始化 tracing
//! 4. 组装 [`AppConfig`] 并交给 [`crate::app::App`] 运行
//!
//! 业务逻辑（REPL 循环、slash 命令、restart 流程、prompt 构建）分别位于
//! `app.rs`、`repl.rs`、`restart.rs`、`prompt.rs`。



mod agent;
mod app;
mod config;
mod llm;
mod orchestrator;
mod persist;
mod prompt;
mod repl;
mod restart;
mod security;
mod session;
mod skills;
mod tools;
mod ui;
mod utils;

use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::fmt;

use crate::app::{App, AppConfig};
use crate::utils::error::AppError;

#[derive(Parser, Debug)]
#[command(name = "dev-assistant", version, about = "Rust native AI programming assistant")]
struct Cli {
    /// 一次性执行模式：传入消息后立即退出
    #[arg(long)]
    message: Option<String>,

    /// 项目目录（默认当前目录）
    #[arg(long, default_value = ".")]
    project: String,

    /// 覆盖默认 provider（openai / anthropic / ollama 等）
    #[arg(long, default_value = "openai")]
    provider: String,

    /// 覆盖默认模型名
    #[arg(long)]
    model: Option<String>,

    /// 关闭交互式审批（高危操作直接执行）
    #[arg(long)]
    no_approval: bool,

    /// 启用详细日志输出
    #[arg(long)]
    verbose: bool,

    /// 单次任务最大 Agent 迭代次数
    #[arg(long)]
    max_iterations: Option<usize>,

    /// 上下文窗口的最大 token 数
    #[arg(long, default_value_t = 8192)]
    max_tokens: usize,

    /// 从上次保存的状态恢复对话（restart 后由子进程传入）
    #[arg(long)]
    resume: bool,

    /// 后台模式：执行长时间运行的任务
    #[arg(long)]
    background: bool,
}

impl Cli {
    /// 构建重启时传给子进程的 CLI 参数列表（不含 argv[0]）。
    ///
    /// 集中在此处定义，避免与 `AppConfig` 字段重复。
    fn to_restart_args(&self) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "--resume".to_string(),
            "--project".to_string(),
            self.project.clone(),
        ];
        if self.no_approval {
            args.push("--no-approval".to_string());
        }
        if self.verbose {
            args.push("--verbose".to_string());
        }
        if let Some(ref model) = self.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }
        args.push("--provider".to_string());
        args.push(self.provider.clone());
        if let Some(max_iter) = self.max_iterations {
            args.push("--max-iterations".to_string());
            args.push(max_iter.to_string());
        }
        args.push("--max-tokens".to_string());
        args.push(self.max_tokens.to_string());
        args
    }
}

fn main() -> Result<(), AppError> {
    // Load .env if present
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

    // 重启时传给子进程的 CLI 参数（保持当前会话的所有配置）
    // 使用 Cli::to_restart_args() 方法集中构建，避免与 AppConfig 字段重复
    let restart_args = cli.to_restart_args();

    let config = AppConfig {
        working_dir,
        verbose: cli.verbose,
        max_iterations: cli.max_iterations.unwrap_or(8),
        max_tokens: cli.max_tokens,
        no_approval: cli.no_approval,
        provider: cli.provider,
        model: cli.model,
        message: cli.message,
        resume: cli.resume,
        background: cli.background,
        restart_args,
    };

    // App::build 是同步的，但 App::run 需要 tokio runtime。
    // 直接用 `tokio::main` 也可，但为了避免 `main` 函数签名膨胀，
    // 这里手动构建 runtime。
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Config(format!("Failed to build tokio runtime: {}", e)))?;

    runtime.block_on(async {
        let mut app = App::build(config)?;
        app.run().await
    })
}
