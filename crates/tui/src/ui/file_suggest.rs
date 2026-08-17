// ── ui/file_suggest.rs: `@file` picker over the workspace index ─────────
// Opens on `@` (any word boundary) or Ctrl+Space. Queries the resident
// whycode-index — keystroke filtering never touches the filesystem.
// Layout mirrors the slash dropdown: panel wash, hairlines, selected row
// wash, solid scrollbar; matched characters render in the accent colour.

use std::sync::Arc;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Clear;
use ratatui::{Frame, buffer::Buffer};
use whycode_index::{FileMatch, ScanStatus, WorkspaceIndex};

use crate::app::TuiApp;
use crate::frecency::Frecency;
use crate::theme::ThemePalette;
use crate::ui::scrollbar::{paint_scrollbar, scroll_center};
use crate::ui::slash_suggest::{DropdownColors, elevate, fill_bg, set_line, truncate_to};

/// Columns reserved for gap + scrollbar track when content overflows.
const SCROLLBAR_GUTTER: u16 = 2;

/// Visible rows (excluding hairlines).
const MAX_ROWS: u16 = 8;
/// Matches requested from the index per keystroke (scroll headroom).
const QUERY_LIMIT: usize = 64;

/// State of the `@file` completion popup.
#[derive(Default)]
pub struct FileSuggestState {
    pub active: bool,
    /// Byte offset of the `@` that opened the current token.
    pub token_start: usize,
    /// Current query text (token text after `@`).
    pub query: String,
    pub matches: Vec<FileMatch>,
    pub selected: usize,
    /// Mouse-hovered match index, sticky for paint.
    pub hovered: Option<usize>,
    /// Absolute screen rect of the item list body (for hover hit-test).
    pub list_hit: Option<Rect>,
    /// First visible match index when scrolled (paint meta).
    pub list_scroll_start: usize,
    index: Option<Arc<WorkspaceIndex>>,
    /// Frecency table for the project (loaded with the index).
    frecency: Option<Frecency>,
    /// A fuzzy query was issued and no settled results were consumed since.
    /// Keeps the run loop on a short poll cadence until workers publish;
    /// without it a late publish could wait out a 500 ms idle poll.
    results_pending: bool,
    /// `matching()` has reported work since the last `refresh`. An empty
    /// first poll must not clear `results_pending`: nucleo can sit idle
    /// for a tick before workers start, and a last-chance read of that
    /// empty snapshot used to lock the picker on `matches=[]`.
    rematch_seen: bool,
}

/// Find the `@token` covering `cursor`: a maximal run of non-terminator
/// chars ending at or after `cursor` that starts with `@` at a word
/// boundary. Returns `(at_offset, token_end)` in bytes.
fn at_token(buffer: &str, cursor: usize) -> Option<(usize, usize)> {
    let cursor = cursor.min(buffer.len());
    let is_term = |c: char| c.is_whitespace() || c == ',' || c == ';';
    // Walk back to the start of the current run.
    let mut start = cursor;
    for (i, c) in buffer[..cursor].char_indices().rev() {
        if is_term(c) {
            break;
        }
        start = i;
    }
    // The run must begin with '@' at a word boundary.
    if !buffer[start..].starts_with('@') {
        return None;
    }
    if start > 0 {
        let prev = buffer[..start].chars().next_back()?;
        if !is_term(prev) {
            return None; // e.g. email addresses: user@host
        }
    }
    // Token end: next terminator after the '@'.
    let end = buffer[start + 1..]
        .find(is_term)
        .map(|i| start + 1 + i)
        .unwrap_or(buffer.len());
    if cursor > end {
        return None; // cursor sits past this token already
    }
    Some((start, end))
}

impl FileSuggestState {
    pub fn set_index(&mut self, index: Arc<WorkspaceIndex>) {
        // Frecency is keyed by the canonical primary root — same project,
        // same habits, regardless of the directory the user launched from.
        if self.frecency.is_none() && !index.roots().is_empty() {
            self.frecency = Some(Frecency::load(index.primary_root()));
        }
        self.index = Some(index);
    }

    /// Index scan status for the popup title (None when no index is set).
    pub fn scan_status(&self) -> Option<ScanStatus> {
        self.index.as_ref().map(|i| i.status())
    }

    /// Display label for a non-primary root (matches keep provenance).
    pub fn root_label(&self, root: u16) -> Option<String> {
        if root == 0 {
            return None;
        }
        let index = self.index.as_ref()?;
        index
            .roots()
            .get(root as usize)
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
    }

