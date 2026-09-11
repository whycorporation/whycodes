//! Terminal colour capability and RGB quantization.
//!
//! Apple Terminal.app (and other non-truecolor hosts) drop or mis-handle
//! `38;2` / `48;2`. Detect the host, quantize palette RGB to xterm-256 or
//! 16-colour *before* paint, and wrap the crossterm backend so a stray
//! `Color::Rgb` never reaches the wire.
//!
//! Windows is the other trap: Windows Terminal / PowerShell often leave
//! `TERM` and `COLORTERM` unset. Treating that as 16-colour collapses the
//! theme (and agent-tinted prompt borders) onto gray.

use std::cell::Cell;
use std::io::{self, Write};

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell as BufferCell;
use ratatui::layout::{Position, Size};
use ratatui::style::Color;

thread_local! {
    static ACTIVE: Cell<ColorMode> = const { Cell::new(ColorMode::TrueColor) };
}

/// How many colour bits the host is trusted to honour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorMode {
    /// `38;2` / `48;2` truecolor.
    TrueColor,
    /// xterm 256 (`38;5;n`).
    Ansi256,
    /// 16 ANSI colours (`38;5;0`–`15` or named).
    Ansi16,
}

impl ColorMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TrueColor => "truecolor",
            Self::Ansi256 => "256",
            Self::Ansi16 => "16",
        }
    }

    pub fn is_truecolor(self) -> bool {
        matches!(self, Self::TrueColor)
    }
}

/// Guard that restores the previous paint-time [`ColorMode`] on drop.
pub struct ColorModeGuard(ColorMode);

impl Drop for ColorModeGuard {
    fn drop(&mut self) {
        set_active_color_mode(self.0);
    }
}

/// Colour mode used by [`paint_color`] / [`elevate`](crate::ui::scrollbar::elevate)
/// helpers during this thread's paint.
pub fn active_color_mode() -> ColorMode {
    ACTIVE.with(Cell::get)
}

pub fn set_active_color_mode(mode: ColorMode) {
    ACTIVE.with(|c| c.set(mode));
}

/// Set the paint-time mode, restoring the previous value when the guard drops.
pub fn push_color_mode(mode: ColorMode) -> ColorModeGuard {
    let prev = active_color_mode();
    set_active_color_mode(mode);
    ColorModeGuard(prev)
}

/// Quantize `c` to the thread-local paint mode.
pub fn paint_color(c: Color) -> Color {
    quantize_color(c, active_color_mode())
}

/// Quantize an sRGB triple to the thread-local paint mode.
pub fn paint_rgb(r: u8, g: u8, b: u8) -> Color {
    paint_color(Color::Rgb(r, g, b))
}

/// Detect colour capability from the process environment.
///
/// Precedence: `WHYCODES_COLOR` override, then `TERM_PROGRAM=Apple_Terminal`
/// (always 256 — Terminal.app maps `38;2` to the profile default / ANSI green),
/// then `COLORTERM=truecolor|24bit`, then Windows-host hints (`WT_SESSION`,
/// ConEmu, Win10+ conhost — they honour `38;2` with empty `TERM`), then `TERM`.
pub fn detect_color_mode() -> ColorMode {
    color_mode_from_env(
        std::env::var("WHYCODES_COLOR").ok().as_deref(),
        std::env::var("COLORTERM").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
        windows_truecolor_host(),
    )
}

/// Windows Terminal / ConEmu / Win10+ conhost honour `38;2` even when
/// `TERM` and `COLORTERM` are unset (the usual PowerShell profile).
fn windows_truecolor_host() -> bool {
    windows_truecolor_from_env(
        std::env::var_os("WT_SESSION").is_some(),
        std::env::var("ConEmuANSI").ok().as_deref(),
        cfg!(windows),
    )
}

fn windows_truecolor_from_env(
    wt_session: bool,
    conemu_ansi: Option<&str>,
    is_windows: bool,
) -> bool {
    if wt_session {
        return true;
    }
    if conemu_ansi.is_some_and(|v| v.trim().eq_ignore_ascii_case("ON")) {
        return true;
    }
    is_windows
}

