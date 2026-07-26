# Dev-Assistant-RS UI 界面优化方案

## 一、当前 UI 问题分析

### 1.1 现有实现

当前 UI 实现在 `src/ui/mod.rs` 中，采用**全屏清除重绘模式**：

```rust
// 清屏并将光标移到左上角
print!("\x1b[2J\x1b[H");
```

### 1.2 核心问题

| 问题 | 影响 | 严重程度 |
|------|------|----------|
| **全屏闪烁** | 每次消息更新都清屏重绘，视觉体验差 | 高 |
| **丢失滚动历史** | 清屏后无法查看之前的对话内容 | 高 |
| **无 Markdown 渲染** | 代码块、列表等无格式，可读性差 | 高 |
| **无语法高亮** | 代码内容为纯文本，难以阅读 | 高 |
| **无块级布局** | 工具调用、思考过程、结果混在一起 | 中 |
| **输入体验差** | 无行编辑、历史记录、自动补全 | 中 |

---

## 二、grok-build UI 架构参考

### 2.1 核心架构模式

**grok-build** 的 `xai-grok-pager` crate 采用了成熟的终端 UI 架构：

```
┌─────────────────────────────────────────────────────────────┐
│  Title Bar (状态信息)                                        │
├─────────────────────────────────────────────────────────────┤
│  Scrollback Panel (滚动历史)                                 │
│  ├─ Block: User Message                                     │
│  ├─ Block: Thinking (⏳)                                     │
│  ├─ Block: Tool Call                                        │
│  ├─ Block: Tool Result (Code Block with Syntax Highlighting)│
│  └─ Block: Assistant Response (Markdown Rendered)            │
├─────────────────────────────────────────────────────────────┤
│  Input Panel (输入区域)                                      │
│  > type your message...                                      │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 关键组件

| 组件 | 路径 | 功能 |
|------|------|------|
| **Scrollback Block System** | `xai-grok-pager/src/scrollback/blocks/` | 块级消息渲染，支持多种消息类型 |
| **Markdown Renderer** | `xai-grok-markdown/src/` | 完整的 Markdown 解析和渲染，含语法高亮 |
| **Input System** | `xai-grok-pager/src/input/` | 行编辑、键盘归一化、快捷键支持 |
| **Slash Commands** | `xai-grok-pager/src/slash/` | `/help`、`/rewind`、`/history` 等命令 |

### 2.3 Block 类型设计

```
Block (枚举)
├── User { content: String }
├── Assistant { content: String, is_streaming: bool }
├── Thinking { content: String }
├── ToolCall { tool_name: String, args: Value }
├── ToolResult { 
    success: bool, 
    content: String, 
    code_blocks: Vec<CodeBlock> 
}
├── System { content: String }
└── Error { content: String }
```

---

## 三、UI 优化方案

### 3.1 分阶段实施计划

| 阶段 | 时间 | 目标 | 关键特性 |
|------|------|------|----------|
| **Phase 1** | 1-2 天 | 流式输出替代全屏重绘 + 统一渲染接口 | 追加模式、保留滚动历史、定义最终渲染 API |
| **Phase 2** | 2-3 天 | Markdown 渲染与代码高亮 | 代码块、语法高亮、表格 |
| **Phase 3** | 2-3 天 | 块级消息布局 | 工具调用、思考状态、结果展示 |
| **Phase 4** | 3-4 天 | 高级输入系统 | 行编辑、历史记录、Slash 命令 |

> ⚠️ **重要架构决策**：Phase 1 应定义最终的统一渲染接口 `render_blocks()`，接收 `&[MessageBlock]` 切片。后续阶段只增加 `MessageBlock` 变体和渲染能力，**不修改接口签名**。`render_append()` 仅在 Phase 1 临时使用，到 Phase 3 时被 `render_blocks()` 替代。

---

### 3.2 Phase 1: 流式输出（追加模式）

**目标**: 移除全屏清除，改为追加输出，保留终端滚动历史

**修改文件**: `src/ui/mod.rs`

**核心改动**:

```rust
// 移除全屏清除
// print!("\x1b[2J\x1b[H");  // 删除此行

