//! Web 界面模块（Phase 1）。
//!
//! 提供基于 axum 的 Web 服务，通过 HTMX + Alpine.js 实现对话界面。
//! 复用 100% 核心层（Agent、LlmClient、ToolRegistry 等）。
//!
//! # 启动方式
//!
//! ```bash
//! dev-assistant --web --project ./my-project
//! ```
//!
//! # 模块结构
//!
//! - `mod.rs` — 模块根，`serve()` 函数启动 axum 服务
//! - `router.rs` — axum Router 定义、中间件链
//! - `handlers/` — REST API 和 WebSocket 处理器
//! - `templates/` — minijinja 模板
//! - `static/` — 嵌入的静态资源
//! - `ws/` — WebSocket 事件类型和会话管理

pub mod handlers;
pub mod router;
pub mod ws;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::TcpListener;
use tracing::info;

use crate::agent::AgentConfig;
use crate::config::{load_agent_config, load_models};
use crate::llm::LlmClient;
use crate::prompt::build_system_prompt;
use crate::security::SecurityPolicy;
use crate::skills::{default_skills_dir, discover_skills};
use crate::tools::{async_tool::AsyncToolRegistry, resources::Resources, ToolRegistry};
use crate::utils::error::AppError;

// ---------------------------------------------------------------------------
// 共享状态
// ---------------------------------------------------------------------------

/// 应用共享状态，通过 axum State 提取器注入所有 handler。
pub struct AppState {
    /// 共享的 LLM 客户端
    pub llm: Arc<LlmClient>,
    /// 共享的工具注册表
    #[allow(dead_code)]
    pub tools: ToolRegistry,
    /// 共享的异步工具注册表
    #[allow(dead_code)]
    pub async_tools: Option<AsyncToolRegistry>,
    /// Agent 配置
    pub agent_config: AgentConfig,
    /// 工作目录
    pub working_dir: PathBuf,
    /// 系统提示词
    pub system_prompt: String,
    /// 最大 token 数
    pub max_tokens: usize,
    /// 是否启用详细日志
    #[allow(dead_code)]
    pub verbose: bool,
    /// minijinja 模板引擎环境
    pub templates: minijinja::Environment<'static>,
}

/// Web 服务配置。
pub struct WebConfig {
    /// 绑定的主机地址（默认 127.0.0.1）
    pub host: String,
    /// 绑定的端口（默认 8080）
    pub port: u16,
    /// 工作目录
    pub working_dir: PathBuf,
    /// 是否启用详细日志
    pub verbose: bool,
    /// 最大迭代次数
    pub max_iterations: usize,
    /// 最大 token 数
    pub max_tokens: usize,
    /// provider 名称
    pub provider: String,
    /// 模型名称
    pub model: Option<String>,
    /// 是否关闭审批
    pub no_approval: bool,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8080,
            working_dir: PathBuf::from("."),
            verbose: false,
            max_iterations: 8,
            max_tokens: 8192,
            provider: "openai".to_string(),
            model: None,
            no_approval: false,
        }
    }
}

// ---------------------------------------------------------------------------
// 服务启动
// ---------------------------------------------------------------------------

/// 启动 axum Web 服务。
///
/// 此函数：
/// 1. 加载模型配置和 LLM 客户端
/// 2. 创建工具注册表和安全策略
/// 3. 构建系统提示词
/// 4. 构建 axum Router
/// 5. 绑定并启动 HTTP 服务
pub async fn serve(config: WebConfig) -> Result<(), AppError> {
    if !config.working_dir.exists() {
        return Err(AppError::Config(format!(
            "Project directory does not exist: {}",
            config.working_dir.display()
        )));
    }

    // ── 加载模型配置 ──
    let mut provider_configs = load_models(&config.working_dir)?;
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

    let llm = Arc::new(LlmClient::from_configs(provider_configs)?);

    // ── 安全策略与工具注册表 ──
    let security = Arc::new(SecurityPolicy::new(
        &config.working_dir,
        !config.no_approval,
    ));

    let mut resources = Resources::new();
    resources.insert(crate::tools::resources::Cwd(config.working_dir.clone()));
    resources.insert(crate::tools::resources::DisplayCwd(config.working_dir.clone()));
    resources.insert(crate::tools::resources::RespectGitignore(true));
    let gitignore_filter = crate::tools::resources::GitignoreFilter::from_path(&config.working_dir);
    resources.insert(gitignore_filter);
    let shared_resources = resources.into_shared();

    let approval_manager = Arc::new(crate::security::approval::ApprovalManager::new());
    let tools = ToolRegistry::new_with_resources(
        config.working_dir.clone(),
        security.clone(),
        shared_resources.clone(),
        approval_manager.clone(),
    );

    // ── 异步工具注册表 ──
    let mut async_tools = AsyncToolRegistry::new(
        config.working_dir.clone(),
        security.clone(),
        approval_manager.clone(),
    );
    async_tools.register_tool(Arc::new(crate::tools::file::async_read::AsyncReadFileTool));
    async_tools.register_tool(Arc::new(crate::tools::file::async_read::AsyncBatchReadFilesTool));
    async_tools.register_tool(Arc::new(crate::tools::file::async_write::AsyncWriteFileTool));
    async_tools.register_tool(Arc::new(crate::tools::file::async_write::AsyncEditFileTool));

    // ── 系统提示词 ──
    let tool_schemas = tools.get_tool_schemas();
    let skills_dir = default_skills_dir(&config.working_dir);
    let discovered_skills = discover_skills(&skills_dir).unwrap_or_default();
    let system_prompt = build_system_prompt(&tool_schemas, &discovered_skills);

    // ── Agent 配置 ──
    let env_config = load_agent_config();
    let max_iterations = if config.max_iterations > 0 {
        config.max_iterations
    } else {
        env_config.max_iterations
    };
    let agent_config = AgentConfig { max_iterations };

    // ── 模板引擎 ──
    let mut templates = minijinja::Environment::new();
    // 注册模板：从编译时嵌入的字符串加载
    templates
        .add_template("base.html", include_str!("templates/base.html"))
        .map_err(|e| AppError::Config(format!("模板 base.html 加载失败: {}", e)))?;
    templates
        .add_template("index.html", include_str!("templates/index.html"))
        .map_err(|e| AppError::Config(format!("模板 index.html 加载失败: {}", e)))?;
    templates
        .add_template("files.html", include_str!("templates/files.html"))
        .map_err(|e| AppError::Config(format!("模板 files.html 加载失败: {}", e)))?;

    // ── 共享状态 ──
    let state = AppState {
        llm,
        tools,
        async_tools: Some(async_tools),
        agent_config,
        working_dir: config.working_dir.clone(),
        system_prompt,
        max_tokens: config.max_tokens,
        verbose: config.verbose,
        templates,
    };

    // ── 构建 Router ──
    let app = router::build_router(state);

    // ── 启动服务 ──
    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| AppError::Config(format!("Invalid host:port: {}", e)))?;

    info!("🚀 Web UI 服务启动: http://{}/", addr);
    info!("按 Ctrl+C 停止服务");

    let listener = TcpListener::bind(addr)
        .await
        .map_err(AppError::Io)?;

    axum::serve(listener, app)
        .await
        .map_err(AppError::Io)?;

    Ok(())
}