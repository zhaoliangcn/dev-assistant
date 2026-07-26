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
| **Phase 1** | 1-2 天 | 流式输出替代全屏重绘 | 追加模式、保留滚动历史 |
| **Phase 2** | 2-3 天 | Markdown 渲染与代码高亮 | 代码块、语法高亮、表格 |
| **Phase 3** | 2-3 天 | 块级消息布局 | 工具调用、思考状态、结果展示 |
| **Phase 4** | 3-4 天 | 高级输入系统 | 行编辑、历史记录、Slash 命令 |

---

### 3.2 Phase 1: 流式输出（追加模式）

**目标**: 移除全屏清除，改为追加输出，保留终端滚动历史

**修改文件**: `src/ui/mod.rs`

**核心改动**:

```rust
// 移除全屏清除
// print!("\x1b[2J\x1b[H");  // 删除此行

// 改为追加模式：只更新输入面板，消息追加到底部
pub fn render_append(
    message: Option<&(String, String)>,  // 新消息（可选）
    status_line: Option<&str>,           // 状态栏
) -> io::Result<()> {
    let mut stdout = io::stdout();
    
    // 如果有新消息，追加到输出区域
    if let Some((role, content)) = message {
        let prefix = role_prefix(role);
        let pw = prefix_width(prefix);
        
        // 打印消息分隔线
        writeln!(stdout)?;
        writeln!(stdout, "{}", "─".repeat(80))?;
        
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
syntect = { version = "5.1", features = ["parsing", "html", "dump-create"] }
# 终端颜色
termcolor = "1.4"
```

**新增文件**: `src/ui/markdown.rs`

```rust
use pulldown_cmark::{Event, Options, Parser};
use syntect::highlighting::{Style, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};

pub struct MarkdownRenderer {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
}

impl MarkdownRenderer {
    pub fn new() -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
        }
    }
    
    pub fn render(&self, markdown: &str) -> String {
        let mut output = String::new();
        let parser = Parser::new_ext(markdown, Options::all());
        
        for event in parser {
            match event {
                Event::Start(tag) => self.render_start_tag(&mut output, &tag),
                Event::End(tag) => self.render_end_tag(&mut output, &tag),
                Event::Text(text) => output.push_str(&text),
                Event::Code(code) => self.render_inline_code(&mut output, &code),
                Event::FencedCodeBlock(info, code) => {
                    self.render_code_block(&mut output, &info, &code)
                }
                Event::Heading(_, _, _) => output.push('\n'),
                _ => {}
            }
        }
        
        output
    }
    
    fn render_code_block(&self, output: &mut String, lang: &str, code: &str) {
        let syntax = self.syntax_set.find_syntax_by_name(lang)
            .or_else(|| self.syntax_set.find_syntax_by_extension(lang))
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());
        
        let theme = &self.theme_set.themes["base16-ocean.dark"];
        
        for line in LinesWithEndings::from(code) {
            let ranges = syntect::highlighting::highlight(line, syntax, theme);
            output.push_str(&as_24_bit_terminal_escaped(&ranges[..], false));
        }
    }
    
    fn render_inline_code(&self, output: &mut String, code: &str) {
        output.push_str("\x1b[38;2;156;189;248m`");  // 蓝色
        output.push_str(code);
        output.push_str("`\x1b[0m");
    }
    
    fn render_start_tag(&self, output: &mut String, tag: &pulldown_cmark::Tag) {
        match tag {
            pulldown_cmark::Tag::Bold => output.push_str("\x1b[1m"),
            pulldown_cmark::Tag::Italic => output.push_str("\x1b[3m"),
            pulldown_cmark::Tag::Link(_, _, _) => output.push_str("\x1b[4;34m"),
            pulldown_cmark::Tag::ListItem => output.push_str("• "),
            _ => {}
        }
    }
    
    fn render_end_tag(&self, output: &mut String, tag: &pulldown_cmark::Tag) {
        match tag {
            pulldown_cmark::Tag::Bold | pulldown_cmark::Tag::Italic => {
                output.push_str("\x1b[0m")
            }
            pulldown_cmark::Tag::Link(_, _, _) => output.push_str("\x1b[0m"),
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
    
    // 助手消息使用 Markdown 渲染
    let rendered_content = if role.starts_with("◂ 助手") {
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
    Divider,
}

impl MessageBlock {
    /// 获取块的渲染前缀
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
            MessageBlock::Divider => "────────────────────────────────────────",
        }
    }
    
    /// 渲染块内容
    pub fn render(&self, markdown_renderer: &crate::ui::markdown::MarkdownRenderer) -> String {
        match self {
            MessageBlock::User { content } => content.clone(),
            MessageBlock::Assistant { content, .. } => {
                markdown_renderer.render(content)
            }
            MessageBlock::Thinking { content } => {
                format!("\x1b[2m{}\x1b[0m", content)  // 灰色
            }
            MessageBlock::ToolCall { tool_name, args } => {
                format!(
                    "\x1b[38;2;156;189;248m{}\x1b[0m\n{}",
                    tool_name,
                    serde_json::to_string_pretty(args).unwrap_or_default()
                )
            }
            MessageBlock::ToolResult { content, .. } => {
                markdown_renderer.render(content)
            }
            MessageBlock::System { content } => {
                format!("\x1b[2m{}\x1b[0m", content)
            }
            MessageBlock::Error { content } => {
                format!("\x1b[38;2;239;68;68m{}\x1b[0m", content)  // 红色
            }
            MessageBlock::Divider => String::new(),
        }
    }
}
```

**修改文件**: `src/ui/mod.rs`

```rust
use crate::ui::blocks::MessageBlock;