/// 获取当前终端宽度，支持终端 resize。
/// 使用 libc::ioctl + TIOCGWINSZ 动态获取，避免硬编码 80。
fn get_terminal_width() -> usize {
    #[cfg(unix)]
    {
        use libc::{ioctl, STDOUT_FILENO, TIOCGWINSZ};
        let mut winsize: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { ioctl(STDOUT_FILENO, TIOCGWINSZ, &mut winsize) } == 0 {
            return winsize.ws_col as usize;
        }
    }
    std::env::var("COLUMNS").ok().and_then(|s| s.parse().ok()).unwrap_or(80)
}

// 改为追加模式：只更新输入面板，消息追加到底部
pub fn render_append(
    message: Option<&(String, String)>,  // 新消息（可选）
    status_line: Option<&str>,           // 状态栏
) -> io::Result<()> {
    let mut stdout = io::stdout();
    let term_width = get_terminal_width();
    
    // 如果有新消息，追加到输出区域
    if let Some((role, content)) = message {
        let prefix = role_prefix(role);
        let pw = prefix_width(prefix);
        
        // 打印消息分隔线
        writeln!(stdout)?;
        writeln!(stdout, "{}", "─".repeat(term_width))?;
        
        for (i, line) in content.lines().enumerate() {
            if line.is_empty() {
                writeln!(stdout)?;
            } else if i == 0 {
                writeln!(stdout, "{} │ {}", prefix, line)?;
            } else {
                writeln!(stdout, "{:width$} │ {}", "", line, width = pw)?;
            }
        }
    }
    
    // 更新输入面板（清除旧的输入行）
    write!(stdout, "\x1b[1G\x1b[K")?;  // 移动到行首并清除
    match status_line {
        Some(status) => writeln!(stdout, "│ 输入面板 — {}", status)?,
        None => write!(stdout, "│ > ")?,
    }
    
    stdout.flush()?;
    Ok(())
}
```

**新增功能**:
- `render_append()`: 追加模式渲染新消息
- `clear_input_line()`: 清除当前输入行而不清屏
- `get_terminal_width()`: 动态获取终端宽度（支持 resize）
- 保留终端滚动历史

---

### 3.3 Phase 2: Markdown 渲染与代码高亮

**目标**: 支持 Markdown 渲染和代码语法高亮

**新增依赖**: `Cargo.toml`

```toml
[dependencies]
# Markdown 解析
pulldown-cmark = "0.11"
# 语法高亮
syntect = { version = "5.1", features = ["default-fancy"] }
```

> **注意**：不使用 `termcolor` 依赖。所有颜色输出使用标准 ANSI 转义序列，避免额外依赖。`syntect` 的 `default-fancy` feature 已包含语法高亮所需的全部功能。

**新增文件**: `src/ui/markdown.rs`

```rust
use std::sync::LazyLock;
use pulldown_cmark::{Event, Options, Parser, Tag};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};

/// 全局语法集缓存（~150 种语法定义，加载约 30-50ms）。
/// 使用 LazyLock 确保只加载一次，避免每次创建 MarkdownRenderer 时重复加载。
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(|| {
    SyntaxSet::load_defaults_newlines()
});

/// 全局主题集缓存。
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(|| {
    ThemeSet::load_defaults()
});

/// Markdown 渲染器（无状态，所有数据来自全局缓存）。
pub struct MarkdownRenderer;

impl MarkdownRenderer {
    pub fn render(&self, markdown: &str) -> String {
        let mut output = String::new();
        let parser = Parser::new_ext(markdown, Options::all());
        
        let mut code_block_lang = String::new();
        let mut in_code_block = false;
        
        for event in parser {
            match event {
                Event::Start(tag) => {
                    match &tag {
                        Tag::CodeBlock(kind) => {
                            in_code_block = true;
                            code_block_lang = match kind {
                                pulldown_cmark::CodeBlockKind::Fenced(lang) => lang.to_string(),
                                _ => String::new(),
                            };
                        }
                        _ => self.render_start_tag(&mut output, &tag),
                    }
                }
                Event::End(tag) => {
                    if matches!(tag, Tag::CodeBlock(_)) {
                        // Code block content is handled via Event::Text while in_code_block
                        in_code_block = false;
                        code_block_lang.clear();
                    } else {
                        self.render_end_tag(&mut output, &tag);
                    }
                }
                Event::Text(text) => {
                    if in_code_block {
                        self.render_code_block(&mut output, &code_block_lang, &text);
                    } else {
                        output.push_str(&text);
                    }
                }
                Event::Code(code) => self.render_inline_code(&mut output, &code),
                Event::HardBreak => output.push('\n'),
                Event::SoftBreak => output.push('\n'),
                _ => {}
            }
        }
        
        output
    }
    
