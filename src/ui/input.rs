//! 高级输入系统：行编辑、历史记录、Slash 命令。
//!
//! 提供 `InputSystem` 封装 rustyline 的行编辑能力，
//! 以及 `SlashCommand` / `SlashAction` 用于统一的命令分发。

use std::path::PathBuf;

use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::{DefaultHistory, History};
use rustyline::validate::Validator;
use rustyline::{Editor, Helper, Context};

// ── Slash 命令补全 ────────────────────────────────────────────────────

/// 可补全的 Slash 命令列表（含 SlashCommand 枚举 + REPL 扩展命令）。
const SLASH_COMMANDS: &[&str] = &[
    "/help", "/history", "/clear", "/expand", "/grep", "/search",
    "/exit", "/quit", "/verbose", "/quiet",
    "/model", "/status", "/pipeline", "/background",
    "/schedule", "/unschedule", "/scheduled", "/tasks",
];

/// rustyline Helper：为 Slash 命令提供 Tab 补全。
///
/// 输入以 `/` 开头时，按前缀匹配候选命令；否则不做补全。
#[derive(Clone, Default)]
pub struct SlashHelper {
    /// 已输入的候选命令集合（首次补全时惰性收集，避免每次重建）。
    candidates: std::cell::RefCell<Vec<Pair>>,
}

impl SlashHelper {
    pub fn new() -> Self {
        Self {
            candidates: std::cell::RefCell::new(Vec::new()),
        }
    }
}

impl Completer for SlashHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        // 仅补全命令行首（光标在末尾且输入以 / 开头）
        if !line.starts_with('/') || pos != line.len() {
            return Ok((0, Vec::new()));
        }

        let mut candidates = self.candidates.borrow_mut();
        candidates.clear();
        for cmd in SLASH_COMMANDS {
            if cmd.starts_with(line) {
                candidates.push(Pair {
                    display: cmd.to_string(),
                    replacement: cmd.to_string(),
                });
            }
        }
        Ok((0, candidates.clone()))
    }
}

impl Hinter for SlashHelper {
    type Hint = String;
}

impl Highlighter for SlashHelper {}

impl Validator for SlashHelper {}

impl Helper for SlashHelper {}

// ── InputSystem ────────────────────────────────────────────────────────

/// 输入系统，封装行编辑和历史记录管理。
///
/// 使用带 `SlashHelper` 的 `Editor`（而非 `DefaultEditor`，
/// 后者 helper 泛型固定为 `()`），以启用 Slash 命令 Tab 补全。
pub struct InputSystem {
    rl: Editor<SlashHelper, DefaultHistory>,
    history_file: Option<PathBuf>,
}

impl InputSystem {
    /// 创建新的输入系统，自动加载历史记录。
    pub fn new() -> Self {
        let mut rl = Editor::<SlashHelper, DefaultHistory>::new()
            .expect("Failed to create input editor");

        // 启用 Slash 命令 Tab 补全
        rl.set_helper(Some(SlashHelper::new()));

        // 尝试加载历史记录
        let history_file = if let Some(dir) = dirs::data_local_dir() {
            let path = dir.join("dev-assistant").join("history.txt");
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = rl.load_history(&path);
            Some(path)
        } else {
            None
        };

        Self { rl, history_file }
    }

    /// 读取一行输入，包含历史记录管理。
    ///
    /// 返回 `Ok(Some(input))` 正常输入，
    /// `Ok(None)` 表示 EOF (Ctrl+D)，
    /// `Err(e)` 表示读取错误。
    pub fn read_line(&mut self, prompt: &str) -> Result<Option<String>, ReadlineError> {
        let input = self.rl.readline(prompt)?;

        let trimmed = input.trim().to_string();
        if !trimmed.is_empty() {
            let _ = self.rl.add_history_entry(trimmed.as_str());
            if let Some(ref path) = self.history_file {
                let _ = self.rl.save_history(path);
            }
        }

        Ok(Some(trimmed))
    }

    /// 获取历史条目数。
    #[allow(dead_code)]
    pub fn history_len(&self) -> usize {
        self.rl.history().len()
    }
}

// ── SlashAction ────────────────────────────────────────────────────────

