//! Incremental markdown for a growing assistant bubble.
//!
//! Grok Build's `StreamingMarkdownRenderer`: freeze output up to the last
//! [`whycode_format::markdown::last_checkpoint`] and only re-parse the tail.
//! A streamed reply is O(new bytes) per frame instead of O(whole message).

use std::sync::Arc;

use ratatui::text::Line;

use crate::theme::ThemePalette;
use crate::ui::markdown::render_with_width;

/// Frozen prefix + live tail for one width.
#[derive(Debug, Clone, Default)]
pub struct IncrementalMarkdown {
    frozen_bytes: usize,
    frozen_hash: u64,
    frozen: Arc<Vec<Line<'static>>>,
    width: Option<usize>,
}

impl IncrementalMarkdown {
    /// Render `text` at `width`, reusing every line before the last checkpoint.
    pub fn render(
        &mut self,
        text: &str,
        palette: &ThemePalette,
        width: Option<usize>,
    ) -> Vec<Line<'static>> {
        if self.width != width || !self.prefix_ok(text) {
            self.clear();
            self.width = width;
        }

        let cp = whycode_format::markdown::last_checkpoint(text);
        if cp > self.frozen_bytes && cp <= text.len() && text.is_char_boundary(cp) {
            let chunk = &text[self.frozen_bytes..cp];
            let mut lines = match Arc::try_unwrap(std::mem::take(&mut self.frozen)) {
                Ok(v) => v,
                Err(shared) => (*shared).clone(),
            };
            if !chunk.is_empty() {
                lines.extend(render_with_width(chunk, palette, width));
            }
            self.frozen = Arc::new(lines);
            self.frozen_bytes = cp;
            self.frozen_hash = fnv1a(&text[..cp]);
        }

        let mut out = (*self.frozen).clone();
        if self.frozen_bytes < text.len() && text.is_char_boundary(self.frozen_bytes) {
            out.extend(render_with_width(
                &text[self.frozen_bytes..],
                palette,
                width,
            ));
        }
        out
    }

    fn prefix_ok(&self, text: &str) -> bool {
        if self.frozen_bytes == 0 {
            return true;
        }
        text.len() >= self.frozen_bytes
            && text.is_char_boundary(self.frozen_bytes)
            && fnv1a(&text[..self.frozen_bytes]) == self.frozen_hash
    }

    fn clear(&mut self) {
        self.frozen_bytes = 0;
        self.frozen_hash = 0;
        self.frozen = Arc::new(Vec::new());
    }
}

fn fnv1a(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325;
    for &b in s.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TuiAppConfig;

    fn palette() -> ThemePalette {
        TuiAppConfig::default().palette()
    }

    fn line_text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn incremental_matches_full_render_across_checkpoints() {
        let mut inc = IncrementalMarkdown::default();
        let palette = palette();
        let width = Some(40usize);
        let mut acc = String::new();
        for chunk in [
            "Here is **setup**.\n\n",
            "```rust\nfn main() {\n",
            "    println!(\"hi\");\n",
            "}\n```\n\n",
            "Done.\n",
        ] {
            acc.push_str(chunk);
            let got = inc.render(&acc, &palette, width);
            let full = render_with_width(&acc, &palette, width);
            assert_eq!(line_text(&got), line_text(&full), "mismatch after {acc:?}");
        }
        assert!(
            inc.frozen_bytes > 0,
            "closed fence + blank must freeze a prefix"
        );
    }

    #[test]
    fn open_line_does_not_freeze() {
        let mut inc = IncrementalMarkdown::default();
        let palette = palette();
        let _ = inc.render("partial", &palette, Some(40));
        assert_eq!(inc.frozen_bytes, 0);
    }
}