    /// 渲染围栏代码块（使用 HighlightLines 正确 API，而非已废弃的 highlight()）。
    fn render_code_block(&self, output: &mut String, lang: &str, code: &str) {
        let syntax = SYNTAX_SET
            .find_syntax_by_token(lang)  // 比 find_syntax_by_name 更宽容地匹配
            .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());
        
        let theme = &THEME_SET.themes["base16-ocean.dark"];
        let mut highlighter = HighlightLines::new(syntax, theme);
        
        for line in LinesWithEndings::from(code) {
            if let Ok(ranges) = highlighter.highlight_line(line, &SYNTAX_SET) {
                output.push_str(&as_24_bit_terminal_escaped(&ranges[..], false));
            } else {
                output.push_str(line);
            }
        }
    }
    
    fn render_inline_code(&self, output: &mut String, code: &str) {
        output.push_str("\x1b[38;2;156;189;248m`");  // 蓝色
        output.push_str(code);
        output.push_str("`\x1b[0m");
    }
    
    fn render_start_tag(&self, output: &mut String, tag: &Tag) {
        match tag {
            Tag::Bold => output.push_str("\x1b[1m"),
            Tag::Italic => output.push_str("\x1b[3m"),
            Tag::Link(_, _, _) => output.push_str("\x1b[4;34m"),
            Tag::List(..) => {}  // 列表开始标记，不做特殊处理
            Tag::Item => output.push_str("• "),
            Tag::Paragraph => {}  // 段落标记，不做特殊处理
            Tag::Heading { .. } => output.push_str("\x1b[1;33m"),  // 粗体黄色
            _ => {}
        }
    }
    
    fn render_end_tag(&self, output: &mut String, tag: &Tag) {
        match tag {
            Tag::Bold | Tag::Italic | Tag::Heading { .. } => output.push_str("\x1b[0m"),
            Tag::Link(_, _, _) => output.push_str("\x1b[0m"),
            Tag::Paragraph => output.push('\n'),
            _ => {}
        }
    }
}
```

**修改文件**: `src/ui/mod.rs`

集成 Markdown 渲染到消息输出：

```rust
use crate::ui::markdown::MarkdownRenderer;

pub fn render_message(
    role: &str, 
    content: &str, 
    markdown_renderer: &MarkdownRenderer,
) -> io::Result<()> {
    let mut stdout = io::stdout();
    let prefix = role_prefix(role);
    let pw = prefix_width(prefix);
    
    // 助手消息和工具结果使用 Markdown 渲染
    let rendered_content = if role.starts_with("◂ 助手") || role.starts_with("⚙ 工具") {
        markdown_renderer.render(content)
    } else {
        content.to_string()
    };
    
    for (i, line) in rendered_content.lines().enumerate() {
        if i == 0 {
            writeln!(stdout, "{} │ {}", prefix, line)?;
        } else {
            writeln!(stdout, "{:width$} │ {}", "", line, width = pw)?;
        }
    }
    
    stdout.flush()?;
    Ok(())
}
```

---

### 3.4 Phase 3: 块级消息布局

**目标**: 将消息组织为块，支持工具调用、思考状态、结果展示

**新增文件**: `src/ui/blocks.rs`

```rust
use serde_json::Value;
use crate::ui::markdown::MarkdownRenderer;

/// 消息块类型
#[derive(Debug, Clone)]
pub enum MessageBlock {
    User {
        content: String,
    },
    Assistant {
        content: String,
        is_streaming: bool,
    },
    Thinking {
        content: String,
    },
    ToolCall {
        tool_name: String,
        args: Value,
    },
    ToolResult {
        tool_name: String,
        success: bool,
        content: String,
    },
    System {
        content: String,
    },
    Error {
        content: String,
    },
    /// 分隔线（无前缀，渲染为填满终端宽度的横线）
    Divider,
}

