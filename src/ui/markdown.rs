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
    
    /// 渲染围栏代码块。
    ///
    /// 对 `diff` 语言使用专门的 diff 渲染器（绿/红配色），
    /// 其他语言使用 syntect 语法高亮，并添加文件名栏和行号。
    ///
    /// 支持 `lang:filename` 格式从 info string 提取文件名，例如：
    /// ```text
    /// ```rust:src/main.rs
    /// ```
    fn render_code_block(&self, output: &mut String, lang: &str, code: &str) {
        if lang.eq_ignore_ascii_case("diff") {
            self.render_diff_block(output, code);
            return;
        }

        // 解析语言和文件名（支持 `lang:filename` 格式）
        let (clean_lang, filename) = if let Some(idx) = lang.find(':') {
            (&lang[..idx], Some(&lang[idx + 1..]))
        } else {
            (lang, None)
        };

        let syntax = SYNTAX_SET
            .find_syntax_by_token(clean_lang)
            .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());

        let theme = &THEME_SET.themes["base16-ocean.dark"];
        let mut highlighter = HighlightLines::new(syntax, theme);

        // 收集所有行（含换行标记）
        let lines: Vec<&str> = LinesWithEndings::from(code).collect();
        let total_lines = lines.len();
        let line_num_width = if total_lines > 0 {
            (total_lines as f64).log10().floor() as usize + 1
        } else {
            1
        };
        // 确保至少 2 位宽度
        let line_num_width = line_num_width.max(2);

        // ── 文件名栏 ──
        let header = filename.unwrap_or(clean_lang);
        if !header.is_empty() {
            output.push_str(&format!(
                "\x1b[2m── {} ──\x1b[0m\n",
                header
            ));
        }

        // ── 行号 + 代码 ──
        for (i, line) in lines.iter().enumerate() {
            let line_num = i + 1;
            // 行号（灰色，右对齐）
            output.push_str(&format!(
                "\x1b[2m{:>width$}\x1b[0m ",
                line_num,
                width = line_num_width
            ));

            // 语法高亮的代码行
            if let Ok(ranges) = highlighter.highlight_line(line, &SYNTAX_SET) {
                output.push_str(&as_24_bit_terminal_escaped(&ranges[..], false));
            } else {
                output.push_str(line);
            }
        }

        // 末尾保证换行
        if !code.ends_with('\n') {
            output.push('\n');
        }
    }

    /// 渲染 diff 代码块（unified diff 格式）。
    ///
    /// 配色：
    /// - `+` 开头 → 🟢 绿色 `\x1b[38;2;72;187;120m`
    /// - `-` 开头 → 🔴 红色 `\x1b[38;2;239;68;68m`
    /// - `@@` 开头 → 青色粗体 `\x1b[1;38;2;79;193;255m`
    /// - `---`/`+++` 开头 → 暗淡 `\x1b[2m`
    /// - 其余行 → 普通
    const DIFF_GREEN: &str = "\x1b[38;2;72;187;120m";
    const DIFF_RED: &str = "\x1b[38;2;239;68;68m";
    const DIFF_CYAN: &str = "\x1b[1;38;2;79;193;255m";
    const DIFF_DIM: &str = "\x1b[2m";
    const RESET: &str = "\x1b[0m";

    fn render_diff_block(&self, output: &mut String, code: &str) {
        for line in LinesWithEndings::from(code) {
            let line = line.strip_suffix('\n').unwrap_or(line);
            let line = line.strip_suffix('\r').unwrap_or(line);

            if line.is_empty() {
                output.push('\n');
                continue;
            }

            let first = line.chars().next().unwrap_or(' ');

            // 先检查 ---/+++ 文件头，再检查 +/-
            if line.starts_with("---") || line.starts_with("+++") {
                output.push_str(Self::DIFF_DIM);
                output.push_str(line);
                output.push_str(Self::RESET);
            } else {
                match first {
                    '+' => {
                        output.push_str(Self::DIFF_GREEN);
                        output.push_str(line);
                        output.push_str(Self::RESET);
                    }
                    '-' => {
                        output.push_str(Self::DIFF_RED);
                        output.push_str(line);
                        output.push_str(Self::RESET);
                    }
                    '@' if line.starts_with("@@") => {
                        output.push_str(Self::DIFF_CYAN);
                        output.push_str(line);
                        output.push_str(Self::RESET);
                    }
                    _ => {
                        output.push_str(line);
                    }
                }
            }
            output.push('\n');
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
        // 包含行号（灰色 ANSI 序列）
        assert!(result.contains("\x1b[2m"), "Code block should contain dim line numbers");
        assert!(result.contains("fn") && result.contains("main"), "Code block should contain function name");
        assert!(result.contains(" 1"), "Should have line number 1");
    }

    #[test]
    fn test_code_block_with_filename() {
        let renderer = MarkdownRenderer::new();
        let result = renderer.render("```rust:src/main.rs\nfn main() {}\n```");
        // 应包含文件名栏
        assert!(result.contains("src/main.rs"), "Should contain filename in header");
        assert!(result.contains("\x1b[2m"), "Should have dim styling for header and line numbers");
    }

    #[test]
    fn test_code_block_single_line() {
        let renderer = MarkdownRenderer::new();
        let result = renderer.render("```rust\nlet x = 1;\n```");
        // 至少包含行号
        assert!(result.contains("\x1b[2m"), "Should have line numbers");
    }

    #[test]
    fn test_diff_block_added_lines() {
        let renderer = MarkdownRenderer::new();
        let diff = "\
```diff
+fn new_function() {}
+// new comment
```";
        let result = renderer.render(diff);
        // 新增行应为绿色
        assert!(result.contains("\x1b[38;2;72;187;120m"), "Added lines should be green");
        assert!(result.contains("new_function"), "Code content should be preserved");
    }

    #[test]
    fn test_diff_block_removed_lines() {
        let renderer = MarkdownRenderer::new();
        let diff = "\
```diff
-old_function()
-old_comment
```";
        let result = renderer.render(diff);
        // 删除行应为红色
        assert!(result.contains("\x1b[38;2;239;68;68m"), "Removed lines should be red");
        assert!(result.contains("old_function"), "Code content should be preserved");
    }

    #[test]
    fn test_diff_block_hunk_header() {
        let renderer = MarkdownRenderer::new();
        let diff = "\
```diff
@@ -1,4 +1,5 @@
 context
+added
```";
        let result = renderer.render(diff);
        // @@ 行应为青色粗体
        assert!(result.contains("\x1b[1;38;2;79;193;255m"), "Hunk header should be cyan bold");
        assert!(result.contains("@@"), "Hunk header should contain @@");
    }

    #[test]
    fn test_diff_block_file_header() {
        let renderer = MarkdownRenderer::new();
        let diff = "\
```diff
--- a/old.rs
+++ b/new.rs
```";
        let result = renderer.render(diff);
        // 文件头应为暗淡
        assert!(result.contains("\x1b[2m"), "File headers should be dim");
        assert!(result.contains("old.rs"), "File path should be preserved");
    }

    #[test]
    fn test_diff_block_mixed() {
        let renderer = MarkdownRenderer::new();
        let diff = "\
```diff
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 fn hello() {
-    println!(\"old\");
+    println!(\"new\");
 }
```";
        let result = renderer.render(diff);
        // 应包含所有三种 ANSI 颜色
        assert!(result.contains("\x1b[38;2;72;187;120m"), "Should contain green for added");
        assert!(result.contains("\x1b[38;2;239;68;68m"), "Should contain red for removed");
        assert!(result.contains("\x1b[2m"), "Should contain dim for file headers");
        assert!(result.contains("println"), "Code content should be preserved");
        assert!(result.contains("\x1b[0m"), "Should reset colors");
    }
}
