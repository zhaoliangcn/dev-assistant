//! 半透明（玻璃拟态）界面支持。
//!
//! # 技术现实
//! 终端窗口本身的透明度由**终端模拟器**控制（iTerm2 的 Transparency 滑块、
//! kitty/Alacritty 的 `background_opacity`、WezTerm 的 `window_background_opacity`
//! 等），CLI 程序无法直接修改 OS 窗口的透明度。
//!
//! 本模块实现 CLI 侧能做到的两件事：
//! 1. 通过 **OSC 11** 查询终端真实背景色，把 UI 中的实心色块（如 diff 的绿/红底）
//!    与背景色做 **alpha 混合**，得到"半透明玻璃"观感——这在任何终端上都能生效。
//! 2. 若终端本身已开启透明，本模块探测到背景色后生成的混色背景不会"挡住"透明度，
//!    观感更接近毛玻璃。
//!
//! # 使用
//! - [`init`]：启动时调用一次（查询背景色并开启半透明模式）。
//! - [`glass_background`]：渲染色块时调用，返回与终端背景混色的 ANSI 背景序列。
//! - [`enabled`] / [`detected_bg`]：查询状态。

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 半透明模式是否启用（由 `--translucent` 开关控制）。
static TRANSLUCENT_ENABLED: AtomicBool = AtomicBool::new(false);

/// 探测到的终端背景色（8 位 RGB）；`None` 表示未探测到（终端不支持或非 TTY）。
static DETECTED_BG: OnceLock<Option<(u8, u8, u8)>> = OnceLock::new();

/// 色块叠加的默认透明度（0.0=完全透明，1.0=不透明）。
pub const GLASS_ALPHA: f32 = 0.25;

/// 半透明模式是否启用。
pub fn enabled() -> bool {
    TRANSLUCENT_ENABLED.load(Ordering::Relaxed)
}

/// 开启半透明模式并查询终端背景色。
///
/// 应在 REPL 读取 stdin 之前调用（例如 [`crate::App::run`] 之前），
/// 因为查询需要短暂占用 stdin 接收 OSC 11 的回复。
pub fn init() {
    TRANSLUCENT_ENABLED.store(true, Ordering::Relaxed);
    let bg = query_background_color();
    let _ = DETECTED_BG.set(bg);
}

/// 已探测到的终端背景色（若有）。
pub fn detected_bg() -> Option<(u8, u8, u8)> {
    DETECTED_BG.get().copied().flatten()
}

/// 标准 alpha 混合：`out = fg * alpha + bg * (1 - alpha)`。
pub fn blend(fg: (u8, u8, u8), alpha: f32, bg: (u8, u8, u8)) -> (u8, u8, u8) {
    let mix = |f: u8, b: u8| (f as f32 * alpha + b as f32 * (1.0 - alpha)).round() as u8;
    (mix(fg.0, bg.0), mix(fg.1, bg.1), mix(fg.2, bg.2))
}

/// 将色块渲染为"玻璃拟态"背景：把 `tint` 以 `alpha` 透明度叠加到终端背景色上。
///
/// 未启用半透明模式或未探测到背景色时返回 `None`，调用方应回退到主题默认值。
pub fn glass_background(tint: (u8, u8, u8), alpha: f32) -> Option<String> {
    if !enabled() {
        return None;
    }
    let bg = detected_bg()?;
    let (r, g, b) = blend(tint, alpha, bg);
    Some(format!("\x1b[48;2;{};{};{}m", r, g, b))
}

/// 通过 OSC 11 查询终端背景色。
///
/// 发送 `ESC ] 11 ; ? ESC \`，终端回复 `ESC ] 11 ; rgb:rrrr/gggg/bbbb ESC \`
/// （或 `#rrggbb` 形式）。带超时，避免在无响应的终端上阻塞。
fn query_background_color() -> Option<(u8, u8, u8)> {
    // 非交互环境（管道/重定向）不查询，避免读到外部输入或污染输出。
    if !is_tty() {
        return None;
    }
    let mut stdout = io::stdout();
    write!(stdout, "\x1b]11;?\x1b\\").ok()?;
    stdout.flush().ok()?;
    read_osc_reply(100)
}