/// Slash 命令执行结果，让调用方决定后续行为。
///
/// 避免直接调用 `std::process::exit(0)`，防止跳过析构函数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashAction {
    /// 命令已处理，继续下一轮读取
    Continue,
    /// 退出程序
    Exit,
    /// 切换模式（true = verbose, false = quiet）
    ChangeMode(bool),
}

// ── Built-in SlashCommand ──────────────────────────────────────────────

/// 内置 Slash 命令枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    /// /help — 显示帮助
    Help,
    /// /history — 显示对话历史
    History,
    /// /clear — 清屏
    Clear,
    /// /exit — 退出
    Exit,
    /// /verbose — 详细模式
    Verbose,
    /// /quiet — 安静模式
    Quiet,
    /// /expand — 展开上一条被折叠的内容
    Expand,
    /// /grep — 搜索文件内容（支持正则）
    Grep,
    /// /model — 查看/切换 LLM 模型
    Model,
}

impl SlashCommand {
    /// 从字符串解析命令（不包含 '/' 前缀）。
    pub fn from_str(command: &str) -> Option<Self> {
        match command.to_lowercase().as_str() {
            "help" => Some(Self::Help),
            "history" => Some(Self::History),
            "clear" => Some(Self::Clear),
            "exit" | "quit" => Some(Self::Exit),
            "verbose" => Some(Self::Verbose),
            "quiet" => Some(Self::Quiet),
            "expand" => Some(Self::Expand),
            "grep" | "search" => Some(Self::Grep),
            "model" => Some(Self::Model),
            _ => None,
        }
    }

