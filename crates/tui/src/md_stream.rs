//! Incremental markdown for a growing assistant bubble.
//!
//! Grok Build's `StreamingMarkdownRenderer`: freeze output up to the last
//! [`whycodes_format::markdown::last_checkpoint`] and only re-parse the tail.
//! A streamed reply is O(new bytes) per frame instead of O(whole message).
//!
//! Open fenced code is a special tail: `last_checkpoint` cannot freeze inside
//! a fence (the closer has not arrived), so a naive re-parse would rebuild
//! every highlighted `Line` on every token. The open-fence painter keeps
//! committed rows in `buf` and only wraps new source lines.

use std::hash::{Hash, Hasher};

use ratatui::text::Line;
use rustc_hash::FxHasher;

use crate::theme::ThemePalette;
use crate::ui::markdown::{append_open_fence, code_gutter_nw, open_fence_tail, render_with_width};

/// Frozen prefix + live tail for one width.
#[derive(Debug, Clone, Default)]
pub struct IncrementalMarkdown {
    frozen_bytes: usize,
    frozen_hash: u64,
    /// Painted lines: `[frozen markdown | committed fence | partial+pad]`.
    buf: Vec<Line<'static>>,
    /// `buf[..frozen_len]` is stable markdown before the live tail.
    frozen_len: usize,
    width: Option<usize>,
    /// Complete source lines already painted for the open fence tail.
    fence_src: usize,
    /// `buf[frozen_len .. frozen_len+fence_display]` is committed fence chrome
    /// + complete rows (no partial line, no closing pad).
    fence_display: usize,
    fence_nw: usize,
}

impl IncrementalMarkdown {
    /// Current painted lines. Call [`render`] first to refresh.
    pub fn lines(&self) -> &[Line<'static>] {
        &self.buf
    }

    /// Mutable view for paint-time overlays (clock on the first content row).
    pub fn lines_mut(&mut self) -> &mut [Line<'static>] {
        &mut self.buf
    }

    /// Render `text` at `width`, reusing every line before the last checkpoint.
    ///
    /// Returns a slice of the internal buffer — no clone of the frozen prefix.
    pub fn render(
        &mut self,
        text: &str,
        palette: &ThemePalette,
        width: Option<usize>,
    ) -> &[Line<'static>] {
        if self.width != width || !self.prefix_ok(text) {
            self.clear();
            self.width = width;
        }

        let cp = whycodes_format::markdown::last_checkpoint(text);
        if cp > self.frozen_bytes && cp <= text.len() && text.is_char_boundary(cp) {
            // Checkpoint advanced: the previous open fence (if any) is now
            // closed inside `chunk`. Drop the incremental fence rows and
            // paint the frozen chunk once.
            self.buf.truncate(self.frozen_len);
            self.fence_src = 0;
            self.fence_display = 0;
            self.fence_nw = 0;
            let chunk = &text[self.frozen_bytes..cp];
            if !chunk.is_empty() {
                self.buf.extend(render_with_width(chunk, palette, width));
            }
            self.frozen_len = self.buf.len();
            self.frozen_bytes = cp;
            self.frozen_hash = hash_prefix(&text[..cp]);
        }

        if self.frozen_bytes < text.len() && text.is_char_boundary(self.frozen_bytes) {
            let tail = &text[self.frozen_bytes..];
            self.render_tail(tail, palette, width);
        } else {
            self.buf.truncate(self.frozen_len);
            self.fence_src = 0;
            self.fence_display = 0;
            self.fence_nw = 0;
        }
        &self.buf
    }

    fn render_tail(&mut self, tail: &str, palette: &ThemePalette, width: Option<usize>) {
        if let Some((lang, body)) = open_fence_tail(tail) {
            let nw = code_gutter_nw(body);
            if nw != self.fence_nw && self.fence_src > 0 {
                self.fence_src = 0;
                self.fence_display = 0;
            }
            self.fence_nw = nw;
            if self.fence_src == 0 {
                self.buf.truncate(self.frozen_len);
            } else {
                self.buf.truncate(self.frozen_len + self.fence_display);
            }
            let (src, display) =
                append_open_fence(&mut self.buf, lang, body, palette, width, self.fence_src);
            self.fence_src = src;
            self.fence_display = display.saturating_sub(self.frozen_len);
            return;
        }
        self.buf.truncate(self.frozen_len);
        self.fence_src = 0;
        self.fence_display = 0;
        self.fence_nw = 0;
        self.buf.extend(render_with_width(tail, palette, width));
    }

    fn prefix_ok(&self, text: &str) -> bool {
        if self.frozen_bytes == 0 {
            return true;
        }
        text.len() >= self.frozen_bytes
            && text.is_char_boundary(self.frozen_bytes)
            && hash_prefix(&text[..self.frozen_bytes]) == self.frozen_hash
    }

    fn clear(&mut self) {
        self.frozen_bytes = 0;
        self.frozen_hash = 0;
        self.buf.clear();
        self.frozen_len = 0;
        self.fence_src = 0;
        self.fence_display = 0;
        self.fence_nw = 0;
    }
}

fn hash_prefix(s: &str) -> u64 {
    let mut hasher = FxHasher::default();
    s.hash(&mut hasher);
    // Hash length as well so different-length prefixes with same content hash
    // (theoretical Fx collision) cannot be confused; matches issue C1 spec.
    s.len().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
#[path = "md_stream_tests.rs"]
mod tests;