pub fn color_mode_from_env(
    override_var: Option<&str>,
    colorterm: Option<&str>,
    term_program: Option<&str>,
    term: Option<&str>,
    windows_truecolor: bool,
) -> ColorMode {
    if let Some(raw) = override_var {
        let v = raw.trim();
        if !v.is_empty() {
            match v.to_ascii_lowercase().as_str() {
                "truecolor" | "24bit" | "24" => return ColorMode::TrueColor,
                "256" | "ansi256" => return ColorMode::Ansi256,
                "16" | "ansi" | "ansi16" => return ColorMode::Ansi16,
                _ => {}
            }
        }
    }

    // Terminal.app advertises xterm-256color but does not honour 38;2.
    // Check this before COLORTERM — some profiles leak a stale truecolor flag.
    if term_program.is_some_and(|p| p == "Apple_Terminal") {
        return ColorMode::Ansi256;
    }

    if let Some(ct) = colorterm {
        let ct = ct.trim().to_ascii_lowercase();
        if ct == "truecolor" || ct == "24bit" {
            return ColorMode::TrueColor;
        }
    }

    // Windows Terminal leaves TERM/COLORTERM empty; classic conhost on Win10+
    // still speaks 24-bit VT. Do not fall through to Ansi16.
    if windows_truecolor {
        return ColorMode::TrueColor;
    }

    let term = term.unwrap_or("").trim();
    let term_l = term.to_ascii_lowercase();
    if term_l.contains("truecolor") || term_l.contains("24bit") || term_l.contains("direct") {
        return ColorMode::TrueColor;
    }
    if term_l.contains("256color") || term_l.contains("256colour") {
        return ColorMode::Ansi256;
    }
    if term.is_empty() || term_l == "dumb" || term_l == "unknown" {
        return ColorMode::Ansi16;
    }
    if term_l.contains("color") || term_l.contains("colour") {
        return ColorMode::Ansi16;
    }
    ColorMode::Ansi16
}

/// Map a ratatui colour onto `mode`. `Color::Rgb` becomes Indexed (or a 16-colour
/// index); named / Reset colours pass through.
pub fn quantize_color(c: Color, mode: ColorMode) -> Color {
    match mode {
        ColorMode::TrueColor => c,
        ColorMode::Ansi256 => match c {
            Color::Rgb(r, g, b) => Color::Indexed(rgb_to_indexed256(r, g, b)),
            other => other,
        },
        ColorMode::Ansi16 => match c {
            Color::Rgb(r, g, b) => Color::Indexed(rgb_to_ansi16(r, g, b)),
            Color::Indexed(i) if i > 15 => {
                let (r, g, b) = indexed_rgb(i);
                Color::Indexed(rgb_to_ansi16(r, g, b))
            }
            other => other,
        },
    }
}

/// Nearest xterm-256 index for `r,g,b` (cube + gray ramp + 16 system colours).
pub fn rgb_to_indexed256(r: u8, g: u8, b: u8) -> u8 {
    // Prefer the 6×6×6 cube + gray ramp so those indices round-trip; fall
    // back to the 16 system colours only when they are strictly closer.
    let mut best_i = 16u8;
    let mut best_d = u32::MAX;
    for i in 16..=255u8 {
        let (cr, cg, cb) = indexed_rgb(i);
        let d = dist2(r, g, b, cr, cg, cb);
        if d < best_d {
            best_d = d;
            best_i = i;
        }
    }
    for i in 0..=15u8 {
        let (cr, cg, cb) = indexed_rgb(i);
        let d = dist2(r, g, b, cr, cg, cb);
        if d < best_d {
            best_d = d;
            best_i = i;
        }
    }
    best_i
}

/// Nearest of the 16 system colours, returned as `0..=15`.
///
/// Chromatic RGB (agent greens, peach accent, …) must not collapse onto the
/// gray slots (0/7/8/15). Those pastels are closer in Euclidean distance to
/// silver than to green/cyan, which is why Windows 16-colour mode painted
/// the whole TUI black-and-white.
pub fn rgb_to_ansi16(r: u8, g: u8, b: u8) -> u8 {
    let chroma = r.max(g).max(b).saturating_sub(r.min(g).min(b));
    let skip_gray = chroma >= 32;
    let mut best_i = 0u8;
    let mut best_d = u32::MAX;
    for i in 0..=15u8 {
        if skip_gray && matches!(i, 0 | 7 | 8 | 15) {
            continue;
        }
        let (cr, cg, cb) = indexed_rgb(i);
        let d = dist2(r, g, b, cr, cg, cb);
        if d < best_d {
            best_d = d;
            best_i = i;
        }
    }
    best_i
}