/// 读取 OSC 回复，最多等待 `timeout_ms` 毫秒。
#[cfg(unix)]
fn read_osc_reply(timeout_ms: u64) -> Option<(u8, u8, u8)> {
    use libc::{poll, pollfd, POLLIN};
    use std::os::unix::io::AsRawFd;

    let stdin = io::stdin();
    let fd = stdin.as_raw_fd();
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut buf: Vec<u8> = Vec::new();

    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline.saturating_duration_since(now).as_millis() as i32;
        let mut fds = [pollfd {
            fd,
            events: POLLIN,
            revents: 0,
        }];
        let rc = unsafe { poll(fds.as_mut_ptr(), 1, remaining) };
        if rc <= 0 {
            break;
        }
        let mut tmp = [0u8; 64];
        let n = unsafe { libc::read(fd, tmp.as_mut_ptr() as *mut libc::c_void, tmp.len()) };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n as usize]);
        // OSC 回复以 BEL (\x07) 或 ST (ESC \) 结束
        if buf.contains(&0x07) || buf.windows(2).any(|w| w == b"\x1b\\") {
            break;
        }
    }

    parse_osc_reply(&buf)
}

/// 非 unix 平台不查询（直接跳过）。
#[cfg(not(unix))]
fn read_osc_reply(_timeout_ms: u64) -> Option<(u8, u8, u8)> {
    None
}

/// stdout 与 stdin 是否都是 TTY。
#[cfg(unix)]
fn is_tty() -> bool {
    unsafe { libc::isatty(libc::STDOUT_FILENO) == 1 && libc::isatty(libc::STDIN_FILENO) == 1 }
}

#[cfg(not(unix))]
fn is_tty() -> bool {
    false
}

/// 解析 OSC 11 回复，如 `\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\` 或 `\x1b]11;#1e1e1e\x1b\\`。
fn parse_osc_reply(raw: &[u8]) -> Option<(u8, u8, u8)> {
    let s = String::from_utf8_lossy(raw);
    let body = s.split("11;").nth(1)?;
    let body = body.split('\x07').next().unwrap_or(body);
    let body = body.split("\x1b\\").next().unwrap_or(body);
    let body = body.trim();

    // `#rrggbb` 形式
    if let Some(hex) = body.strip_prefix('#') {
        if hex.len() >= 6 {
            return Some((
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
            ));
        }
        return None;
    }

    // `rgb:rrrr/gggg/bbbb`（16 位）或 `rgb:rr/gg/bb`（8 位）
    let (_, channels) = body.split_once(':')?;
    let mut parts = channels.split('/');
    let r = parse_channel(parts.next()?)?;
    let g = parse_channel(parts.next()?)?;
    let b = parse_channel(parts.next()?)?;
    Some((r, g, b))
}

/// 解析单个颜色通道：支持 8 位（`rr`）与 16 位（`rrrr`，取高位字节）。
fn parse_channel(s: &str) -> Option<u8> {
    let s = s.trim();
    match s.len() {
        2 => u8::from_str_radix(s, 16).ok(),
        4 => u8::from_str_radix(&s[0..2], 16).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_16bit_osc_reply() {
        let raw = b"\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\";
        assert_eq!(parse_osc_reply(raw), Some((0x1e, 0x1e, 0x1e)));
    }

    #[test]
    fn parses_8bit_osc_reply() {
        let raw = b"\x1b]11;rgb:1e/1e/1e\x1b\\";
        assert_eq!(parse_osc_reply(raw), Some((0x1e, 0x1e, 0x1e)));
    }

    #[test]
    fn parses_hex_osc_reply() {
        let raw = b"\x1b]11;#123456\x07";
        assert_eq!(parse_osc_reply(raw), Some((0x12, 0x34, 0x56)));
    }

    #[test]
    fn blend_mixes_toward_background() {
        // 纯红以 0.5 叠加到纯黑 → (128, 0, 0)
        assert_eq!(blend((255, 0, 0), 0.5, (0, 0, 0)), (128, 0, 0));
        // alpha=0 → 完全背景色；alpha=1 → 完全前景色
        assert_eq!(blend((255, 0, 0), 0.0, (10, 20, 30)), (10, 20, 30));
        assert_eq!(blend((255, 0, 0), 1.0, (10, 20, 30)), (255, 0, 0));
    }

    #[test]
    fn glass_background_disabled_returns_none() {
        TRANSLUCENT_ENABLED.store(false, Ordering::Relaxed);
        assert!(glass_background((18, 42, 34), GLASS_ALPHA).is_none());
    }
}