pub fn render_block(
    block: &MessageBlock,
    markdown_renderer: &MarkdownRenderer,
) -> io::Result<()> {
    let mut stdout = io::stdout();
    let prefix = block.prefix();
    
    if matches!(block, MessageBlock::Divider) {
        writeln!(stdout, "{}", prefix)?;
        return Ok(());
    }
    
    let content = block.render(markdown_renderer);
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
# 行编辑
rustyline = "12.0"
```

**新增文件**: `src/ui/input.rs`

```rust
use rustyline::{error::ReadlineError, Editor, Helper, Highlighter, Hinter, Validator};
use rustyline_derive::{Completer, Helper, Highlighter, Hinter, Validator};

#[derive(Helper, Completer, Highlighter, Hinter, Validator)]
struct InputHelper;

pub struct InputSystem {
    rl: Editor<InputHelper>,
    history_file: Option<std::path::PathBuf>,
}

impl InputSystem {
    pub fn new() -> Self {
        let mut rl = Editor::new().expect("Failed to create input editor");
        
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
    
    pub fn read_line(&mut self, prompt: &str) -> Result<String, ReadlineError> {
        let input = self.rl.readline(prompt)?;
        
        // 添加到历史记录
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

/// Slash 命令定义
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
    
    pub fn execute(&self) {
        match self {
            SlashCommand::Help => Self::show_help(),
            SlashCommand::History => println!("显示历史记录"),
            SlashCommand::Clear => println!("\x1b[2J\x1b[H"),
            SlashCommand::Exit => std::process::exit(0),
            SlashCommand::Verbose => println!("切换到详细模式"),
            SlashCommand::Quiet => println!("切换到安静模式"),
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

## 四、UI 模块重构后的文件结构

```
src/ui/
├── mod.rs                    # 主渲染入口（追加模式）
├── blocks.rs                 # 消息块类型定义
├── markdown.rs               # Markdown 渲染器
├── input.rs                  # 输入系统（行编辑、Slash 命令）
├── style.rs                  # 样式常量（颜色、间距）
└── output_impls.rs           # 消息输出实现（保持不变）
```

---

## 五、实施优先级

| 优先级 | 功能 | 理由 |
|--------|------|------|
| **P0** | 流式输出 | 立即解决闪烁和丢失历史问题 |
| **P0** | 代码语法高亮 | 代码阅读体验是核心需求 |
| **P1** | Markdown 渲染 | 提升文本可读性 |
| **P1** | 块级布局 | 结构化展示工具调用和结果 |
| **P2** | 行编辑输入 | 提升输入体验 |
| **P2** | Slash 命令 | 增加交互便捷性 |

---

## 六、预期效果对比

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

## 七、风险与注意事项

| 风险 | 应对措施 |
|------|----------|
| 终端兼容性 | 使用标准 ANSI 转义序列，避免使用终端特定功能 |
| 性能问题 | Markdown 渲染只在助手消息上执行，用户消息直接输出 |
| 依赖增加 | 只添加必要的依赖，保持轻量 |
| 滚动卡顿 | 长对话时考虑分页加载或历史压缩 |

---

## 八、总结

本方案参考 **grok-build** 的 `xai-grok-pager` 架构，提出了分阶段的 UI 优化计划：

1. **Phase 1** 解决最核心的闪烁问题，采用流式追加输出
2. **Phase 2** 提升内容可读性，添加 Markdown 和代码高亮
3. **Phase 3** 结构化消息展示，支持工具调用、思考状态等块类型
4. **Phase 4** 增强交互体验，添加行编辑和 Slash 命令

建议从 **Phase 1 + Phase 2** 开始，这两个阶段可以在不改变整体架构的情况下立即提升用户体验。
