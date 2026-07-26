//! Markdown 渲染器（支持语法高亮）

use std::sync::LazyLock;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
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
#[derive(Debug, Clone, Default)]
pub struct MarkdownRenderer;

impl MarkdownRenderer {
    pub fn new() -> Self {
        Self
    }
    
    /// 渲染 Markdown 文本为终端格式
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
                Event::End(tag_end) => {
                    if matches!(tag_end, TagEnd::CodeBlock) {
                        in_code_block = false;
                        code_block_lang.clear();
                    } else {
                        self.render_end_tag(&mut output, &tag_end);
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
    
    /// 渲染围栏代码块（使用 HighlightLines 正确 API）。
    fn render_code_block(&self, output: &mut String, lang: &str, code: &str) {
        let syntax = SYNTAX_SET
            .find_syntax_by_token(lang)
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
    
    /// 渲染行内代码
    fn render_inline_code(&self, output: &mut String, code: &str) {
        output.push_str("\x1b[38;2;156;189;248m`");  // 蓝色
        output.push_str(code);
        output.push_str("`\x1b[0m");
    }
    
    /// 渲染标签开始
    fn render_start_tag(&self, output: &mut String, tag: &Tag) {
        match tag {
            Tag::Strong => output.push_str("\x1b[1m"),
            Tag::Emphasis => output.push_str("\x1b[3m"),
            Tag::Link { .. } => output.push_str("\x1b[4;34m"),
            Tag::List(..) => {}
            Tag::Item => output.push_str("• "),
            Tag::Paragraph => {}
            Tag::Heading { .. } => output.push_str("\x1b[1;33m"),  // 粗体黄色
            _ => {}
        }
    }
    
    /// 渲染标签结束
    fn render_end_tag(&self, output: &mut String, tag_end: &TagEnd) {
        match tag_end {
            TagEnd::Strong | TagEnd::Emphasis | TagEnd::Heading(..) => output.push_str("\x1b[0m"),
            TagEnd::Link => output.push_str("\x1b[0m"),
            TagEnd::Paragraph => output.push('\n'),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_inline_code() {
        let renderer = MarkdownRenderer::new();
        let result = renderer.render("`let x = 1;`");
        assert!(result.contains("let x = 1;"));
        assert!(result.contains("\x1b[38;2;156;189;248m"));  // 蓝色
    }
    
    #[test]
    fn test_bold() {
        let renderer = MarkdownRenderer::new();
        let result = renderer.render("**bold text**");
        assert!(result.contains("\x1b[1m"));  // 粗体
        assert!(result.contains("\x1b[0m"));  // 重置
    }
    
    #[test]
    fn test_heading() {
        let renderer = MarkdownRenderer::new();
        let result = renderer.render("# Heading");
        assert!(result.contains("\x1b[1;33m"));  // 粗体黄色
    }
    
    #[test]
    fn test_code_block() {
        let renderer = MarkdownRenderer::new();
        let result = renderer.render("```rust\nfn main() {}\n```");
        // 检查是否包含代码内容（可能带有 ANSI 颜色代码）
        assert!(result.contains("fn") && result.contains("main"), "Code block should contain function name");
    }
}