fn dist2(r: u8, g: u8, b: u8, cr: u8, cg: u8, cb: u8) -> u32 {
    let dr = r as i32 - cr as i32;
    let dg = g as i32 - cg as i32;
    let db = b as i32 - cb as i32;
    (dr * dr + dg * dg + db * db) as u32
}

/// xterm 256-colour cube / gray ramp / system colours.
pub(crate) fn indexed_rgb(i: u8) -> (u8, u8, u8) {
    match i {
        16..=231 => {
            let n = i - 16;
            let r = n / 36;
            let g = (n % 36) / 6;
            let b = n % 6;
            let f = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
            (f(r), f(g), f(b))
        }
        232..=255 => {
            let v = (i - 232) * 10 + 8;
            (v, v, v)
        }
        _ => named_rgb(i),
    }
}

fn named_rgb(i: u8) -> (u8, u8, u8) {
    // Same sRGB as `theme::to_rgb` so tint math and the quantizer agree.
    match i {
        0 => (0, 0, 0),
        1 => (128, 0, 0),
        2 => (0, 128, 0),
        3 => (128, 128, 0),
        4 => (0, 0, 128),
        5 => (128, 0, 128),
        6 => (0, 128, 128),
        7 => (192, 192, 192),
        8 => (128, 128, 128),
        9 => (255, 0, 0),
        10 => (0, 255, 0),
        11 => (255, 255, 0),
        12 => (0, 0, 255),
        13 => (255, 0, 255),
        14 => (0, 255, 255),
        _ => (255, 255, 255),
    }
}

/// Crossterm backend that rewrites `Color::Rgb` to Indexed on draw.
pub struct QuantizingBackend<B> {
    pub(crate) inner: B,
    mode: ColorMode,
    /// Used when the inner backend cannot report a size (no TTY / ioctl
    /// ENOENT). Production consoles still prefer the inner size.
    size_fallback: Option<Size>,
}

impl<B> QuantizingBackend<B> {
    pub fn new(inner: B, mode: ColorMode) -> Self {
        Self {
            inner,
            mode,
            size_fallback: None,
        }
    }

    pub fn with_size_fallback(inner: B, mode: ColorMode, fallback: Size) -> Self {
        Self {
            inner,
            mode,
            size_fallback: Some(fallback),
        }
    }
}

impl<B: Write> Write for QuantizingBackend<B> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn quantize_cell(cell: &BufferCell, mode: ColorMode) -> BufferCell {
    let mut c = cell.clone();
    c.fg = quantize_color(c.fg, mode);
    c.bg = quantize_color(c.bg, mode);
    c.underline_color = quantize_color(c.underline_color, mode);
    c
}

impl<B: Backend> Backend for QuantizingBackend<B> {
    type Error = B::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a BufferCell)>,
    {
        if self.mode.is_truecolor() {
            return self.inner.draw(content);
        }
        let mode = self.mode;
        let owned: Vec<(u16, u16, BufferCell)> = content
            .map(|(x, y, cell)| (x, y, quantize_cell(cell, mode)))
            .collect();
        self.inner
            .draw(owned.iter().map(|(x, y, cell)| (*x, *y, cell)))
    }

    fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        match self.inner.get_cursor_position() {
            Ok(p) => Ok(p),
            Err(e) => match self.size_fallback {
                Some(_) => Ok(Position::ORIGIN),
                None => Err(e),
            },
        }
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        match self.inner.size() {
            Ok(s) if s.width > 0 && s.height > 0 => Ok(s),
            other => match self.size_fallback {
                Some(fb) => Ok(fb),
                None => other,
            },
        }
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        match self.inner.window_size() {
            Ok(s) if s.columns_rows.width > 0 && s.columns_rows.height > 0 => Ok(s),
            other => match self.size_fallback {
                Some(fb) => Ok(WindowSize {
                    columns_rows: fb,
                    pixels: Size {
                        width: 0,
                        height: 0,
                    },
                }),
                None => other,
            },
        }
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }
}

#[cfg(test)]
#[path = "color_tests.rs"]
mod tests;