impl MessageBlock {
    /// 获取块的渲染前缀。
    /// 注意：Divider 返回空字符串，渲染时会特殊处理（见 render_blocks 函数）。
    pub fn prefix(&self) -> &'static str {
        match self {
            MessageBlock::User { .. } => "👤 你",
            MessageBlock::Assistant { .. } => "🤖 助手",
            MessageBlock::Thinking { .. } => "💭 思考",
            MessageBlock::ToolCall { .. } => "🔧 调用",
            MessageBlock::ToolResult { success, .. } => {
                if *success { "✅ 结果" } else { "❌ 失败" }
            }
            MessageBlock::System { .. } => "⚙ 系统",
            MessageBlock::Error { .. } => "🔥 错误",
            MessageBlock::Divider => "",
        }
    }
    
    /// 渲染块内容（返回纯文本 + ANSI 转义序列）
    pub fn render(&self, md: &MarkdownRenderer) -> String {
        match self {
            MessageBlock::User { content } => content.clone(),
            MessageBlock::Assistant { content, .. } => md.render(content),
            MessageBlock::Thinking { content } => {
                format!("\x1b[2m{}\x1b[0m", content)  // 灰色（dim）
            }
            MessageBlock::ToolCall { tool_name, args } => {
                format!(
                    "\x1b[38;2;156;189;248m{}\x1b[0m\n{}",
                    tool_name,
                    serde_json::to_string_pretty(args).unwrap_or_default()
                )
            }
            MessageBlock::ToolResult { content, .. } => md.render(content),
            MessageBlock::System { content } => {
                format!("\x1b[2m{}\x1b[0m", content)  // 灰色
            }
            MessageBlock::Error { content } => {
                format!("\x1b[38;2;239;68;68m{}\x1b[0m", content)  // 红色
            }
            MessageBlock::Divider => String::new(),
        }
    }
}

/// 从 (role, content) 元组构造 MessageBlock 的辅助函数。
/// 用于将现有 DisplayBuffer 的消息转换为块类型。
impl From<(&str, &str)> for MessageBlock {
    fn from((role, content): (&str, &str)) -> Self {
        if role.starts_with("▸ 你") || role == "你" || role == "👤 你" {
            MessageBlock::User { content: content.to_string() }
        } else if role.starts_with("◂ 助手") || role == "助手" || role == "🤖 助手" {
            MessageBlock::Assistant { content: content.to_string(), is_streaming: false }
        } else if role.starts_with("▸ 成功") || role == "成功" || role == "✅ 结果" {
            MessageBlock::ToolResult {
                tool_name: String::new(),
                success: true,
                content: content.to_string(),
            }
        } else if role.starts_with("▸ 错误") || role == "错误" || role == "❌ 失败" || role == "🔥 错误" {
            MessageBlock::Error { content: content.to_string() }
        } else if role.starts_with("▸ 警告") || role == "警告" || role == "⚠️ 警告" {
            MessageBlock::System { content: content.to_string() }
        } else {
            MessageBlock::System { content: content.to_string() }
        }
    }
}
```

**修改文件**: `src/ui/mod.rs` — 替换 `render()` 为统一接口：

```rust
use crate::ui::blocks::MessageBlock;
use crate::ui::markdown::MarkdownRenderer;

/// 统一渲染接口：渲染一组消息块 + 状态栏。
///
/// 这是最终的渲染入口，Phase 1 的 `render_append()` 和 `render()` 被此函数替代。
/// 后续阶段（Phase 4 等）只增加 `MessageBlock` 变体，不修改此函数签名。
pub fn render_blocks(
    blocks: &[MessageBlock],
    markdown: &MarkdownRenderer,
    status_line: Option<&str>,
) -> io::Result<()> {
    let mut stdout = io::stdout();
    let term_width = get_terminal_width();

    // ── 标题栏 ──
    writeln!(stdout, "{}", "═".repeat(term_width))?;
    writeln!(stdout, "  Dev-Assistant — 消息窗口")?;
    writeln!(stdout, "{}", "═".repeat(term_width))?;

    for block in blocks {
        match block {
            MessageBlock::Divider => {
                writeln!(stdout, "{}", "─".repeat(term_width))?;
            }
            _ => {
                let prefix = block.prefix();
                let content = block.render(markdown);
                let pw = prefix_width(prefix);
                for (i, line) in content.lines().enumerate() {
                    if line.is_empty() {
                        writeln!(stdout)?;
                    } else if i == 0 {
                        writeln!(stdout, "{} │ {}", prefix, line)?;
                    } else {
                        writeln!(stdout, "{:width$} │ {}", "", line, width = pw)?;
                    }
                }
                writeln!(stdout)?;
            }
        }
    }

    // ── 输入面板 ──
    writeln!(stdout, "{}", "─".repeat(term_width))?;
    match status_line {
        Some(status) => writeln!(stdout, "│ 输入面板 — {}", status)?,
        None => write!(stdout, "│ > ")?,
    }
    stdout.flush()?;
    Ok(())
}
```

---

### 3.5 Phase 4: 高级输入系统

**目标**: 支持行编辑、历史记录、Slash 命令

**新增依赖**: `Cargo.toml`

```toml
[dependencies]
# 行编辑（使用 DefaultEditor 而非 Editor<InputHelper>，避免必须自定义 helper）
rustyline = "12.0"
```

> **注意**：rustyline 12.x 中 `Editor::new()` 需要传入 helper 实现。推荐使用 `DefaultEditor`（无需自定义 helper），或添加 `rustyline-derive` 依赖配合 `#[derive(...)]`。

