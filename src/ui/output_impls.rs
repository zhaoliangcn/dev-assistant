use crate::utils::message_output::MessageOutput;
use crate::utils::message_level::MessageLevel;
use unicode_width::UnicodeWidthStr;

/// 流式助手内容的前缀（可见文本，不含 ANSI 转义）。
/// 用于计算续行缩进与可用宽度，避免把转义序列计入宽度导致错位。
const ASSISTANT_STREAM_PREFIX: &str = "🤖 助手: ";

// ---------------------------------------------------------------------------
// 交互模式：消息暂存到缓冲区，run() 返回后再写入 ContextManager
// ---------------------------------------------------------------------------

/// 交互 REPL 模式下的消息输出。消息先暂存到内部缓冲区，
/// 待 `agent.run()` 返回后由调用方统一写入 `ContextManager`。
/// 这样避免了在 `agent.run()` 执行期间同时持有 `&mut agent` 和
/// `&mut agent.context` 的双重可变借用问题。
pub struct UIMessageOutput {
    verbose: bool,
    buffer: Vec<(MessageLevel, String)>,
    /// 上一次流式渲染的助手内容，用于跳过重复渲染
    last_streamed_content: String,
    /// 上一次流式显示占用的终端行数（含自动换行），用于完整清除残留
    last_streamed_lines: usize,
    /// 流式完成后的待渲染助手内容，由主循环统一渲染避免重复
    pending_assistant_content: Option<String>,
    /// 累计的 Token 消耗数据（prompt, completion, total）。
    /// 由 `report_token_usage` 累计，不 emit 到 buffer，
    /// 调用方在 `process_user_message` 末尾单独渲染，避免与交互消息混在一起。
    token_usage: Option<(usize, usize, usize)>,
}

impl UIMessageOutput {
    pub fn new(verbose: bool) -> Self {
        Self {
            verbose,
            buffer: Vec::new(),
            last_streamed_content: String::new(),
            last_streamed_lines: 0,
            pending_assistant_content: None,
            token_usage: None,
        }
    }

    /// 取出所有暂存的消息并清空缓冲区。
    pub fn drain(&mut self) -> Vec<(MessageLevel, String)> {
        std::mem::take(&mut self.buffer)
    }

    /// 取出流式完成后累积的助手内容（消费式）。
    pub fn take_pending_assistant(&mut self) -> Option<String> {
        self.pending_assistant_content.take()
    }

    /// 取出累计的 Token 消耗数据（消费式）。
    /// 由调用方在流程末尾单独渲染，避免与交互消息混在一起。
    pub fn take_token_usage(&mut self) -> Option<(usize, usize, usize)> {
        self.token_usage.take()
    }

    /// 清除当前活跃的流式内容（含其占用的所有终端行），并重置流式状态。
    ///
    /// 流式帧之间一旦插入其他内容（如 drain 状态块、输入面板），旧的流式行
    /// 与新增行会交错堆叠，导致清理行数错位、内容互相覆盖。调用方应在渲染
    /// 其他内容前先清空活跃流区域，让流式内容始终位于屏幕最底部。
    pub fn clear_active_stream(&mut self) {
        use std::io::Write;

        if self.last_streamed_lines == 0 {
            return;
        }
        let mut stdout = std::io::stdout();
        for _ in 0..self.last_streamed_lines.saturating_sub(1) {
            let _ = write!(stdout, "\r\x1b[2K\x1b[A");
        }
        let _ = write!(stdout, "\r\x1b[2K");
        let _ = stdout.flush();
        self.last_streamed_content.clear();
        self.last_streamed_lines = 0;
    }

    /// 获取最后一条消息的内容（不消费缓冲区）。
    /// 用于在下一轮 step 前生成上下文状态提示。
    pub fn last_message(&self) -> Option<&str> {
        self.buffer.last().map(|(_, msg)| msg.as_str())
    }

    /// 获取最后一条消息的级别。
    #[allow(dead_code)]
    pub fn last_level(&self) -> Option<MessageLevel> {
        self.buffer.last().map(|(level, _)| *level)
    }
}

