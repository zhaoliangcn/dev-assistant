//! Markdown 渲染器（支持语法高亮）

use std::sync::LazyLock;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};

/// 移除 ANSI 转义序列，返回纯文本（用于宽度计算）。
fn strip_ansi_escapes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // 跳过 ESC[...m 序列
            if chars.next() == Some('[') {
                for ch in chars.by_ref() {
                    if ch == 'm' {
                        break;
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// 全局语法集缓存（~150 种语法定义，加载约 30-50ms）。
/// 使用 LazyLock 确保只加载一次，避免每次创建 MarkdownRenderer 时重复加载。
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(|| {
    SyntaxSet::load_defaults_newlines()
});

/// 全局主题集缓存。
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(|| {
    ThemeSet::load_defaults()
});

/// Markdown 渲染器（渲染样式来自当前主题，语法集/主题集为全局缓存）。
///
/// 主题默认取 [`crate::ui::theme::active_theme`]（自动检测亮/暗），
/// 测试可通过 [`MarkdownRenderer::with_theme`] 注入固定主题保证断言稳定。
#[derive(Debug, Clone)]
pub struct MarkdownRenderer {
    theme: crate::ui::theme::Theme,
}

impl Default for MarkdownRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownRenderer {
    pub fn new() -> Self {
        Self {
            theme: *crate::ui::theme::active_theme(),
        }
    }

    /// 使用指定主题创建渲染器（测试/定制用）。
    #[must_use]
    pub fn with_theme(theme: crate::ui::theme::Theme) -> Self {
        Self { theme }
    }
    
    /// 渲染 Markdown 文本为终端格式
    pub fn render(&self, markdown: &str) -> String {
        let mut output = String::new();
        let parser = Parser::new_ext(markdown, Options::all());

        let mut code_block_lang = String::new();
        let mut in_code_block = false;

        // ── 表格渲染状态 ──
        let mut in_table = false;
        let mut in_table_head = false;
        let mut in_cell = false;
        let mut current_row: Vec<String> = Vec::new();
        let mut current_cell = String::new();
        let mut table_header: Vec<String> = Vec::new();
        let mut table_rows: Vec<Vec<String>> = Vec::new();

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
                        Tag::Table(_) => {
                            in_table = true;
                            table_header.clear();
                            table_rows.clear();
                            current_row.clear();
                        }
                        Tag::TableHead => {
                            in_table_head = true;
                        }
                        Tag::TableRow => {
                            current_row.clear();
                        }
                        Tag::TableCell => {
                            in_cell = true;
                            current_cell.clear();
                        }
                        _ => {
                            if !in_table {
                                self.render_start_tag(&mut output, &tag);
                            }
                        }
                    }
                }
                Event::End(tag_end) => {
                    if matches!(tag_end, TagEnd::CodeBlock) {
                        in_code_block = false;
                        code_block_lang.clear();
                    } else if in_table {
                        match &tag_end {
                            TagEnd::TableCell => {
                                in_cell = false;
                                current_row.push(current_cell.clone());
                                current_cell.clear();
                            }
                            TagEnd::TableRow => {
                                if in_table_head {
                                    table_header = current_row.clone();
                                    in_table_head = false;
                                } else if !current_row.is_empty() {
                                    table_rows.push(current_row.clone());
                                }
                                current_row.clear();
                            }
                            TagEnd::TableHead => {
                                // Header rows are captured in TableRow end handler
                            }
                            TagEnd::Table => {
                                self.render_table(&mut output, &table_header, &table_rows);
                                in_table = false;
                                table_header.clear();
                                table_rows.clear();
                                current_row.clear();
                            }
                            _ => {}
                        }
                    } else {
                        self.render_end_tag(&mut output, &tag_end);
                    }
                }
                Event::Text(text) => {
                    if in_code_block {
                        self.render_code_block(&mut output, &code_block_lang, &text);
                    } else if in_cell {
                        current_cell.push_str(&text);
                    } else {
                        output.push_str(&text);
                    }
                }
                Event::Code(code) => {
                    if in_cell {
                        current_cell.push_str(&format!(
                            "{}{}{}",
                            self.theme.code_fg, code, crate::ui::theme::RESET
                        ));
                    } else {
                        self.render_inline_code(&mut output, &code);
                    }
                }
                Event::HardBreak => {
                    if in_cell {
                        current_cell.push('\n');
                    } else {
                        output.push('\n');
                    }
                }
                Event::SoftBreak => {
                    if in_cell {
                        current_cell.push('\n');
                    } else {
                        output.push('\n');
                    }
                }
                _ => {}
            }
        }

        output
    }

    /// 渲染表格为 ASCII 边框格式。
    fn render_table(&self, output: &mut String, header: &[String], rows: &[Vec<String>]) {
        // 计算每列最大宽度
        let num_cols = header.len().max(
            rows.iter().map(|r| r.len()).max().unwrap_or(0)
        );
        if num_cols == 0 {
            return;
        }

        let mut col_widths: Vec<usize> = header.iter().map(|c| strip_ansi_escapes(c).len()).collect();
        for row in rows {
            for (i, cell) in row.iter().enumerate() {
                if i < col_widths.len() {
                    // 只取第一行宽度（忽略 ANSI 序列）
                    let plain = strip_ansi_escapes(cell);
                    col_widths[i] = col_widths[i].max(plain.len());
                } else {
                    let plain = strip_ansi_escapes(cell);
                    col_widths.push(plain.len());
                }
            }
        }
        // 确保最小宽度
        for w in &mut col_widths {
            *w = (*w).max(3);
        }

        // ── 顶边框 ──
        output.push('┌');
        for (i, w) in col_widths.iter().enumerate() {
            output.push_str(&"─".repeat(*w + 2));
            if i < col_widths.len() - 1 {
                output.push('┬');
            }
        }
        output.push_str("┐\n");

        // ── 表头 ──
        if !header.is_empty() {
            output.push('│');
            for (i, cell) in header.iter().enumerate() {
                let w = col_widths.get(i).copied().unwrap_or(3);
                let padding = w.saturating_sub(strip_ansi_escapes(cell).len());
                let left = padding / 2;
                let right = padding - left;
                output.push_str(&format!(
                    " \x1b[1m{}\x1b[0m{}{}│",
                    cell,
                    " ".repeat(left),
                    " ".repeat(right),
                ));
            }
            output.push('\n');

            // ── 表头/数据分隔线 ──
            output.push('├');
            for (i, w) in col_widths.iter().enumerate() {
                output.push_str(&"─".repeat(*w + 2));
                if i < col_widths.len() - 1 {
                    output.push('┼');
                }
            }
            output.push_str("┤\n");
        }

        // ── 数据行 ──
        for row in rows {
            output.push('│');
            for i in 0..num_cols {
                let cell = row.get(i).map(|s| s.as_str()).unwrap_or("");
                let w = col_widths.get(i).copied().unwrap_or(3);
                let plain = strip_ansi_escapes(cell);
                let padding = w.saturating_sub(plain.len());
                output.push_str(&format!(" {}{}│", cell, " ".repeat(padding)));
            }
            output.push('\n');
        }

        // ── 底边框 ──
        output.push('└');
        for (i, w) in col_widths.iter().enumerate() {
            output.push_str(&"─".repeat(*w + 2));
            if i < col_widths.len() - 1 {
                output.push('┴');
            }
        }
        output.push_str("┘\n");
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

        // 语法高亮主题：暗色用 base16-ocean.dark，亮色用 InspiredGitHub
        let syntect_theme_name = if self.theme.mode == crate::ui::theme::ThemeMode::Light {
            "InspiredGitHub"
        } else {
            "base16-ocean.dark"
        };
        let syntect_theme = &THEME_SET.themes[syntect_theme_name];
        let mut highlighter = HighlightLines::new(syntax, syntect_theme);

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
    /// 配色（来自当前主题）：
    /// - `+` 开头 → `diff_added_fg` 前景 + `diff_added_bg` 背景（绿）
    /// - `-` 开头 → `diff_deleted_fg` 前景 + `diff_deleted_bg` 背景（红）
    /// - `@@` 开头 → `diff_hunk_fg`（青色粗体）
    /// - `---`/`+++` 开头 → `muted_fg`（暗淡）
    /// - 其余行 → 普通
    fn render_diff_block(&self, output: &mut String, code: &str) {
        let added_fg = self.theme.diff_added_fg;
        let added_bg = self.theme.diff_added_bg;
        let deleted_fg = self.theme.diff_deleted_fg;
        let deleted_bg = self.theme.diff_deleted_bg;
        let hunk = self.theme.diff_hunk_fg;
        let dim = self.theme.muted_fg;
        let reset = crate::ui::theme::RESET;

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
                output.push_str(dim);
                output.push_str(line);
                output.push_str(reset);
            } else {
                match first {
                    '+' => {
                        output.push_str(added_fg);
                        output.push_str(added_bg);
                        output.push_str(line);
                        output.push_str(reset);
                    }
                    '-' => {
                        output.push_str(deleted_fg);
                        output.push_str(deleted_bg);
                        output.push_str(line);
                        output.push_str(reset);
                    }
                    '@' if line.starts_with("@@") => {
                        output.push_str(hunk);
                        output.push_str(line);
                        output.push_str(reset);
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
        output.push_str(self.theme.code_fg);  // 行内代码前景色
        output.push('`');
        output.push_str(code);
        output.push('`');
        output.push_str(crate::ui::theme::RESET);
    }
    
    /// 渲染标签开始
    fn render_start_tag(&self, output: &mut String, tag: &Tag) {
        match tag {
            Tag::Strong => output.push_str(crate::ui::theme::BOLD),
            Tag::Emphasis => output.push_str(crate::ui::theme::ITALIC),
            Tag::Link { .. } => output.push_str(self.theme.link_fg),
            Tag::List(..) => {}
            Tag::Item => output.push_str("• "),
            Tag::Paragraph => {}
            Tag::Heading { .. } => output.push_str(self.theme.heading_fg),  // 标题
            _ => {}
        }
    }
    
    /// 渲染标签结束
    fn render_end_tag(&self, output: &mut String, tag_end: &TagEnd) {
        match tag_end {
            TagEnd::Strong | TagEnd::Emphasis | TagEnd::Heading(..) => {
                output.push_str(crate::ui::theme::RESET);
            }
            TagEnd::Link => output.push_str(crate::ui::theme::RESET),
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
        let renderer = MarkdownRenderer::with_theme(crate::ui::theme::Theme::dark());
        let result = renderer.render("`let x = 1;`");
        assert!(result.contains("let x = 1;"));
        assert!(result.contains("\x1b[38;2;156;189;248m"));  // 蓝色
    }
    
    #[test]
    fn test_bold() {
        let renderer = MarkdownRenderer::with_theme(crate::ui::theme::Theme::dark());
        let result = renderer.render("**bold text**");
        assert!(result.contains("\x1b[1m"));  // 粗体
        assert!(result.contains("\x1b[0m"));  // 重置
    }
    
    #[test]
    fn test_heading() {
        let renderer = MarkdownRenderer::with_theme(crate::ui::theme::Theme::dark());
        let result = renderer.render("# Heading");
        assert!(result.contains("\x1b[1;33m"));  // 粗体黄色
    }
    
    #[test]
    fn test_code_block() {
        let renderer = MarkdownRenderer::with_theme(crate::ui::theme::Theme::dark());
        let result = renderer.render("```rust\nfn main() {}\n```");
        // 包含行号（灰色 ANSI 序列）
        assert!(result.contains("\x1b[2m"), "Code block should contain dim line numbers");
        assert!(result.contains("fn") && result.contains("main"), "Code block should contain function name");
        assert!(result.contains(" 1"), "Should have line number 1");
    }

    #[test]
    fn test_code_block_with_filename() {
        let renderer = MarkdownRenderer::with_theme(crate::ui::theme::Theme::dark());
        let result = renderer.render("```rust:src/main.rs\nfn main() {}\n```");
        // 应包含文件名栏
        assert!(result.contains("src/main.rs"), "Should contain filename in header");
        assert!(result.contains("\x1b[2m"), "Should have dim styling for header and line numbers");
    }

    #[test]
    fn test_code_block_single_line() {
        let renderer = MarkdownRenderer::with_theme(crate::ui::theme::Theme::dark());
        let result = renderer.render("```rust\nlet x = 1;\n```");
        // 至少包含行号
        assert!(result.contains("\x1b[2m"), "Should have line numbers");
    }

    #[test]
    fn test_diff_block_added_lines() {
        let renderer = MarkdownRenderer::with_theme(crate::ui::theme::Theme::dark());
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
        let renderer = MarkdownRenderer::with_theme(crate::ui::theme::Theme::dark());
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
        let renderer = MarkdownRenderer::with_theme(crate::ui::theme::Theme::dark());
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
        let renderer = MarkdownRenderer::with_theme(crate::ui::theme::Theme::dark());
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
        let renderer = MarkdownRenderer::with_theme(crate::ui::theme::Theme::dark());
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

    #[test]
    fn test_diff_block_backgrounds() {
        let renderer = MarkdownRenderer::with_theme(crate::ui::theme::Theme::dark());
        let diff = "\
```diff
+fn new_function() {}
-old_function()
```";
        let result = renderer.render(diff);
        // 新增行应带绿底（48;2 背景色），删除行应带红底
        assert!(
            result.contains("\x1b[48;2;18;42;34m"),
            "Added lines should have dark green background"
        );
        assert!(
            result.contains("\x1b[48;2;42;18;26m"),
            "Removed lines should have dark red background"
        );
        // 前景色仍保留
        assert!(result.contains("\x1b[38;2;72;187;120m"), "Added fg preserved");
        assert!(result.contains("\x1b[38;2;239;68;68m"), "Removed fg preserved");
    }
}