**新增文件**: `src/ui/input.rs`

```rust
use std::path::PathBuf;

use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

/// 输入系统，封装行编辑和历史记录管理。
pub struct InputSystem {
    rl: DefaultEditor,  // 使用 DefaultEditor（rustyline 12.x 推荐）
    history_file: Option<PathBuf>,
}

impl InputSystem {
    pub fn new() -> Self {
        let mut rl = DefaultEditor::new()
            .expect("Failed to create input editor");
        
        // 加载历史记录
        if let Some(dir) = dirs::data_local_dir() {
            let history_file = dir.join("dev-assistant").join("history.txt");
            let _ = std::fs::create_dir_all(history_file.parent().unwrap());
            let _ = rl.load_history(&history_file);
            Self { rl, history_file: Some(history_file) }
        } else {
            Self { rl, history_file: None }
        }
    }
    
    /// 读取一行输入，包含历史记录管理。
    pub fn read_line(&mut self, prompt: &str) -> Result<String, ReadlineError> {
        let input = self.rl.readline(prompt)?;
        
        if !input.trim().is_empty() {
            self.rl.add_history_entry(input.as_str());
            if let Some(ref path) = self.history_file {
                let _ = self.rl.save_history(path);
            }
        }
        
        Ok(input)
    }
    
    /// 检查是否为 Slash 命令
    pub fn is_slash_command(input: &str) -> bool {
        input.trim().starts_with('/')
    }
    
    /// 解析 Slash 命令
    pub fn parse_slash_command(input: &str) -> Option<(String, Vec<String>)> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }
        
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }
        
        let command = parts[0][1..].to_string();  // 去掉 '/'
        let args = parts[1..].iter().map(|s| s.to_string()).collect();
        
        Some((command, args))
    }
}

/// Slash 命令定义。
///
/// 注意：`execute()` 返回 `SlashAction` 而非直接调用 `std::process::exit(0)`，
/// 这样调用方可以优雅地清理资源再退出。
#[derive(Debug)]
pub enum SlashAction {
    Continue,
    Exit,
    ChangeMode(bool),  // true = verbose, false = quiet
}

/// Slash 命令枚举。
pub enum SlashCommand {
    Help,
    History,
    Clear,
    Exit,
    Verbose,
    Quiet,
}

impl SlashCommand {
    pub fn from_str(command: &str) -> Option<Self> {
        match command.to_lowercase().as_str() {
            "help" => Some(Self::Help),
            "history" => Some(Self::History),
            "clear" => Some(Self::Clear),
            "exit" => Some(Self::Exit),
            "verbose" => Some(Self::Verbose),
            "quiet" => Some(Self::Quiet),
            _ => None,
        }
    }
    
    /// 执行命令，返回 SlashAction 让调用方决定后续行为。
    /// 避免直接调用 std::process::exit(0)，防止跳过析构函数。
    pub fn execute(&self) -> SlashAction {
        match self {
            SlashCommand::Help => {
                Self::show_help();
                SlashAction::Continue
            }
            SlashCommand::History => {
                println!("显示历史记录（待实现）");
                SlashAction::Continue
            }
            SlashCommand::Clear => {
                print!("\x1b[2J\x1b[H");
                SlashAction::Continue
            }
            SlashCommand::Exit => SlashAction::Exit,
            SlashCommand::Verbose => {
                println!("切换到详细模式");
                SlashAction::ChangeMode(true)
            }
            SlashCommand::Quiet => {
                println!("切换到安静模式");
                SlashAction::ChangeMode(false)
            }
        }
    }
    
    fn show_help() {
        println!("可用命令:");
        println!("  /help      - 显示帮助");
        println!("  /history   - 显示对话历史");
        println!("  /clear     - 清屏");
        println!("  /exit      - 退出");
        println!("  /verbose   - 详细模式");
        println!("  /quiet     - 安静模式");
    }
}
```