impl MessageOutput for UIMessageOutput {
    fn emit(&mut self, level: MessageLevel, msg: &str) {
        // 非 verbose 模式下跳过 Debug 和 Info 级别消息
        if !self.verbose && matches!(level, MessageLevel::Debug | MessageLevel::Info) {
            return;
        }
        // 去重：避免相同 (level, msg) 连续出现
        let entry = (level, msg.to_string());
        if self.buffer.last() != Some(&entry) {
            self.buffer.push(entry);
        }
    }

    /// 报告 Token 用量：累计到 `token_usage` 字段，不 emit 到 buffer，
    /// 由调用方在 `process_user_message` 末尾单独渲染，避免与交互消息混在一起。
    fn report_token_usage(&mut self, prompt_tokens: usize, completion_tokens: usize, total_tokens: usize) {
        self.token_usage = Some(match self.token_usage {
            Some((p, c, t)) => (p + prompt_tokens, c + completion_tokens, t + total_tokens),
            None => (prompt_tokens, completion_tokens, total_tokens),
        });
    }

    /// 流式输出助手消息，直接渲染到终端。
    ///
    /// 每次调用时，先清除上一次显示占用的**全部**终端行（含宽度换行产生的多行），
    /// 再写入累积的助手内容，避免长内容换行后残留旧行造成重复渲染。
    /// 完成后（is_final=true），输出完整内容并移除闪烁光标。
    fn streaming_assistant(&mut self, content: &str, is_final: bool) {
        use std::io::Write;

        let mut stdout = std::io::stdout();
        let term_width = crate::ui::get_terminal_width().unwrap_or(80);
        // 流式显示保留实际换行符，正确计算终端占用行数
        let total_lines = self.calc_streaming_lines(content, term_width);

        if is_final {
            // 清除上一次流式显示占用的所有行（含自动换行的残留行），再输出最终内容
            for _ in 0..self.last_streamed_lines.saturating_sub(1) {
                let _ = write!(stdout, "\r\x1b[2K\x1b[A");
            }
            // 清除第一行（流式内容所在行）
            let _ = write!(stdout, "\r\x1b[2K");
            // 存入待渲染缓冲区，由主循环统一以 Assistant 块渲染，
            // 避免与后续的 drain/render_block 重复
            if !content.is_empty() {
                self.pending_assistant_content = Some(content.to_string());
            }
            self.last_streamed_content.clear();
            self.last_streamed_lines = 0;
        } else {
            // 内容未变时跳过重复渲染，避免内容包含换行时产生多行残留
            if content == self.last_streamed_content {
                return;
            }
            // 先清除上一次显示占用的所有行，再写入新内容
            for _ in 0..self.last_streamed_lines.saturating_sub(1) {
                let _ = write!(stdout, "\r\x1b[2K\x1b[A");
            }
            let _ = write!(stdout, "\r\x1b[2K");

            self.last_streamed_content = content.to_string();
            self.last_streamed_lines = total_lines;

            // 渲染：第一行带前缀，后续行缩进对齐前缀宽度
            // 注意：前缀含 ANSI 转义，缩进宽度按可见文本计算，避免错位
            let theme = crate::ui::theme::active_theme();
            let prefix = format!("{}🤖 助手:{} ", theme.tool_fg, crate::ui::theme::RESET);
            let indent = " ".repeat(ASSISTANT_STREAM_PREFIX.width());
            
            for (i, line) in content.lines().enumerate() {
                if i == 0 {
                    let _ = write!(stdout, "{}{}", prefix, line);
                } else {
                    let _ = write!(stdout, "\n{}", indent);
                    let _ = write!(stdout, "{}", line);
                }
            }
            // 添加闪烁光标
            let _ = write!(stdout, " ▊");
            let _ = stdout.flush();
        }
    }
}

// ---------------------------------------------------------------------------
// UIMessageOutput 自有方法（非 trait 方法）
// ---------------------------------------------------------------------------