    /// Apply frecency boosts and re-sort (best first). Associated fn so the
    /// call sites can split-borrow `self.frecency` and `self.matches`.
    fn apply_boosts(frecency: Option<&Frecency>, matches: &mut [FileMatch]) {
        let Some(fr) = frecency else {
            return;
        };
        for m in matches.iter_mut() {
            m.score = m.score.saturating_add(fr.boost(&m.rel));
        }
        matches.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.rel.len().cmp(&b.rel.len()))
        });
    }

    /// Ctrl+Space: ensure an `@` token exists at the cursor, then open.
    pub fn activate(&mut self, buffer: &mut String, cursor: &mut usize) {
        if at_token(buffer, *cursor).is_none() {
            let at = (*cursor).min(buffer.len());
            // Keep the token standalone: after a non-space char, insert " @".
            let needs_space = at > 0
                && buffer[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| !c.is_whitespace());
            let text = if needs_space { " @" } else { "@" };
            buffer.insert_str(at, text);
            *cursor = at + text.len();
        }
        self.refresh(buffer, *cursor);
        self.active = true;
    }

    /// Recompute state from the prompt buffer. Auto-activates while the
    /// cursor sits inside an `@token`; auto-dismisses otherwise.
    pub fn refresh(&mut self, buffer: &str, cursor: usize) {
        let Some((start, end)) = at_token(buffer, cursor) else {
            if self.active {
                self.dismiss();
            }
            return;
        };
        let query = &buffer[start + 1..end];
        if query == self.query && self.active {
            return; // cursor moved inside the token; nothing to do
        }
        self.token_start = start;
        self.query.clear();
        self.query.push_str(query);
        let Some(index) = &self.index else {
            self.matches.clear();
            self.active = true; // still open: render shows "index unavailable"
            return;
        };
        // Non-blocking: workers rematch in the background; poll_matches()
        // picks up fresh results via the dirty flag (no frame ever blocks
        // on a full rematch and no stale-empty flash on big trees).
        self.matches = index.query_now(&self.query, QUERY_LIMIT);
        Self::apply_boosts(self.frecency.as_ref(), &mut self.matches);
        self.selected = 0;
        // Browse forms are store-backed (complete instantly); only fuzzy
        // queries need the short-poll window.
        self.results_pending = !self.query.is_empty() && !self.query.ends_with('/');
        self.rematch_seen = false;
        self.active = true;
    }

    /// Run-loop hook: adopt freshly published matcher results. Returns true
    /// when the visible list changed (caller should mark the frame dirty).
    pub fn poll_matches(&mut self) -> bool {
        if !self.active {
            return false;
        }
        let Some(index) = &self.index else {
            return false;
        };
        let dirty = index.take_results_dirty();
        // nucleo only publishes a snapshot on `tick`. `matching()` is that
        // tick. Waiting on the dirty flag alone can stall the picker empty
        // until the next keystroke: `tick(0)` sets should_notify=false, a
        // just-finished worker then skips notify, and the UI never ticks
        // again. Seen as `picker_flow_over_real_index` under parallel CI.
        let running = index.matching();
        if running {
            self.rematch_seen = true;
        }
        if !dirty && !self.results_pending {
            return false;
        }
        if !dirty && running {
            return false; // rematch in flight; keep the last non-empty list
        }

        let before = self.matches.clone();
        self.matches = index.read_matches(QUERY_LIMIT);
        Self::apply_boosts(self.frecency.as_ref(), &mut self.matches);
        if self.selected >= self.matches.len() {
            self.selected = 0;
        }
        // Do not clear pending on an empty first settle — workers may not
        // have started yet. Wait until we saw a rematch or got hits.
        if !index.matching() && (self.rematch_seen || !self.matches.is_empty()) {
            self.results_pending = false;
        }
        before.len() != self.matches.len()
            || before
                .iter()
                .zip(self.matches.iter())
                .any(|(a, b)| a.rel != b.rel || a.score != b.score || a.is_dir != b.is_dir)
    }

    /// True while fresh matches may still arrive — the run loop keeps a
    /// short poll cadence and keeps repainting during that window.
    pub fn awaiting_matches(&self) -> bool {
        if !self.active {
            return false;
        }
        let Some(index) = &self.index else {
            return false;
        };
        self.results_pending || index.matching()
    }

    pub fn dismiss(&mut self) {
        self.active = false;
        self.query.clear();
        self.matches.clear();
        self.selected = 0;
        self.hovered = None;
        self.list_hit = None;
        self.list_scroll_start = 0;
        self.results_pending = false;
        self.rematch_seen = false;
    }

    pub fn step(&mut self, delta: isize) {
        if self.matches.is_empty() {
            return;
        }
        let len = self.matches.len() as isize;
        let cur = self.selected as isize;
        self.selected = ((cur + delta).rem_euclid(len)) as usize;
    }

    pub fn current(&self) -> Option<&FileMatch> {
        self.matches.get(self.selected)
    }

    /// Replace the current `@token` with the selection.
    ///
    /// Files complete to `@path ` (space ends the token → popup closes).
    /// Dirs complete to `@dir/` and the popup stays open, drilling down.
    /// Returns true when the popup should stay open.
    pub fn accept(&mut self, buffer: &mut String, cursor: &mut usize) -> bool {
        let Some(m) = self.current().cloned() else {
            return false;
        };
        let end = buffer[self.token_start..]
            .find(|c: char| c.is_whitespace() || c == ',' || c == ';')
            .map(|i| self.token_start + i)
            .unwrap_or(buffer.len());
        let replacement = if m.is_dir {
            format!("@{}/", m.rel)
        } else {
            format!("@{} ", m.rel)
        };
        buffer.replace_range(self.token_start..end, &replacement);
        *cursor = self.token_start + replacement.len();
        if m.is_dir {
            self.refresh(buffer, *cursor);
            true
        } else {
            // A file pick is a habit signal: frecency boosts it next time.
            if let Some(fr) = &mut self.frecency {
                fr.record(&m.rel);
            }
            self.dismiss();
            false
        }
    }

    /// Match index under the pointer (uses last paint's list hit rect).
    pub fn row_index_at(&self, col: u16, row: u16) -> Option<usize> {
        let hit = self.list_hit?;
        if col < hit.x
            || col >= hit.x.saturating_add(hit.width)
            || row < hit.y
            || row >= hit.y.saturating_add(hit.height)
        {
            return None;
        }
        let idx = self.list_scroll_start + (row - hit.y) as usize;
        (idx < self.matches.len()).then_some(idx)
    }
}