---

## 四、与现有组件的集成

### 4.1 与 MessageOutput trait 的集成

现有 `MessageOutput` trait 在 `src/utils/message_output.rs` 中定义。新增 `BlockMessageOutput` 实现，将块转换为 `MessageOutput` 事件：

```rust
// src/ui/output_impls.rs 新增
use crate::ui::blocks::MessageBlock;
use crate::utils::message_level::MessageLevel;
use crate::utils::message_output::MessageOutput;

/// 将消息转换为 MessageBlock 的 MessageOutput 实现。
/// 在 REPL 中，Agent 的消息通过此实现收集为块，最终由 render_blocks() 渲染。
pub struct BlockMessageOutput {
    blocks: Vec<MessageBlock>,
    verbose: bool,
}

impl BlockMessageOutput {
    pub fn new(verbose: bool) -> Self {
        Self { blocks: Vec::new(), verbose }
    }
    
    pub fn drain_blocks(&mut self) -> Vec<MessageBlock> {
        std::mem::take(&mut self.blocks)
    }
}

impl MessageOutput for BlockMessageOutput {
    fn emit(&mut self, level: MessageLevel, msg: &str) {
        if !self.verbose && matches!(level, MessageLevel::Debug | MessageLevel::Info) {
            return;
        }
        let block = match level {
            MessageLevel::Info => MessageBlock::System { content: msg.to_string() },
            MessageLevel::Success => MessageBlock::ToolResult {
                tool_name: String::new(),
                success: true,
                content: msg.to_string(),
            },
            MessageLevel::Error => MessageBlock::Error { content: msg.to_string() },
            MessageLevel::Warning => MessageBlock::System { content: msg.to_string() },
            MessageLevel::Debug => MessageBlock::System { content: msg.to_string() },
        };
        // 去重
        if self.blocks.last().map(|b| format!("{:?}", b)) != Some(format!("{:?}", block)) {
            self.blocks.push(block);
        }
    }
}
```

---

## 五、测试策略

### 5.1 渲染与 IO 分离

将渲染逻辑与 IO 操作分离，使渲染函数可测试：

```rust
// 可测试的纯渲染函数（不操作 stdout）
pub fn render_blocks_to_string(
    blocks: &[MessageBlock],
    markdown: &MarkdownRenderer,
    term_width: usize,
) -> String {
    let mut buf = Vec::new();
    // ... 渲染逻辑写入 buf（使用 writeln! 宏）
    String::from_utf8(buf).unwrap()
}

// IO 包装函数（实际写入 stdout）
pub fn render_blocks(
    blocks: &[MessageBlock],
    markdown: &MarkdownRenderer,
    status_line: Option<&str>,
) -> io::Result<()> {
    let term_width = get_terminal_width();
    let output = render_blocks_to_string(blocks, markdown, term_width);
    let mut stdout = io::stdout();
    write!(stdout, "{}", output)?;
    if let Some(status) = status_line { /* ... */ }
    stdout.flush()?;
    Ok(())
}
```