    /// 解析完整输入行，返回 (SlashCommand, 剩余参数)。
    ///
    /// 输入必须以 '/' 开头，否则返回 `None`。
    pub fn parse(input: &str) -> Option<(Self, Vec<String>)> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        let parts: Vec<&str> = trimmed.splitn(2, |c: char| c.is_whitespace()).collect();
        let cmd_name = &parts[0][1..]; // 去掉 '/'
        let args: Vec<String> = parts
            .get(1)
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        Self::from_str(cmd_name).map(|cmd| (cmd, args))
    }

    /// 执行命令，返回 `SlashAction`。
    pub fn execute(&self) -> SlashAction {
        match self {
            SlashCommand::Help => {
                Self::show_help();
                SlashAction::Continue
            }
            SlashCommand::History => {
                println!("📋 历史记录（通过 rustyline 管理，支持上下键浏览）");
                SlashAction::Continue
            }
            SlashCommand::Clear => {
                // 清屏并将光标移到左上角
                print!("\x1b[2J\x1b[H");
                SlashAction::Continue
            }
            SlashCommand::Exit => SlashAction::Exit,
            SlashCommand::Verbose => {
                println!("🔊 切换到详细模式");
                SlashAction::ChangeMode(true)
            }
            SlashCommand::Quiet => {
                println!("🔇 切换到安静模式");
                SlashAction::ChangeMode(false)
            }
            SlashCommand::Expand => {
                // /expand 由调用方在 REPL 循环中处理（需要访问 ui::get_last_truncated_content）
                // execute() 返回 Continue 并打印占位提示
                println!("📖 /expand 命令正在展开上一段被折叠的内容...");
                SlashAction::Continue
            }
            SlashCommand::Grep => {
                // /grep 由调用方在 REPL 循环中处理（需要访问文件系统和工作目录）
                println!("🔍 /grep 命令正在搜索文件内容...");
                SlashAction::Continue
            }
            SlashCommand::Model => {
                // /model 由调用方在 REPL 循环中处理（需要访问 LLM 客户端）
                println!("📦 /model 命令正在切换模型...");
                SlashAction::Continue
            }
        }
    }

    fn show_help() {
        println!("可用命令:");
        println!("  /help      - 显示本帮助");
        println!("  /history   - 显示对话历史");
        println!("  /clear     - 清屏");
        println!("  /expand    - 展开上一段被折叠的内容");
        println!("  /grep      - 搜索文件内容（支持正则，用法: /grep <模式> [路径]）");
        println!("  /search    - 同 /grep");
        println!("  /exit      - 退出程序");
        println!("  /quit      - 退出程序（同 /exit）");
        println!("  /verbose   - 切换到详细模式（显示所有消息）");
        println!("  /quiet     - 切换到安静模式（仅显示关键消息）");
        println!("  /model     - 查看/切换 LLM 模型");
        println!("  /status    - 查看当前状态");
        println!("  /pipeline  - 执行流水线任务");
        println!("  /background - 后台任务管理");
        println!("  /schedule  - 创建定时任务（用法: /schedule cron <表达式> agent <指令>）");
        println!("  /unschedule - 取消定时任务（用法: /unschedule <任务ID>）");
        println!("  /scheduled - 查看所有定时任务（同 /tasks）");
        println!("  /tasks     - 查看所有定时任务（同 /scheduled）");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_slash_command() {
        let (cmd, args) = SlashCommand::parse("/help").unwrap();
        assert_eq!(cmd, SlashCommand::Help);
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_slash_command_with_unknown_command() {
        // "unknown" is not a built-in command, so parse returns None
        let result = SlashCommand::parse("/unknowncmd arg");
        assert!(result.is_none(), "unknown is not a built-in command");
    }

    #[test]
    fn test_parse_model_command() {
        // /model 是内置命令，应解析为 Model 变体并携带参数
        let result = SlashCommand::parse("/model gpt-4");
        let (cmd, args) = result.expect("model should be a built-in command");
        assert_eq!(cmd, SlashCommand::Model);
        assert_eq!(args, vec!["gpt-4".to_string()]);

        // 无参数也应解析成功
        let (cmd, args) = SlashCommand::parse("/model").unwrap();
        assert_eq!(cmd, SlashCommand::Model);
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_with_args() {
        let (cmd, args) = SlashCommand::parse("/verbose extra arg").unwrap();
        assert_eq!(cmd, SlashCommand::Verbose);
        assert_eq!(args, vec!["extra", "arg"]);
    }

    #[test]
    fn test_parse_exit_quit() {
        let (cmd, _) = SlashCommand::parse("/exit").unwrap();
        assert_eq!(cmd, SlashCommand::Exit);
        let (cmd, _) = SlashCommand::parse("/quit").unwrap();
        assert_eq!(cmd, SlashCommand::Exit);
    }

    #[test]
    fn test_parse_non_slash() {
        assert!(SlashCommand::parse("hello").is_none());
        assert!(SlashCommand::parse("").is_none());
    }

    #[test]
    fn test_from_str() {
        assert_eq!(SlashCommand::from_str("help"), Some(SlashCommand::Help));
        assert_eq!(SlashCommand::from_str("exit"), Some(SlashCommand::Exit));
        assert_eq!(SlashCommand::from_str("quit"), Some(SlashCommand::Exit));
        assert_eq!(SlashCommand::from_str("unknown"), None);
    }

    #[test]
    fn test_execute_help_does_not_panic() {
        SlashCommand::Help.execute();
    }

    #[test]
    fn test_execute_exit() {
        assert_eq!(SlashCommand::Exit.execute(), SlashAction::Exit);
    }

    #[test]
    fn test_execute_clear() {
        assert_eq!(SlashCommand::Clear.execute(), SlashAction::Continue);
    }

    #[test]
    fn test_expand_parsed() {
        let (cmd, _) = SlashCommand::parse("/expand").unwrap();
        assert_eq!(cmd, SlashCommand::Expand);
    }

    #[test]
    fn test_expand_from_str() {
        assert_eq!(SlashCommand::from_str("expand"), Some(SlashCommand::Expand));
    }

    #[test]
    fn test_expand_execute_returns_continue() {
        assert_eq!(SlashCommand::Expand.execute(), SlashAction::Continue);
    }

    #[test]
    fn test_grep_parsed() {
        let (cmd, args) = SlashCommand::parse("/grep fn main").unwrap();
        assert_eq!(cmd, SlashCommand::Grep);
        assert_eq!(args, vec!["fn", "main"]);
    }

    #[test]
    fn test_search_parsed() {
        let (cmd, args) = SlashCommand::parse("/search pattern").unwrap();
        assert_eq!(cmd, SlashCommand::Grep);
        assert_eq!(args, vec!["pattern"]);
    }

    #[test]
    fn test_grep_from_str() {
        assert_eq!(SlashCommand::from_str("grep"), Some(SlashCommand::Grep));
        assert_eq!(SlashCommand::from_str("search"), Some(SlashCommand::Grep));
    }

    #[test]
    fn test_grep_execute_returns_continue() {
        assert_eq!(SlashCommand::Grep.execute(), SlashAction::Continue);
    }
}