// ── render ──────────────────────────────────────────────────────────────

/// Render the picker just above the prompt (same anchor as the slash menu).
pub fn render(frame: &mut Frame, prompt_area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    if !app.file_suggest.active {
        app.file_suggest.list_hit = None;
        app.file_suggest.hovered = None;
        return;
    }
    let colors = DropdownColors::from_palette(palette);
    let total = app.file_suggest.matches.len();
    // One status row when empty ("no matches" / "scanning…") so the popup
    // stays legible instead of flickering closed mid-scan.
    let rows = (total.max(1) as u16).min(MAX_ROWS);
    let height = rows + 2; // hairline + items + hairline
    if prompt_area.y < height || prompt_area.width < 8 {
        app.file_suggest.list_hit = None;
        return;
    }
    let area = Rect {
        x: prompt_area.x,
        y: prompt_area.y.saturating_sub(height),
        width: prompt_area.width,
        height,
    };
    frame.render_widget(Clear, area);

    let visible = rows as usize;
    let needs_scrollbar = total > visible;
    let start = scroll_center(app.file_suggest.selected, total.max(1), visible);
    app.file_suggest.list_scroll_start = start;

    let buf = frame.buffer_mut();
    fill_bg(buf, area, colors.panel);

    // Hairlines; right-aligned status: count, or live scan progress.
    let rule = "─".repeat(area.width as usize);
    let rule_style = Style::default().fg(colors.chrome_fg).bg(colors.chrome);
    set_line(buf, area.x, area.y, &rule, area.width, rule_style);
    set_line(
        buf,
        area.x,
        area.y + height - 1,
        &rule,
        area.width,
        rule_style,
    );
    let status_text = match app.file_suggest.scan_status() {
        Some(ScanStatus::Scanning { scanned }) => format!("scanning {scanned}…"),
        Some(ScanStatus::Ready { truncated, .. }) if truncated => format!("{total} (capped)"),
        _ => format!("{total}"),
    };
    let sw = status_text.len() as u16;
    if sw + 2 <= area.width {
        set_line(
            buf,
            area.x + area.width - sw - 1,
            area.y,
            &status_text,
            sw,
            Style::default().fg(colors.hint).bg(colors.chrome),
        );
    }
    // Left title on the top hairline.
    set_line(
        buf,
        area.x + 1,
        area.y,
        "files",
        5,
        Style::default().fg(colors.hint).bg(colors.chrome),
    );

    let items_area = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: rows,
    };
    app.file_suggest.list_hit = Some(Rect {
        x: items_area.x,
        y: items_area.y,
        width: items_area
            .width
            .saturating_sub(if needs_scrollbar { SCROLLBAR_GUTTER } else { 0 }),
        height: items_area.height,
    });
    let content_w = if needs_scrollbar {
        items_area.width.saturating_sub(SCROLLBAR_GUTTER)
    } else {
        items_area.width
    };
    let hover_bg = elevate(palette.bg, 36);

    if total == 0 {
        let msg = match app.file_suggest.scan_status() {
            Some(ScanStatus::Scanning { .. }) => "  scanning project files…",
            None => "  file index unavailable",
            _ => "  no matches",
        };
        let line = Line::from(Span::styled(
            truncate_to(msg, content_w as usize),
            Style::default().fg(colors.hint).bg(colors.panel),
        ));
        let _ = buf.set_line(items_area.x, items_area.y, &line, content_w);
    }

    for vis_row in 0..visible {
        let item_idx = start + vis_row;
        if item_idx >= total {
            break;
        }
        let m = app.file_suggest.matches[item_idx].clone();
        let root_tag = app.file_suggest.root_label(m.root);
        let selected = item_idx == app.file_suggest.selected;
        let mouse_hover = app.file_suggest.hovered == Some(item_idx) && !selected;
        let y = items_area.y + vis_row as u16;
        let row_bg = if selected {
            colors.selected
        } else if mouse_hover {
            hover_bg
        } else {
            colors.panel
        };
        fill_bg(
            buf,
            Rect {
                x: items_area.x,
                y,
                width: content_w,
                height: 1,
            },
            row_bg,
        );
        paint_row(
            buf,
            items_area.x,
            y,
            content_w,
            &m,
            root_tag,
            selected,
            row_bg,
            &colors,
            palette,
        );
    }

    if needs_scrollbar {
        let sb = Rect {
            x: items_area.x + items_area.width.saturating_sub(1),
            y: items_area.y,
            width: 1,
            height: items_area.height,
        };
        paint_scrollbar(buf, sb, total, visible, start, colors.track, colors.thumb);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_row(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    m: &FileMatch,
    root_tag: Option<String>,
    selected: bool,
    row_bg: Color,
    colors: &DropdownColors,
    palette: &ThemePalette,
) {
    if width == 0 {
        return;
    }
    let name_style = Style::default()
        .fg(if selected {
            colors.name_selected
        } else {
            colors.name
        })
        .bg(row_bg)
        .add_modifier(if selected {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
    let hit_style = Style::default()
        .fg(palette.accent)
        .bg(row_bg)
        .add_modifier(Modifier::BOLD);
    let dim_style = Style::default().fg(colors.hint).bg(row_bg);
    let pad_style = Style::default().bg(row_bg);

    let prefix = if selected { "❯ " } else { "  " };
    let mut spans = vec![Span::styled(prefix, name_style)];
    let mut used = 2usize;

    // Path with matched characters highlighted (nucleo indices are char
    // positions into the displayed column: rel + optional trailing '/').
    let display = if m.is_dir {
        format!("{}/", m.rel)
    } else {
        m.rel.clone()
    };
    for (ci, ch) in display.chars().enumerate() {
        let hit = m.indices.contains(&(ci as u32));
        spans.push(Span::styled(
            ch.to_string(),
            if hit { hit_style } else { name_style },
        ));
        used += 1;
    }

    // External roots get a dim source tag so matches stay attributable.
    if let Some(label) = root_tag {
        let tag = format!("  {label}");
        spans.push(Span::styled(tag.clone(), dim_style));
        used += tag.chars().count();
    }

    if (width as usize) > used {
        spans.push(Span::styled(" ".repeat(width as usize - used), pad_style));
    }
    let line = Line::from(spans);
    let _ = buf.set_line(x, y, &line, width);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_token_finds_mentions() {
        assert_eq!(at_token("@main", 5), Some((0, 5)));
        assert_eq!(at_token("fix @src/ now", 9), Some((4, 9)));
        assert_eq!(at_token("fix @src/ now", 11), None); // cursor past token
        assert_eq!(at_token("mail bob@corp.io", 12), None); // email, not a mention
        assert_eq!(at_token("no mention", 5), None);
        assert_eq!(at_token("@", 1), Some((0, 1)));
        assert_eq!(at_token("a@", 2), None); // glued to a word → not a mention
        assert_eq!(at_token(" @", 2), Some((1, 2))); // space-separated → mention
    }

    /// Picker over a real index: activate → fuzzy → drill down → accept.
    #[test]
    fn picker_flow_over_real_index() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(tmp.path().join("src/lib.rs"), "// lib").unwrap();
        std::fs::write(tmp.path().join("README.md"), "hi").unwrap();

        let idx = WorkspaceIndex::start_with(
            vec![tmp.path().to_path_buf()],
            whycode_index::IndexOptions {
                watch: false,
                threads: 1,
                ..Default::default()
            },
        );
        assert!(idx.wait_ready(std::time::Duration::from_secs(10)));
        // Store-backed: if this fails the walk ignored the fixture (not a
        // fuzzy-poll flake).
        assert!(
            idx.entries().iter().any(|e| &*e.rel == "src/main.rs"),
            "scan missed src/main.rs; status={:?} entries={:?}",
            idx.status(),
            idx.entries()
                .iter()
                .map(|e| e.rel.as_ref())
                .collect::<Vec<_>>(),
        );

        let mut st = FileSuggestState::default();
        st.set_index(idx);

        // Matching is async: poll (like the run loop does) until it lands.
        fn poll_until(st: &mut FileSuggestState, pred: impl Fn(&FileSuggestState) -> bool) -> bool {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while std::time::Instant::now() < deadline {
                if pred(st) {
                    return true;
                }
                st.poll_matches();
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            pred(st)
        }

        // Type "@mai" — picker opens and fuzzy-finds main.rs.
        let mut buf = String::from("@mai");
        let mut cur = buf.len();
        st.refresh(&buf, cur);
        assert!(st.active);
        assert!(
            poll_until(&mut st, |s| s
                .matches
                .iter()
                .any(|m| m.rel == "src/main.rs")),
            "picker never saw src/main.rs; matches={:?} status={:?}",
            st.matches
                .iter()
                .map(|m| m.rel.as_str())
                .collect::<Vec<_>>(),
            st.scan_status(),
        );

        // Accept the file → token replaced with @path + trailing space, closed.
        while !st.current().is_some_and(|m| m.rel == "src/main.rs") {
            st.step(1);
        }
        let open = st.accept(&mut buf, &mut cur);
        assert!(!open);
        assert_eq!(buf, "@src/main.rs ");
        assert!(!st.active);

        // "@s" → drill into src/ with Tab-style accept, picker stays open.
        let mut buf = String::from("@s");
        let mut cur = 2;
        st.refresh(&buf, cur);
        assert!(st.active);
        assert!(poll_until(&mut st, |s| s
            .matches
            .iter()
            .any(|m| m.is_dir && m.rel == "src")));
        while !st.current().is_some_and(|m| m.is_dir && m.rel == "src") {
            st.step(1);
        }
        let open = st.accept(&mut buf, &mut cur);
        assert!(open);
        assert_eq!(buf, "@src/");
        // Now browsing inside src/ — main.rs and lib.rs listed (browse is
        // store-backed, so visible immediately; poll once for safety).
        assert!(poll_until(&mut st, |s| s
            .matches
            .iter()
            .any(|m| m.rel == "src/main.rs")
            && s.matches.iter().any(|m| m.rel == "src/lib.rs")));
    }

    #[test]
    fn frecency_lifts_picked_files() {
        let mut st = FileSuggestState {
            frecency: Some(Frecency::ephemeral()),
            ..Default::default()
        };
        let mut matches = vec![
            FileMatch {
                rel: "src/main.rs".into(),
                score: 100,
                ..Default::default()
            },
            FileMatch {
                rel: "src/mail.rs".into(),
                score: 110,
                ..Default::default()
            },
        ];
        st.frecency.as_mut().unwrap().record("src/main.rs");
        FileSuggestState::apply_boosts(st.frecency.as_ref(), &mut matches);
        assert_eq!(matches[0].rel, "src/main.rs"); // boosted past the raw winner
        assert!(matches[0].score > 110);
    }
}