### 5.2 单元测试示例

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::markdown::MarkdownRenderer;

    #[test]
    fn test_render_user_block() {
        let md = MarkdownRenderer;
        let blocks = [MessageBlock::User { content: "hello".into() }];
        let output = render_blocks_to_string(&blocks, &md, 80);
        assert!(output.contains("hello"));
        assert!(output.contains("👤 你"));
    }

    #[test]
    fn test_render_code_block_syntax_highlighting() {
        let md = MarkdownRenderer;
        let code = "fn hello() {\n    println!(\"hi\");\n}";
        let rendered = md.render(&format!("```rust\n{}\n```", code));
        // 验证代码块含有 ANSI 转义序列（语法高亮）
        assert!(rendered.contains("\x1b["), "syntax highlighting should produce ANSI codes");
        assert!(rendered.contains("fn"), "code content should be preserved");
    }

    #[test]
    fn test_render_empty_blocks() {
        let md = MarkdownRenderer;
        let output = render_blocks_to_string(&[], &md, 80);
        assert!(!output.is_empty(), "even empty blocks should produce a UI frame");
    }
}
```

---

## 六、UI 模块重构后的文件结构

```
src/ui/
├── mod.rs                    # 主渲染入口（追加模式 → 统一 render_blocks 接口）
├── blocks.rs                 # 消息块类型定义（MessageBlock 枚举）
├── markdown.rs               # Markdown 渲染器（全局缓存 SyntaxSet/ThemeSet）
├── input.rs                  # 输入系统（行编辑、Slash 命令）
├── style.rs                  # 样式常量（颜色、间距）
├── output_impls.rs           # 消息输出实现（新增 BlockMessageOutput）
└── markdown.rs               # 测试模块（渲染与 IO 分离）
```

---

## 七、实施优先级

| 优先级 | 功能 | 理由 |
|--------|------|------|
| **P0** | 流式输出（Phase 1）+ 统一渲染接口 | 立即解决闪烁和丢失历史问题 |
| **P0** | 代码语法高亮（Phase 2） | 代码阅读体验是核心需求 |
| **P1** | Markdown 渲染（Phase 2） | 提升文本可读性 |
| **P1** | 块级布局（Phase 3） | 结构化展示工具调用和结果 |
| **P2** | 行编辑输入（Phase 4） | 提升输入体验 |
| **P2** | Slash 命令（Phase 4） | 增加交互便捷性 |

---

## 八、预期效果对比

### 当前效果
```
════════════════════════════════════════════════════════════════
  Dev-Assistant — 消息窗口
════════════════════════════════════════════════════════════════
────────────────────────────────────────────────────────────────
│ 输出面板
────────────────────────────────────────────────────────────────
│ 👤 你 │ 帮我写一个 Rust 函数
│
│ 🤖 助手 │ 好的，这是一个示例函数：
│          │
│          │ fn hello() {
│          │     println!("Hello");
│          │ }
│
────────────────────────────────────────────────────────────────
│ 输入面板
│ > 
```

### 优化后效果
```
────────────────────────────────────────────────────────────────
👤 你 │ 帮我写一个 Rust 函数

────────────────────────────────────────────────────────────────
💭 思考 │ 分析用户需求，需要编写一个简单的 Rust 函数示例

────────────────────────────────────────────────────────────────
🤖 助手 │ 好的，这是一个示例函数：

        fn hello(name: &str) -> String {
            format!("Hello, {}!", name)
        }

        fn main() {
            println!("{}", hello("World"));
        }

────────────────────────────────────────────────────────────────
│ > 
```

---

## 九、风险与注意事项

| 风险 | 应对措施 |
|------|----------|
| 终端兼容性 | 使用标准 ANSI 转义序列，避免使用终端特定功能 |
| 性能问题 | 使用 `LazyLock` 全局缓存 `SyntaxSet`/`ThemeSet`，避免每次渲染重新加载 |
| 依赖增加 | 只添加必要的依赖（`pulldown-cmark` + `syntect` + `rustyline`），`termcolor` 不必要 |
| 终端 resize | 使用 `get_terminal_width()` 动态获取宽度，而非硬编码 80 |
| 滚动卡顿 | 长对话时考虑分页加载或历史压缩 |
| Document 标签匹配 | 使用 `find_syntax_by_token()` 替代 `find_syntax_by_name()`，更宽容 |
| 测试覆盖 | 渲染与 IO 分离，使用 `render_blocks_to_string()` 进行快照测试 |

---

## 十、总结

本方案参考 **grok-build** 的 `xai-grok-pager` 架构，提出了分阶段的 UI 优化计划，并修复了原始方案中的多个技术问题：

1. **Phase 1** 解决最核心的闪烁问题，采用流式追加输出，并定义统一渲染接口
2. **Phase 2** 提升内容可读性，使用 `LazyLock` 缓存语法集，API 适配 syntect 5.x
3. **Phase 3** 结构化消息展示，支持 `MessageBlock` 枚举和 `From` trait 转换
4. **Phase 4** 增强交互体验，使用 `DefaultEditor`、`SlashAction` 信号量而非 `exit(0)`

建议从 **Phase 1 + Phase 2** 开始，这两个阶段可以在不改变整体架构的情况下立即提升用户体验。