impl UIMessageOutput {
    /// 计算流式内容在终端上显示时占用的总行数。
    ///
    /// 流式渲染保留真实换行：第一行带前缀，后续行以相同宽度的空格缩进对齐，
    /// 因此每一段内容的可用宽度均为 `term_width - 前缀宽度`；
    /// 每段内按视觉宽度估算终端自动换行产生的行数，最后一行计入末尾光标。
    fn calc_streaming_lines(&self, content: &str, term_width: usize) -> usize {
        use unicode_width::UnicodeWidthChar;

        if content.is_empty() {
            return 1; // 至少保留光标行
        }

        let prefix_width = ASSISTANT_STREAM_PREFIX.width(); // 9
        let cursor_width = " ▊".width(); // 3 (space + ▊ emoji)
        let available = term_width.saturating_sub(prefix_width);

        // 与渲染端一致：按实际换行符分段。content.lines() 会丢弃末尾空段，
        // 避免对结尾换行符多计一行，与渲染循环完全对齐。
        let lines: Vec<&str> = content.lines().collect();
        let mut total: usize = 0;

        for (i, line) in lines.iter().enumerate() {
            let visual_width: usize = line.chars()
                .map(|c| c.width().unwrap_or(0))
                .sum();

            if visual_width == 0 {
                total += 1; // 空行（来自连续换行符）
            } else {
                total += (visual_width.saturating_sub(1) / available.max(1)) + 1;
            }

            // 最后一行末尾追加闪烁光标，若超宽则额外换行
            if i == lines.len() - 1 {
                let last_total_width = visual_width + cursor_width;
                if last_total_width > available && available > 0 {
                    total += 1;
                }
            }
        }

        total.max(1)
    }
}

// ---------------------------------------------------------------------------
// 非交互模式：消息直接输出到 stdout
// ---------------------------------------------------------------------------

/// 非交互模式（`--message` 参数一次性执行）下的消息输出。
/// 消息直接写入 stdout，带有级别标签。
pub struct CliMessageOutput {
    verbose: bool,
    /// 最后一次流式 final 且实际打印过的内容，供 `run_once` 去重，
    /// 避免最终结果被 streaming(final) 与 success() 重复输出。
    last_printed_stream: Option<String>,
}

impl CliMessageOutput {
    pub fn new(verbose: bool) -> Self {
        Self {
            verbose,
            last_printed_stream: None,
        }
    }

    /// 判断 `msg` 是否已通过流式输出打印过。
    /// 仅在 verbose 模式下 Info 级别实际输出时记录，非 verbose 下视为未输出。
    pub fn already_streamed(&self, msg: &str) -> bool {
        self.last_printed_stream.as_deref() == Some(msg)
    }
}

impl MessageOutput for CliMessageOutput {
    fn emit(&mut self, level: MessageLevel, msg: &str) {
        // 非 verbose 模式下跳过 Debug 和 Info 级别消息（与 UIMessageOutput 保持一致）
        if !self.verbose && matches!(level, MessageLevel::Debug | MessageLevel::Info) {
            return;
        }

        let prefix = level.label();
        println!("{} {}", prefix, msg);
    }

    /// 流式输出：最终内容以 Info 级别输出一次并记录，供 `run_once` 去重。
    /// 非 final 块不输出（CLI 模式不做逐帧刷新，最终内容由 final 块统一输出）。
    fn streaming_assistant(&mut self, content: &str, is_final: bool) {
        if is_final && !content.is_empty() {
            if self.verbose {
                let prefix = MessageLevel::Info.label();
                println!("{} {}", prefix, content);
                self.last_printed_stream = Some(content.to_string());
            } else {
                // 非 verbose 下 Info 会被 emit 过滤，视为未输出
                self.last_printed_stream = None;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 静默模式（测试等）
// ---------------------------------------------------------------------------

/// 静默模式，不输出任何消息。用于测试场景。
#[allow(dead_code)]
pub struct SilentMessageOutput;

impl MessageOutput for SilentMessageOutput {
    fn emit(&mut self, _level: MessageLevel, _msg: &str) {}
}
