// ── app.rs: Main application state ────────────────────────────────────
// TuiApp holds all mutable state for the TUI application, including
// the focused mode, dialog stack, session messages, input buffer,
// sidebar visibility, theme, and keybinding context.

use crate::keymap::KeymapContext;
use crate::theme::ThemeName;
use ratatui::layout::Rect;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;
use whycode_tools::question::{QuestionAnswer, QuestionSpec};

// ── Application Modes ──────────────────────────────────────────────────
/// Top-level application mode.  Mutually exclusive — only one active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    /// Normal interaction: chat + prompt visible, no overlay.
    Normal,
    /// Agent is actively generating a response.
    Session,
    /// Command palette (or Vim-style `:` prompt).
    Command,
    /// A modal dialog is open (see `DialogManager`).
    Dialog,
    /// Help / keybinding cheatsheet overlay.
    Help,
}

// ── Dialog Manager ─────────────────────────────────────────────────────
/// Centralized dialog stack. Only one dialog is active at a time.
#[derive(Debug, Clone)]
pub enum DialogKind {
    Provider,
    Model,
    /// Primary-agent picker (prompt footer click / `/agent` with no args).
    Agent,
    Help,
    Alert {
        title: String,
        message: String,
    },
    Confirm {
        title: String,
        message: String,
        on_confirm: ConfirmAction,
    },
    /// Tool permission prompt (y/n)
    Permission {
        tool_name: String,
        detail: String,
    },
    /// Grok-style interactive questionnaire (`question` tool).
    Question(QuestionDialogState),
    SessionList,
    /// Live multi-session dashboard (S2/S3): grouped rows, peek, attach.
    Sessions,
    Status,
    Theme,
    Workspace,
    /// OAuth sign-in picker (`/login`): one row per subscription provider.
    Login,
    /// OpenAI-compat / xAI reasoning effort (`low`/`medium`/`high`/`xhigh`).
    Effort,
}

/// One row in the `/login` provider picker.
#[derive(Debug, Clone)]
pub struct LoginProviderRow {
    /// Provider id (`anthropic`, `openai`, …) passed to the OAuth flow.
    pub provider: String,
    /// Human label from the provider spec ("Anthropic (Claude Pro/Max)").
    pub label: String,
    /// A credential already sits in the token store.
    pub connected: bool,
}

/// State of the `/login` OAuth provider picker.
#[derive(Debug, Clone, Default)]
pub struct LoginDialogState {
    pub selected: usize,
    pub rows: Vec<LoginProviderRow>,
}

/// Live state for an open questionnaire modal.
#[derive(Debug, Clone)]
pub struct QuestionDialogState {
    pub questions: Vec<QuestionSpec>,
    /// Answers filled so far (length = questions.len(), None until answered).
    pub answers: Vec<Option<QuestionAnswer>>,
    /// Index of the question currently shown.
    pub index: usize,
    /// Cursor among options + trailing Other.
    pub cursor: usize,
    /// Multi-select toggles for predefined options (not Other).
    pub multi_selected: HashSet<usize>,
    pub free_text: String,
    pub free_text_focus: bool,
    /// Set when the user dismisses without answering (TUI run loop completes).
    pub cancelled: bool,
}

impl QuestionDialogState {
    pub fn new(questions: Vec<QuestionSpec>) -> Self {
        let n = questions.len();
        let free_focus = questions
            .first()
            .map(|q| q.options.is_empty())
            .unwrap_or(true);
        Self {
            questions,
            answers: vec![None; n],
            index: 0,
            cursor: 0,
            multi_selected: HashSet::new(),
            free_text: String::new(),
            free_text_focus: free_focus,
            cancelled: false,
        }
    }

    pub fn current(&self) -> Option<&QuestionSpec> {
        self.questions.get(self.index)
    }

    /// Number of rows: real options + Other (always).
    pub fn option_count(&self) -> usize {
        self.current().map(|q| q.options.len() + 1).unwrap_or(1)
    }

    pub fn is_other_index(&self, i: usize) -> bool {
        self.current().map(|q| i >= q.options.len()).unwrap_or(true)
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let n = self.option_count() as isize;
        if n <= 0 {
            return;
        }
        let mut c = self.cursor as isize + delta;
        while c < 0 {
            c += n;
        }
        self.cursor = (c % n) as usize;
        // Auto-focus free text when landing on Other or empty options.
        if self.is_other_index(self.cursor)
            || self
                .current()
                .map(|q| q.options.is_empty())
                .unwrap_or(false)
        {
            // Keep free_text_focus if already typing; otherwise select Other row.
        } else {
            self.free_text_focus = false;
        }
    }

    pub fn toggle_multi_at_cursor(&mut self) {
        if self.free_text_focus {
            return;
        }
        if self.is_other_index(self.cursor) {
            self.free_text_focus = true;
            return;
        }
        if self.multi_selected.contains(&self.cursor) {
            self.multi_selected.remove(&self.cursor);
        } else {
            self.multi_selected.insert(self.cursor);
        }
    }

    /// Confirm current question. Returns `Some(all_answers)` when questionnaire done.
    pub fn confirm_current(&mut self) -> Option<Vec<QuestionAnswer>> {
        let q = self.current()?.clone();
        let answer = if q.multi_select {
            let mut selected = Vec::new();
            for &i in &self.multi_selected {
                if i < q.options.len() {
                    selected.push(q.options[i].label.clone());
                }
            }
            let free = {
                let t = self.free_text.trim();
                if !t.is_empty()
                    && (self.is_other_index(self.cursor)
                        || self.free_text_focus
                        || selected.is_empty())
                {
                    Some(t.to_string())
                } else if self.is_other_index(self.cursor) && t.is_empty() {
                    // Other with no text — require text
                    return None;
                } else {
                    None
                }
            };
            if selected.is_empty() && free.is_none() {
                return None;
            }
            QuestionAnswer {
                selected,
                free_text: free,
            }
        } else if q.options.is_empty() || self.is_other_index(self.cursor) || self.free_text_focus {
            let t = self.free_text.trim();
            if t.is_empty() {
                // Enter on Other with empty text → focus free-text field
                self.free_text_focus = true;
                return None;
            }
            QuestionAnswer {
                selected: vec![],
                free_text: Some(t.to_string()),
            }
        } else {
            let label = q.options.get(self.cursor)?.label.clone();
            QuestionAnswer {
                selected: vec![label],
                free_text: None,
            }
        };

        if self.index < self.answers.len() {
            self.answers[self.index] = Some(answer);
        }
        if self.index + 1 >= self.questions.len() {
            // Prefer filled answers in order; require one per question.
            if self.answers.iter().all(|a| a.is_some()) {
                let done: Vec<QuestionAnswer> =
                    self.answers.iter().filter_map(|a| a.clone()).collect();
                return Some(done);
            }
            // Hole (navigated past unanswered): jump to first unanswered.
            if let Some(i) = self.answers.iter().position(|a| a.is_none()) {
                self.index = i;
                self.rehydrate_ui();
                return None;
            }
        }
        // Advance
        self.index += 1;
        self.rehydrate_ui();
        None
    }

    /// Jump to previous question (← / `[`). Keeps any already-saved answer.
    pub fn go_prev_question(&mut self) -> bool {
        if self.index == 0 {
            return false;
        }
        self.index -= 1;
        self.rehydrate_ui();
        true
    }

    /// Jump to next question when it already has an answer (→ / `]`).
    ///
    /// Forward-only navigation does not skip unanswered questions — use Enter
    /// to answer and advance.
    pub fn go_next_question(&mut self) -> bool {
        if self.index + 1 >= self.questions.len() {
            return false;
        }
        // Allow forward if current is answered, or target already has an answer.
        let can = self
            .answers
            .get(self.index)
            .and_then(|a| a.as_ref())
            .is_some()
            || self
                .answers
                .get(self.index + 1)
                .and_then(|a| a.as_ref())
                .is_some();
        if !can {
            return false;
        }
        self.index += 1;
        self.rehydrate_ui();
        true
    }

    /// Restore cursor / multi / free-text from a saved answer (if any).
    fn rehydrate_ui(&mut self) {
        self.cursor = 0;
        self.multi_selected.clear();
        self.free_text.clear();
        self.free_text_focus = self.current().map(|q| q.options.is_empty()).unwrap_or(true);

        let Some(q) = self.current().cloned() else {
            return;
        };
        let Some(ans) = self.answers.get(self.index).and_then(|a| a.clone()) else {
            return;
        };

        if let Some(ref t) = ans.free_text {
            self.free_text = t.clone();
            if ans.selected.is_empty() || q.options.is_empty() {
                self.cursor = q.options.len(); // Other
                self.free_text_focus = false;
            }
        }

        if q.multi_select {
            for label in &ans.selected {
                if let Some(i) = q.options.iter().position(|o| &o.label == label) {
                    self.multi_selected.insert(i);
                }
            }
            if !ans.selected.is_empty() {
                self.cursor = self.multi_selected.iter().copied().min().unwrap_or(0);
            }
        } else if let Some(label) = ans.selected.first()
            && let Some(i) = q.options.iter().position(|o| &o.label == label)
        {
            self.cursor = i;
        }
    }

    /// Plain-text dump of the full questionnaire for clipboard copy.
    pub fn clipboard_text(&self) -> String {
        let mut out = String::new();
        for (qi, q) in self.questions.iter().enumerate() {
            if self.questions.len() > 1 {
                out.push_str(&format!("Question {}/{}\n", qi + 1, self.questions.len()));
            }
            out.push_str(&q.prompt);
            out.push('\n');
            if q.options.is_empty() {
                out.push_str("(free-text)\n");
            } else {
                for (i, opt) in q.options.iter().enumerate() {
                    if opt.description.is_empty() {
                        out.push_str(&format!("  {}. {}\n", i + 1, opt.label));
                    } else {
                        out.push_str(&format!(
                            "  {}. {} — {}\n",
                            i + 1,
                            opt.label,
                            opt.description
                        ));
                    }
                }
                out.push_str(&format!("  {}. Other…\n", q.options.len() + 1));
            }
            if let Some(Some(a)) = self.answers.get(qi) {
                out.push_str(&format!("Answer: {}\n", a.summary()));
            } else if qi == self.index {
                out.push_str("(current)\n");
            }
            if qi + 1 < self.questions.len() {
                out.push('\n');
            }
        }
        out
    }

    /// Set option cursor by absolute index (mouse / digit).
    pub fn set_cursor(&mut self, idx: usize) {
        let n = self.option_count();
        if n == 0 {
            return;
        }
        self.cursor = idx.min(n - 1);
        if self.is_other_index(self.cursor)
            || self
                .current()
                .map(|q| q.options.is_empty())
                .unwrap_or(false)
        {
            // Stay on Other row; free-text focus opted-in by Space / o / Enter.
        } else {
            self.free_text_focus = false;
        }
    }
}

/// What to do when a confirmation dialog is accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    Quit,
    ClearSession,
    DeleteProvider(String),
}

#[derive(Debug, Clone)]
pub struct DialogManager {
    /// Stack of open dialogs; last item is the visible one.
    pub stack: Vec<DialogKind>,
}

impl Default for DialogManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DialogManager {
    pub fn new() -> Self {
        Self { stack: vec![] }
    }

    pub fn is_open(&self) -> bool {
        !self.stack.is_empty()
    }

    pub fn active(&self) -> Option<&DialogKind> {
        self.stack.last()
    }

    pub fn push(&mut self, dialog: DialogKind) {
        self.stack.push(dialog);
    }

    pub fn pop(&mut self) -> Option<DialogKind> {
        self.stack.pop()
    }

    pub fn clear(&mut self) {
        self.stack.clear();
    }
}

// ── Sidebar State ──────────────────────────────────────────────────────
#[derive(Debug, Clone)]
pub struct SidebarState {
    pub visible: bool,
    pub active_tab: SidebarTab,
    /// File tree entries (relative paths).
    pub file_tree: Vec<String>,
    /// LSP diagnostic count.
    pub diagnostics: usize,
    /// MCP server status messages.
    pub mcp_status: Vec<String>,
    /// Agent-pinned preview (file / diff / mermaid).
    pub preview: SidebarPreview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarTab {
    Files,
    Diagnostics,
    Mcp,
    Todos,
    Preview,
    Agents,
}

impl SidebarTab {
    pub const ALL: [Self; 6] = [
        Self::Files,
        Self::Diagnostics,
        Self::Mcp,
        Self::Todos,
        Self::Preview,
        Self::Agents,
    ];

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let i = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::Diagnostics => "Diag",
            Self::Mcp => "MCP",
            Self::Todos => "Todos",
            Self::Preview => "View",
            Self::Agents => "Agents",
        }
    }
}

/// What the Preview tab shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SidebarPreview {
    #[default]
    None,
    File {
        path: String,
        text: String,
    },
    Diff {
        path: String,
        unified: String,
    },
    Mermaid {
        source: String,
    },
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            visible: false,
            active_tab: SidebarTab::Files,
            file_tree: vec![],
            diagnostics: 0,
            mcp_status: vec![],
            preview: SidebarPreview::None,
        }
    }
}

// ── Provider Dialog State ──────────────────────────────────────────────
/// Two modes: select from list, or add custom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderDialogMode {
    Select,
    AddCustom,
}

#[derive(Debug, Clone)]
pub struct ProviderDialogState {
    pub mode: ProviderDialogMode,
    /// Index of selected item in the provider list.
    pub selected: usize,
    /// Provider names loaded from config.
    pub providers: Vec<String>,
    // ── Add-custom form fields ──
    pub form_name: String,
    pub form_api_key: String,
    pub form_base_url: String,
    pub form_headers: String,
    pub form_auth_method: AuthMethod,
    /// Which form field index is active (0..3).
    pub active_field: usize,
    pub error: Option<String>,
    pub saved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    None,
    ApiKey,
    Bearer,
    Basic,
}

impl Default for ProviderDialogState {
    fn default() -> Self {
        Self {
            mode: ProviderDialogMode::Select,
            selected: 0,
            providers: vec![],
            form_name: String::new(),
            form_api_key: String::new(),
            form_base_url: String::new(),
            form_headers: String::new(),
            form_auth_method: AuthMethod::ApiKey,
            active_field: 0,
            error: None,
            saved: false,
        }
    }
}

// ── Chat Message (display-friendly) ────────────────────────────────────
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    /// For assistant messages with content blocks.
    pub blocks: Vec<ChatBlock>,
    /// Whether tool results are expanded beyond truncation.
    pub results_expanded: bool,
    /// Tool calls that were made in this message.
    pub tool_calls: Vec<ChatToolCall>,
    /// Error associated with this message.
    pub error: Option<String>,
    /// Wall-clock duration of the agent turn that produced this assistant reply.
    pub duration_ms: Option<u128>,
    /// Image attachment labels shown on user bubbles (file names).
    pub image_labels: Vec<String>,
    /// When this bubble was authored (always painted on the right).
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Cached display row count for `(width, closed)` — see
    /// [`ChatMessage::invalidate_layout`]. `closed` is per-message
    /// (finished vs live tail), not the global agent-busy flag.
    pub layout_cache: Option<(u16, bool, usize)>,
    /// Cached painted lines for closed messages at `(width, closed)`.
    /// `Arc` so a scroll frame can paint by reference (no per-row String clone).
    pub line_cache: Option<(u16, bool, std::sync::Arc<Vec<ratatui::text::Line<'static>>>)>,
    /// Live-tail markdown freeze (Grok checkpoint). Only the growing
    /// assistant bubble uses this; closed messages sit in `line_cache`.
    pub stream_md: Option<crate::md_stream::IncrementalMarkdown>,
}

impl ChatMessage {
    /// Drop cached height/lines so the next layout pass re-measures this message.
    pub fn invalidate_layout(&mut self) {
        self.layout_cache = None;
        self.line_cache = None;
        // Keep `stream_md`: append-only growth still hits the frozen prefix.
        // Width / prefix mismatch inside IncrementalMarkdown resets itself.
    }

    fn blank(role: ChatRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            blocks: vec![],
            results_expanded: false,
            tool_calls: vec![],
            error: None,
            duration_ms: None,
            image_labels: vec![],
            created_at: Some(chrono::Utc::now()),
            layout_cache: None,
            line_cache: None,
            stream_md: None,
        }
    }
}

/// How many trailing reasoning lines to show while the block is still streaming.
/// Matches Grok Build default `truncated_lines: 3`.
pub const THINKING_LIVE_TAIL_LINES: usize = 3;

/// Max lines painted when the user expands a finished thinking block.
/// Unbounded expand of a long reasoning dump freezes soft-wrap paint.
pub const THINKING_EXPANDED_MAX_LINES: usize = 200;

/// Hard cap on stored thinking text (bytes). Prevents runaway memory if a
/// provider streams multi‑MB reasoning or resends full snapshots every chunk.
pub const THINKING_MAX_CHARS: usize = 96 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
    System,
    Tool,
}

impl ChatRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
            Self::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ChatBlock {
    Text(String),
    Thinking(ThinkingBlock),
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        id: String,
        content: String,
        is_error: bool,
    },
    /// Compact Grok-style subagent lifecycle row in the parent scrollback.
    Subagent {
        id: String,
        kind: String,
        description: String,
        status: String,
        activity: String,
        elapsed_ms: u64,
    },
}

/// Payload for [`TuiApp::upsert_subagent`].
#[derive(Debug, Clone)]
pub struct SubagentUpdate {
    pub id: String,
    pub kind: String,
    pub description: String,
    pub status: String,
    pub activity: String,
    pub elapsed_ms: u64,
    pub output: String,
}

/// One child session shown in the top strip / tasks pane / framed view.
#[derive(Debug, Clone)]
pub struct SubagentUi {
    pub id: String,
    pub kind: String,
    pub description: String,
    /// `running` | `completed` | `failed` | `cancelled`
    pub status: String,
    pub activity: String,
    pub started_at: Instant,
    pub elapsed_ms: u64,
    pub output: String,
}

impl SubagentUi {
    pub fn is_running(&self) -> bool {
        self.status == "running"
    }

    pub fn bullet(&self, spin: usize) -> &'static str {
        if self.is_running() {
            const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            FRAMES[spin % FRAMES.len()]
        } else if self.status == "completed" {
            "✓"
        } else {
            "✗"
        }
    }

    pub fn headline(&self) -> String {
        let desc = truncate_mid(&self.description, 48);
        let head = format!("Subagent {}: \"{desc}\" ({})", self.verb(), self.kind);
        if self.is_running() && !self.activity.is_empty() {
            format!("{head} — {}", self.activity)
        } else if !self.is_running() && self.elapsed_ms > 0 {
            format!("{head} in {:.1}s", self.elapsed_ms as f64 / 1000.0)
        } else {
            head
        }
    }

    fn verb(&self) -> &'static str {
        match self.status.as_str() {
            "running" => "running",
            "completed" => "completed",
            "failed" => "failed",
            "cancelled" => "cancelled",
            _ => "started",
        }
    }
}

fn truncate_mid(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1) / 2;
    let start: String = s.chars().take(keep).collect();
    let end: String = s
        .chars()
        .rev()
        .take(keep)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{start}…{end}")
}

/// One reasoning segment with lifecycle (Grok-style Thought for Xs).
///
/// - **Running** (`finished_at == None`): live truncated tail of the stream.
/// - **Finished** + `collapsed`: single header line (`Thought for 1.4s`).
/// - **Finished** + expanded (or running + user expanded): full body.
#[derive(Debug, Clone)]
pub struct ThinkingBlock {
    pub text: String,
    pub started_at: Instant,
    pub finished_at: Option<Instant>,
    /// Per-block fold. Default true; live stream still shows a tail while running.
    pub collapsed: bool,
}

impl ThinkingBlock {
    pub fn new(text: impl Into<String>) -> Self {
        let mut b = Self {
            text: String::new(),
            started_at: Instant::now(),
            finished_at: None,
            collapsed: true,
        };
        b.push_delta(text.into().as_str());
        b
    }

    pub fn is_running(&self) -> bool {
        self.finished_at.is_none()
    }

    pub fn finish(&mut self) {
        if self.finished_at.is_none() {
            self.finished_at = Some(Instant::now());
            // Keep expanded if the user opened it mid-stream.
            // Otherwise stay collapsed (finished default).
        }
    }

    /// Restore a completed thought from session history.
    pub fn finished(text: impl Into<String>) -> Self {
        let mut b = Self::new(text);
        b.finish();
        b
    }

    pub fn elapsed_ms(&self) -> u128 {
        match self.finished_at {
            Some(end) => end.saturating_duration_since(self.started_at).as_millis(),
            None => self.started_at.elapsed().as_millis(),
        }
    }

    /// `1.4s` / `1m12s` for headers.
    pub fn format_elapsed(&self) -> String {
        format_thinking_elapsed(self.elapsed_ms())
    }

    /// Collapsed/finished header label without expand hint.
    ///
    /// Grok-style: running shows `Thinking` (+ live elapsed when available);
    /// finished is `Thought for Xs`. Always includes the word "Thinking" /
    /// "Thought" so the user can see that reasoning happened.
    pub fn header_label(&self) -> String {
        if self.is_running() {
            let elapsed = self.format_elapsed();
            if elapsed.is_empty() || elapsed == "0.0s" {
                "Thinking…".into()
            } else {
                format!("Thinking · {elapsed}")
            }
        } else {
            format!("Thought for {}", self.format_elapsed())
        }
    }

    /// Merge a stream fragment into this block.
    ///
    /// Handles both true deltas and full-snapshot providers (each chunk is the
    /// complete reasoning so far). Dedupes exact re-sends. Caps at
    /// [`THINKING_MAX_CHARS`].
    pub fn push_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        // Full-snapshot style: chunk starts with everything we already have.
        if !self.text.is_empty() && delta.starts_with(self.text.as_str()) {
            let rest = &delta[self.text.len()..];
            if !rest.is_empty() {
                Self::push_capped(&mut self.text, rest);
            }
            return;
        }
        // Exact re-delivery of the last fragment (gateway retries).
        if self.text.ends_with(delta) {
            return;
        }
        Self::push_capped(&mut self.text, delta);
    }

    fn push_capped(buf: &mut String, s: &str) {
        if buf.len() >= THINKING_MAX_CHARS {
            return;
        }
        let room = THINKING_MAX_CHARS - buf.len();
        if s.len() <= room {
            buf.push_str(s);
            return;
        }
        let mut end = room;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        if end > 0 {
            buf.push_str(&s[..end]);
        }
        if !buf.ends_with('…') {
            buf.push('…');
        }
    }

    /// Whether the body should be painted (live tail or full expand).
    pub fn show_body(&self) -> bool {
        if self.is_running() {
            // Live: always show a tail; full body if user expanded.
            true
        } else {
            !self.collapsed
        }
    }

    /// Lines of body text to render given current fold/run state.
    ///
    /// - Live + collapsed → last [`THINKING_LIVE_TAIL_LINES`]
    /// - Expanded → up to [`THINKING_EXPANDED_MAX_LINES`] (from the start;
    ///   footer in the renderer signals truncation)
    pub fn body_lines(&self) -> Vec<&str> {
        if !self.show_body() {
            return Vec::new();
        }
        let lines: Vec<&str> = self.text.lines().collect();
        if self.is_running() && self.collapsed {
            let n = THINKING_LIVE_TAIL_LINES;
            if lines.len() > n {
                return lines[lines.len() - n..].to_vec();
            }
            return lines;
        }
        // Expanded (running or finished).
        if lines.len() > THINKING_EXPANDED_MAX_LINES {
            return lines[..THINKING_EXPANDED_MAX_LINES].to_vec();
        }
        lines
    }

    /// True when live tail dropped earlier lines.
    pub fn is_truncated_live(&self) -> bool {
        self.is_running() && self.collapsed && line_count_gt(&self.text, THINKING_LIVE_TAIL_LINES)
    }

    /// True when expanded paint hit [`THINKING_EXPANDED_MAX_LINES`].
    pub fn is_truncated_expanded(&self) -> bool {
        self.show_body()
            && !(self.is_running() && self.collapsed)
            && line_count_gt(&self.text, THINKING_EXPANDED_MAX_LINES)
    }
}

/// `true` when `text` has more than `n` lines without allocating a full vec.
fn line_count_gt(text: &str, n: usize) -> bool {
    let mut lines = 0usize;
    for _ in text.lines() {
        lines += 1;
        if lines > n {
            return true;
        }
    }
    false
}

/// Format elapsed wall time for display (`1.4s`, `12s`, `1m12s`).
///
/// Used for thinking blocks and full agent-turn latency.
/// Compact duration for chrome / turn footers (Grok `format_duration`).
///
/// - under 10s: `5.2s`
/// - 10–59s: `32s`
/// - 1–59m: `2m5s`
/// - 1h+: `1h2m`
pub fn format_elapsed_ms(ms: u128) -> String {
    let total_secs = ms / 1000;
    if total_secs < 10 {
        let secs = ms as f64 / 1000.0;
        return format!("{secs:.1}s");
    }
    if total_secs < 60 {
        return format!("{total_secs}s");
    }
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    if mins < 60 {
        return format!("{mins}m{secs}s");
    }
    let hours = mins / 60;
    let remaining_mins = mins % 60;
    format!("{hours}h{remaining_mins}m")
}

/// Alias kept for call sites that name the thinking lifecycle.
#[inline]
pub fn format_thinking_elapsed(ms: u128) -> String {
    format_elapsed_ms(ms)
}

/// Compact token usage for status / epilogue (`1.2k in / 340 out`).
pub fn format_usage_short(usage: &whycode_core::types::Usage) -> String {
    let mut s = format!(
        "{} in / {} out",
        format_token_count(usage.input_tokens),
        format_token_count(usage.output_tokens)
    );
    if let Some(read) = usage.cache_read_input_tokens
        && read > 0
    {
        s.push_str(&format!(" / {} cached", format_token_count(read)));
    }
    s
}

/// Compact count for status chrome (`1.2k`, `12k`, `1M`).
pub fn format_token_count(n: u64) -> String {
    if n >= 1_000_000 {
        format_scaled(n as f64 / 1_000_000.0, 'M')
    } else if n >= 1_000 {
        format_scaled(n as f64 / 1_000.0, 'k')
    } else {
        n.to_string()
    }
}

/// One decimal when needed (`1.2k`); whole number when clean (`200k`).
fn format_scaled(v: f64, suffix: char) -> String {
    let tenths = (v * 10.0).round() as i64;
    if tenths % 10 == 0 {
        format!("{}{suffix}", tenths / 10)
    } else {
        format!("{:.1}{suffix}", tenths as f64 / 10.0)
    }
}

/// Tokens currently filling the context window (prompt side) from a provider report.
///
/// Uses **Anthropic-style additive** cache fields (`cache_creation` /
/// `cache_read` are not already inside `input_tokens`). OpenAI-compatible
/// providers must not map subset `cached_tokens` into those fields — only
/// `input_tokens` (= `prompt_tokens`) counts as fill for them.
///
/// Output/completion tokens are excluded: they are not part of the next
/// prefill until the assistant turn is stored in the session.
pub fn context_tokens_from_usage(usage: &whycode_core::types::Usage) -> u64 {
    usage.input_tokens
        + usage.cache_creation_input_tokens.unwrap_or(0)
        + usage.cache_read_input_tokens.unwrap_or(0)
}

/// Grok-style default context label: `1.2k / 200k`.
pub fn format_context_usage(used: u64, max: u64) -> String {
    format!(
        "{} / {}",
        format_token_count(used),
        format_token_count(max.max(1))
    )
}

/// Context fill as a fraction of capacity (`0.0`…`100.0+`).
pub fn context_usage_pct(used: u64, max: u64) -> f64 {
    let max = max.max(1) as f64;
    (used as f64 / max) * 100.0
}

/// Grok `fmt_pct5`: fixed 5-char percentage for hover (`0.00%`…`99.9%` / `MAX %`).
pub fn format_context_percent(used: u64, max: u64) -> String {
    fmt_pct5(context_usage_pct(used, max))
}

/// Fixed-width 5-char percent (matches Grok Build `context_bar::fmt_pct5`).
pub fn fmt_pct5(pct: f64) -> String {
    if pct >= 100.0 {
        "MAX %".to_string()
    } else if pct < 10.0 {
        format!("{pct:.2}%")
    } else {
        format!("{pct:.1}%")
    }
}

/// Grok hover line: 1/8 progress bar + gap + 5-char %, same display width as
/// the default `used / total` token string (no layout shift).
pub fn format_context_hover(used: u64, max: u64) -> String {
    let token_str = format_context_usage(used, max);
    // Min width = gap(1) + pct(5) so degenerate `0 / 9` still matches hover.
    const MIN_W: usize = 6;
    let natural = token_str.chars().count();
    let total_w = natural.max(MIN_W);
    let bar_w = (total_w - MIN_W) as u16;
    let pct = context_usage_pct(used, max);
    let bar = crate::ui::progress_bar::progress_bar_string(bar_w, pct / 100.0);
    format!("{bar} {}", fmt_pct5(pct))
}

/// A rendered tool-call chunk for display.
#[derive(Debug, Clone)]
pub struct ChatToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub collapsed: bool,
    pub result: Option<String>,
    pub is_error: bool,
}

impl std::fmt::Display for ChatRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ── Command State ──────────────────────────────────────────────────────
#[derive(Debug, Clone, Default)]
pub struct CommandState {
    pub buffer: String,
    pub history: Vec<String>,
    pub history_index: usize,
    /// Available commands for tab-completion.
    pub completions: Vec<String>,
    pub completion_index: usize,
}

// ── Agent Running State ────────────────────────────────────────────────
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Generating,
    Thinking,
    WaitingForPermission,
    /// Blocked on interactive `question` questionnaire.
    WaitingForQuestion,
    Error(String),
}

// ── Focus (Grok Build: Prompt vs Scrollback) ───────────────────────────
/// Which pane owns the keyboard.
///
/// Grok's model: when scrollback is focused, j/k navigate the transcript
/// without typing into the prompt; when the prompt is focused, every letter
/// goes to the draft. Tab toggles. Typing while scrollback is focused
/// auto-focuses the prompt (simple mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPane {
    #[default]
    Prompt,
    Scrollback,
}

/// Double-Esc window for "press again to clear" (Grok: 800ms).
pub const ESC_DOUBLE_MS: u128 = 800;

// ── TuiApp ─────────────────────────────────────────────────────────────
pub struct TuiApp {
    // ── runtime ──
    pub running: bool,
    pub mode: AppMode,
    pub key_context: KeymapContext,
    /// Prompt or scrollback owns keys (Grok focus model).
    pub focus: FocusPane,

    // ── session ──
    pub messages: Vec<ChatMessage>,
    pub current_agent_state: AgentState,
    pub status_message: String,
    pub spinner_frame: usize,
    /// Paint when true. Cleared after a successful draw unless animation
    /// (spinner / live toast) still needs frames. See the TUI event loop.
    pub needs_redraw: bool,
    /// Extra `terminal.clear()` paints. Bracketed-paste echo (and key-flood
    /// paste on hosts without bracketed paste) writes onto the PTY outside
    /// ratatui's diff; breathing-room cells stay spaces in both frames so
    /// the leftover sits left of the centered home prompt until we force a
    /// full rewrite. Backspace/delete must request this too — otherwise the
    /// ghost cannot be erased.
    pub pending_full_clears: u8,

    // ── input ──
    pub input_buffer: String,
    /// Multi-line input lines beyond the first.
    pub input_lines: Vec<String>,
    pub input_history: Vec<String>,
    pub input_history_idx: usize,
    /// Cursor column in the current input line.
    pub input_cursor: usize,
    /// First Esc of a double-Esc clear/cancel gesture.
    pub esc_armed_at: Option<std::time::Instant>,
    /// Images staged on the prompt (drag-drop / path paste). Sent with the next turn.
    pub pending_images: Vec<crate::images::PromptImage>,
    /// Images consumed with `pending_prompt` by the run loop (taken on submit).
    pub pending_submit_images: Vec<crate::images::PromptImage>,
    /// Large pastes collapsed to `[pasted #N ~ L lines]` tokens in `input_buffer`.
    /// Expanded on submit so the agent receives the full text.
    pub pending_pastes: Vec<crate::paste::PastedBlock>,

    // ── scroll / selection ──
    /// Display rows scrolled up from the newest line (not message count).
    pub scroll_offset: usize,
    pub auto_scroll: bool,
    /// Selected message index when scrollback is focused.
    pub selected_msg: Option<usize>,
    /// Chat viewport height in rows (updated each paint).
    pub chat_viewport_rows: u16,
    /// Chat content width in columns (updated each paint).
    pub chat_content_width: u16,
    /// Last-paint message viewport (for wheel hit-testing).
    pub chat_area: Option<Rect>,
    /// Last-paint chat scrollbar track when content overflows.
    pub chat_scrollbar_hit: Option<Rect>,
    /// Total display rows of the transcript at last paint (scrollbar math).
    pub chat_scroll_total: usize,
    /// Active chat scrollbar thumb drag (`None` = not dragging).
    pub chat_scrollbar_grab: Option<u16>,

    // ── mouse text selection (app-owned; native terminal select copies pad spaces) ──
    pub mouse_sel: Option<MouseSelection>,
    /// Last drawn frame as a packed cell grid (selection → clipboard).
    pub screen_cells: crate::cell_grid::CellGrid,

    // ── dialogs ──
    pub dialogs: DialogManager,
    pub provider_dialog: ProviderDialogState,
    pub model_selection: ModelSelectionState,
    pub session_list: SessionListState,
    pub login_dialog: LoginDialogState,
    /// Last-paint hit box for the modal `[✗]` control (click = cancel).
    pub dialog_close_hit: Option<Rect>,
    /// Full modal rect (border inclusive). Text selection/copy is clipped to this
    /// so background chat behind the popup cannot be selected.
    pub dialog_modal_hit: Option<Rect>,
    /// Last-paint list body for the active select-style dialog.
    pub dialog_list_hit: Option<Rect>,
    /// Last-paint scrollbar track (when the list overflows).
    pub dialog_scrollbar_hit: Option<Rect>,
    /// First visible row index of that list (for click → absolute index).
    pub dialog_list_scroll_start: usize,
    /// Viewport row capacity of that list.
    pub dialog_list_visible: usize,
    /// Total items in that list.
    pub dialog_list_total: usize,
    /// Active thumb drag: row within the thumb where the grab started (`None` = not dragging).
    pub dialog_scrollbar_grab: Option<u16>,
    /// Questionnaire closed via `[✗]` / generic dismiss — run loop completes oneshot.
    pub question_dismissed: bool,
    /// Questionnaire finished via mouse click (single-select) — run loop sends answers.
    pub pending_question_answers: Option<Vec<QuestionAnswer>>,

    // ── slash suggestion popup ──
    pub slash_suggest: SlashSuggestState,

    // ── `@file` picker (workspace index backed) ──
    pub file_suggest: crate::ui::file_suggest::FileSuggestState,

    // ── transient notices ──
    pub toasts: crate::toast::Toasts,
    pub help_scroll: usize,
    /// Filter text in the Keyboard Shortcuts popup (`/` to start).
    pub help_query: String,
    /// True while the cheatsheet search bar owns typing.
    pub help_searching: bool,

    // ── sidebar ──
    pub sidebar: SidebarState,

    // ── command ──
    pub command: CommandState,

    // ── theme ──
    pub theme: ThemeName,
    /// Cursor in the Theme picker dialog (`ThemeName::ALL` index).
    pub theme_selected: usize,

    // ── config ──
    pub config: crate::config::TuiAppConfig,

    /// Prompt waiting to be sent to the agent (set by submit / slash commands).
    /// Images for this turn live in `pending_submit_images` until the run loop takes them.
    pub pending_prompt: Option<String>,
    /// Queued prompts from `/loop` or `schedule` (drained when idle).
    pub pending_auto_prompts: std::collections::VecDeque<String>,
    /// Idle follow-up suggestion (`tui.prompt_suggestions = "idle"`). Tab accepts when input empty.
    pub pending_suggestion: Option<String>,
    /// OAuth paste-code flow: while set, the next submitted input line is
    /// the pasted `code#state`, not a prompt. Dropping the sender cancels
    /// the in-flight login (the flow's receiver errors out).
    pub auth_code_sink: Option<tokio::sync::oneshot::Sender<String>>,
    /// Running background shell jobs (status bar chip).
    pub bg_running_count: usize,
    /// Model switch from the picker dialog: `(provider, model)`.
    pub pending_model: Option<(String, String)>,
    /// Reasoning effort from the picker / `/effort` (`low`/`medium`/`high`/`xhigh`).
    pub pending_effort: Option<String>,
    /// Cursor in the effort picker (`ThinkingConfig::supported_efforts`).
    pub effort_picker_selected: usize,
    /// Last chosen `reasoning_effort` (`None` = family default).
    pub reasoning_effort: Option<String>,
    /// Agent switch from the picker dialog (prompt footer click / `/agent`).
    pub pending_agent: Option<String>,
    /// Cursor in the agent picker (list is `primary_agents`).
    pub agent_picker_selected: usize,
    /// `/login` picker result: provider id awaiting an OAuth flow spawn.
    pub pending_login_provider: Option<String>,
    /// Session id to load from the DB (picker Enter or `/resume <id>`).
    pub pending_session_id: Option<String>,
    /// Dashboard: cursor row in the grouped live-session list.
    pub sessions_cursor: usize,
    /// Dashboard: switch target — index into the parked runtimes vec,
    /// or `usize::MAX` for "stay on current".
    pub pending_session_switch: Option<usize>,
    /// Dashboard rows snapshot, refreshed by the run loop on open.
    /// Each row: (parked index or None = active session, title, state glyph,
    /// state label, preview, unread).
    pub sessions_rows: Vec<SessionDashboardRow>,
    /// Re-fetch `GET /v1/models` for the active provider (config base + key).
    pub pending_catalog_refresh: bool,

    /// Primary agent names for Ctrl+T cycling (build/plan).
    pub primary_agents: Vec<String>,
    pub agent_cycle_idx: usize,

    // ── session chrome (status header / footer) ──
    pub provider_name: String,
    pub model_name: String,
    pub agent_name: String,
    /// Last turn intent badge for chrome (`Q` / `chg` / `plan`), if any.
    pub intent_badge: Option<String>,
    /// Full intent kind for tooltips/status (`question`, `change`, …).
    pub intent_kind: Option<String>,
    /// Current session display title (auto or manual).
    pub session_title: String,
    /// Short project name (basename) for the top status strip.
    pub project_label: String,
    /// Absolute working directory (bottom bar; click-to-copy target).
    pub project_dir: PathBuf,
    /// Current git branch, if the project is a repo.
    pub git_branch: Option<String>,
    /// Clickable cwd path (sticky hover → underline).
    pub cwd_hit: crate::hit_area::HitArea,
    /// Context-usage meter (hover → bar+%).
    pub context_hit: crate::hit_area::HitArea,
    /// Turn-status `[stop]` control (click → cancel turn).
    pub turn_stop_hit: crate::hit_area::HitArea,
    /// Prompt-footer agent name (click → agent picker).
    pub agent_hit: crate::hit_area::HitArea,
    /// Prompt-footer provider/model (click → model picker).
    pub model_hit: crate::hit_area::HitArea,
    /// Prompt-footer reasoning effort (click → effort picker).
    pub effort_hit: crate::hit_area::HitArea,
    /// Last known mouse cell (for hover tooltips).
    pub mouse_pos: Option<(u16, u16)>,

    /// Tokens currently filling the context window (last provider report or estimate).
    pub context_used: u64,
    /// Context window capacity for the *active* model
    /// (config → `/v1/models` → built-in catalog → `session.max_context_tokens`).
    pub max_context_tokens: u64,
    /// Live context window from `GET /v1/models` for `(provider, model)`, if known.
    /// Only the single active window is kept — never the full gateway catalog.
    pub api_context_window: Option<u32>,
    /// Which provider/model `api_context_window` was fetched for.
    pub api_context_for: Option<(String, String)>,

    /// When the current agent turn started (live latency while busy).
    pub turn_started_at: Option<std::time::Instant>,
    /// Latest token usage reported for the current/last turn.
    pub turn_usage: Option<whycode_core::types::Usage>,
    /// Mouse `[stop]` (or future UI) requested cancel — run loop consumes this.
    pub pending_cancel: bool,
    /// Active session id (todo file key; empty before the first session).
    pub session_id: String,
    /// Session todo list (sticky panel under the header).
    pub todos: Vec<whycode_core::TodoItem>,
    /// Fold the sticky todo list to a single header row (Grok chevron).
    /// Auto-collapses when every item is done; click / `t` reopens it.
    pub todos_collapsed: bool,
    /// Header row of the sticky todo panel (click to fold).
    pub todos_hit: crate::hit_area::HitArea,
    /// Live + finished child sessions (Grok tasks pane / top strip).
    pub subagents: Vec<SubagentUi>,
    /// When set, the framed child transcript overlays the parent session.
    pub open_subagent: Option<String>,
    /// Last-paint hit boxes for the top subagent strip (click to open).
    pub subagent_strip_hit: Vec<(Rect, String)>,
}

/// Drag selection over the terminal grid (absolute cell coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseSelection {
    pub anchor_x: u16,
    pub anchor_y: u16,
    pub focus_x: u16,
    pub focus_y: u16,
    pub dragging: bool,
}

impl MouseSelection {
    pub fn normalized(self) -> (u16, u16, u16, u16) {
        let x0 = self.anchor_x.min(self.focus_x);
        let x1 = self.anchor_x.max(self.focus_x);
        let y0 = self.anchor_y.min(self.focus_y);
        let y1 = self.anchor_y.max(self.focus_y);
        (x0, y0, x1, y1)
    }
}

/// Model selection dialog state.
#[derive(Debug, Clone, Default)]
pub struct ModelSelectionState {
    pub models: Vec<(String, String)>, // (provider_name, model_id)
    pub selected: usize,
}

/// Every provider/model pair the config knows about, for the model picker.
///
/// OAuth subscription logins (`/login`) bypass config, so their providers
/// would never appear here: merge in the suggested models for any provider
/// with a credential in the token store.
pub fn catalog_models(config: &whycode_config::Config) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = config
        .providers
        .values()
        .flat_map(|p| p.models.iter().map(move |m| (p.name.clone(), m.clone())))
        .collect();
    if let Ok(dir) = whycode_config::Config::data_dir() {
        let store = whycode_auth::TokenStore::new(&dir);
        for name in whycode_auth::OAUTH_PROVIDERS {
            if store.get(name).ok().flatten().is_some() {
                out.extend(
                    whycode_auth::providers::suggested_models(name)
                        .iter()
                        .map(|m| ((*name).to_string(), (*m).to_string())),
                );
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// One row of the session list dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    pub id: String,
    pub title: String,
    pub messages: usize,
    /// Last activity (`Session.updated_at`). Shown next to the message count.
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Live (in-memory) session: parked runtime index, or `Some(usize::MAX)`
    /// for the currently active session. `None` = persisted-only row.
    pub live: Option<usize>,
}

/// Session list dialog state.
#[derive(Debug, Clone, Default)]
pub struct SessionListState {
    pub sessions: Vec<SessionEntry>,
    pub selected: usize,
    /// Row chosen for closing (Ctrl+W): parked runtime index, or
    /// `Some(usize::MAX)` for the active session. Persisted-only rows are
    /// never closed from the picker (use `/sessions` management instead).
    pub pending_close: Option<usize>,
}

/// One dashboard row: a live (in-memory) session, active or parked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDashboardRow {
    /// Index into the parked runtimes vec; `None` = the active session.
    pub parked_idx: Option<usize>,
    pub title: String,
    pub glyph: String,
    pub state_label: String,
    pub preview: String,
    pub unread: bool,
}

/// Slash command authored from the input buffer.
#[derive(Debug, Clone)]
pub struct SlashCommand {
    pub name: &'static str,
    pub hint: &'static str,
}

pub const BUILTIN_SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "/exit",
        hint: "Quit the TUI",
    },
    SlashCommand {
        name: "/help",
        hint: "Keybinding cheatsheet",
    },
    SlashCommand {
        name: "/new",
        hint: "Start a new session",
    },
    SlashCommand {
        name: "/init",
        hint: "Create or refresh AGENTS.md",
    },
    SlashCommand {
        name: "/review",
        hint: "AI review of git changes (read-only)",
    },
    SlashCommand {
        name: "/security-review",
        hint: "Security-focused change review",
    },
    SlashCommand {
        name: "/commit",
        hint: "Draft (and optionally create) a git commit",
    },
    SlashCommand {
        name: "/context",
        hint: "Context window breakdown",
    },
    SlashCommand {
        name: "/undo",
        hint: "Undo last turn via git",
    },
    SlashCommand {
        name: "/redo",
        hint: "Redo the undone turn",
    },
    SlashCommand {
        name: "/compact",
        hint: "[context] Compact the conversation (LLM summary)",
    },
    SlashCommand {
        name: "/fresh",
        hint: "Skip provider prompt cache on the next turn",
    },
    SlashCommand {
        name: "/remember",
        hint: "[text] Save a durable project memory",
    },
    SlashCommand {
        name: "/memory",
        hint: "Show memory path and recent entries",
    },
    SlashCommand {
        name: "/share",
        hint: "Export session share link",
    },
    SlashCommand {
        name: "/sessions",
        hint: "Pick a stored session and resume (Enter)",
    },
    SlashCommand {
        name: "/resume",
        hint: "[id] Resume a session (picker if no id)",
    },
    SlashCommand {
        name: "/continue",
        hint: "Resume the most recent session",
    },
    SlashCommand {
        name: "/rename",
        hint: "[args] Set the session title (locks auto-title)",
    },
    SlashCommand {
        name: "/models",
        hint: "[args] Switch provider or model (e.g. /models anthropic/claude-sonnet-4-5)",
    },
    SlashCommand {
        name: "/tools",
        hint: "List the agent's tools",
    },
    SlashCommand {
        name: "/info",
        hint: "Session details",
    },
    SlashCommand {
        name: "/doctor",
        hint: "Environment / config diagnostics",
    },
    SlashCommand {
        name: "/diff",
        hint: "Git status + diff --stat for the project",
    },
    SlashCommand {
        name: "/cost",
        hint: "Session + last-turn token usage",
    },
    SlashCommand {
        name: "/usage",
        hint: "Alias for /cost",
    },
    SlashCommand {
        name: "/agent",
        hint: "[args] Switch agent (picker if no name, e.g. /agent plan)",
    },
    SlashCommand {
        name: "/effort",
        hint: "[low|medium|high|xhigh] Reasoning effort (picker if no arg)",
    },
    SlashCommand {
        name: "/connect",
        hint: "Provider / API key help",
    },
    SlashCommand {
        name: "/login",
        hint: "[provider] OAuth subscription sign-in (picker if none)",
    },
    SlashCommand {
        name: "/theme",
        hint: "[name] Switch theme (picker if no name)",
    },
    SlashCommand {
        name: "/themes",
        hint: "Open the theme picker",
    },
    SlashCommand {
        name: "/unshare",
        hint: "Delete local share files",
    },
    SlashCommand {
        name: "/bg",
        hint: "[list|kill id] Background shell jobs",
    },
    SlashCommand {
        name: "/loop",
        hint: "N prompt… | stop — queue N sequential turns",
    },
];

/// Autocomplete state for slash commands while typing.
#[derive(Debug, Clone, Default)]
pub struct SlashSuggestState {
    pub active: bool,
    pub matches: Vec<usize>,
    pub selected: usize,
    /// Mouse-hovered match index (into `matches`), sticky for paint.
    pub hovered: Option<usize>,
    /// Absolute screen rect of the item list body (for hover hit-test).
    pub list_hit: Option<Rect>,
    /// First visible match index when scrolled (paint meta).
    pub list_scroll_start: usize,
}

impl SlashSuggestState {
    /// Rebuild the match list from the input buffer.
    /// Active only while the buffer starts with `/` and has no space yet
    /// — once the user types an argument the command choice is fixed.
    ///
    /// Extra leading slashes (`//`, `///`) collapse to `/` so a second `/`
    /// after Esc (which leaves a lone `/` in the draft) reopens the menu
    /// instead of filtering to zero matches.
    pub fn refresh(&mut self, input: &str) {
        if !input.starts_with('/') || input.contains(char::is_whitespace) {
            self.active = false;
            self.matches.clear();
            return;
        }
        // `//` and friends: treat as bare `/` (show every command).
        let query = if input.bytes().all(|b| b == b'/') {
            "/"
        } else {
            input
        };
        self.matches = BUILTIN_SLASH_COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.name.starts_with(query)
                    || c.name
                        .strip_prefix('/')
                        .is_some_and(|n| n.starts_with(query.trim_start_matches('/')))
            })
            .map(|(i, _)| i)
            .collect();
        if self.matches.is_empty() {
            // No command matches the typed prefix — hide the popup.
            self.active = false;
            self.selected = 0;
            return;
        }
        self.active = true;
        if self.selected >= self.matches.len() {
            self.selected = 0;
        }
    }

    pub fn step(&mut self, delta: isize) {
        if self.matches.is_empty() {
            return;
        }
        self.selected = move_selection(self.selected, self.matches.len(), delta);
    }

    pub fn current(&self) -> Option<&'static SlashCommand> {
        self.matches
            .get(self.selected)
            .map(|&i| &BUILTIN_SLASH_COMMANDS[i])
    }

    pub fn dismiss(&mut self) {
        self.active = false;
        self.matches.clear();
        self.selected = 0;
        self.hovered = None;
        self.list_hit = None;
        self.list_scroll_start = 0;
    }

    /// Match index under the pointer, if any (using last paint’s list hit).
    pub fn row_index_at(&self, col: u16, row: u16) -> Option<usize> {
        let hit = self.list_hit?;
        if col < hit.x
            || col >= hit.x.saturating_add(hit.width)
            || row < hit.y
            || row >= hit.y.saturating_add(hit.height)
        {
            return None;
        }
        let vis = (row - hit.y) as usize;
        let idx = self.list_scroll_start + vis;
        if idx < self.matches.len() {
            Some(idx)
        } else {
            None
        }
    }
}

/// Cursor movement shared by every list-style dialog.
///
/// Wraps at both ends: a list short enough to see at once is faster to reach
/// the last item of by pressing up once.
pub fn move_selection(selected: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = len as isize;
    let next = (selected as isize + delta).rem_euclid(len);
    next as usize
}

impl TuiApp {
    /// Request a paint on the next event-loop iteration.
    pub fn mark_dirty(&mut self) {
        self.needs_redraw = true;
    }

    /// Force `terminal.clear()` on the next `n` paints (at least one).
    ///
    /// Use 2 after a paste so an emulator that echoes *after* `Event::Paste`
    /// is still wiped on the follow-up frame.
    pub fn request_full_clear(&mut self, frames: u8) {
        self.pending_full_clears = self.pending_full_clears.max(frames.max(1));
        self.needs_redraw = true;
    }

    pub fn new(config: crate::config::TuiAppConfig) -> Self {
        config.theme.apply_syntax_theme();
        Self::from_config(config)
    }

    /// Same as [`Self::new`] but does **not** touch the process-wide syntax
    /// theme. Background-session drain uses this so a parked turn cannot
    /// flush highlight caches every tick.
    pub(crate) fn from_config(config: crate::config::TuiAppConfig) -> Self {
        Self {
            running: true,
            mode: AppMode::Normal,
            key_context: KeymapContext::Normal,
            focus: FocusPane::Prompt,
            messages: vec![],
            current_agent_state: AgentState::Idle,
            status_message: String::from("Ready — /help for keybindings"),
            spinner_frame: 0,
            needs_redraw: true,
            pending_full_clears: 0,
            input_buffer: String::new(),
            input_lines: vec![],
            input_history: vec![],
            input_history_idx: 0,
            input_cursor: 0,
            esc_armed_at: None,
            pending_images: vec![],
            pending_submit_images: vec![],
            pending_pastes: vec![],
            scroll_offset: 0,
            auto_scroll: true,
            selected_msg: None,
            chat_viewport_rows: 20,
            chat_content_width: 80,
            chat_area: None,
            chat_scrollbar_hit: None,
            chat_scroll_total: 0,
            chat_scrollbar_grab: None,
            mouse_sel: None,
            screen_cells: crate::cell_grid::CellGrid::default(),
            dialogs: DialogManager::new(),
            provider_dialog: ProviderDialogState::default(),
            model_selection: ModelSelectionState::default(),
            session_list: SessionListState::default(),
            login_dialog: LoginDialogState::default(),
            dialog_close_hit: None,
            dialog_modal_hit: None,
            dialog_list_hit: None,
            dialog_scrollbar_hit: None,
            dialog_list_scroll_start: 0,
            dialog_list_visible: 0,
            dialog_list_total: 0,
            dialog_scrollbar_grab: None,
            question_dismissed: false,
            pending_question_answers: None,
            slash_suggest: SlashSuggestState::default(),
            file_suggest: crate::ui::file_suggest::FileSuggestState::default(),
            toasts: crate::toast::Toasts::default(),
            help_scroll: 0,
            help_query: String::new(),
            help_searching: false,
            sidebar: SidebarState {
                visible: config.show_sidebar,
                ..SidebarState::default()
            },
            command: CommandState::default(),
            theme: config.theme,
            theme_selected: ThemeName::ALL
                .iter()
                .position(|t| *t == config.theme)
                .unwrap_or(0),
            config,
            pending_prompt: None,
            pending_auto_prompts: std::collections::VecDeque::new(),
            pending_suggestion: None,
            auth_code_sink: None,
            bg_running_count: 0,
            pending_model: None,
            pending_effort: None,
            effort_picker_selected: 0,
            reasoning_effort: None,
            pending_agent: None,
            agent_picker_selected: 0,
            pending_login_provider: None,
            pending_session_id: None,
            sessions_cursor: 0,
            pending_session_switch: None,
            sessions_rows: Vec::new(),
            pending_catalog_refresh: false,
            primary_agents: vec!["build".into(), "plan".into(), "ask".into()],
            agent_cycle_idx: 0,
            provider_name: String::new(),
            model_name: String::new(),
            agent_name: String::from("build"),
            intent_badge: None,
            intent_kind: None,
            session_title: String::new(),
            project_label: String::from("."),
            project_dir: PathBuf::from("."),
            git_branch: None,
            cwd_hit: crate::hit_area::HitArea::default(),
            context_hit: crate::hit_area::HitArea::default(),
            turn_stop_hit: crate::hit_area::HitArea::default(),
            agent_hit: crate::hit_area::HitArea::default(),
            model_hit: crate::hit_area::HitArea::default(),
            effort_hit: crate::hit_area::HitArea::default(),
            mouse_pos: None,
            context_used: 0,
            max_context_tokens: 200_000,
            api_context_window: None,
            api_context_for: None,
            turn_started_at: None,
            turn_usage: None,
            pending_cancel: false,
            session_id: String::new(),
            todos: Vec::new(),
            todos_collapsed: false,
            todos_hit: crate::hit_area::HitArea::default(),
            subagents: Vec::new(),
            open_subagent: None,
            subagent_strip_hit: Vec::new(),
        }
    }

    /// Insert or update a child session and its scrollback lifecycle block.
    pub fn upsert_subagent(&mut self, update: SubagentUpdate) {
        let SubagentUpdate {
            id,
            kind,
            description,
            status,
            activity,
            elapsed_ms,
            output,
        } = update;
        if let Some(row) = self.subagents.iter_mut().find(|s| s.id == id) {
            row.kind = kind.clone();
            row.description = description.clone();
            row.status = status.clone();
            row.activity = activity.clone();
            row.elapsed_ms = elapsed_ms;
            if !output.is_empty() {
                row.output = output;
            }
        } else {
            self.subagents.push(SubagentUi {
                id: id.clone(),
                kind: kind.clone(),
                description: description.clone(),
                status: status.clone(),
                activity: activity.clone(),
                started_at: Instant::now(),
                elapsed_ms,
                output,
            });
        }

        let mut found = false;
        for msg in self.messages.iter_mut().rev() {
            for block in &mut msg.blocks {
                if let ChatBlock::Subagent {
                    id: bid,
                    kind: k,
                    description: d,
                    status: st,
                    activity: act,
                    elapsed_ms: el,
                } = block
                    && *bid == id
                {
                    *k = kind.clone();
                    *d = description.clone();
                    *st = status.clone();
                    *act = activity.clone();
                    *el = elapsed_ms;
                    msg.invalidate_layout();
                    found = true;
                    break;
                }
            }
            if found {
                break;
            }
        }
        if !found {
            let mut msg = ChatMessage::blank(ChatRole::System, String::new());
            msg.blocks.push(ChatBlock::Subagent {
                id,
                kind,
                description,
                status,
                activity,
                elapsed_ms,
            });
            self.messages.push(msg);
        }
        self.mark_dirty();
    }

    pub fn running_subagent_count(&self) -> usize {
        self.subagents.iter().filter(|s| s.is_running()).count()
    }

    pub fn has_subagent_strip(&self) -> bool {
        !self.subagents.is_empty()
    }

    pub fn open_subagent_view(&mut self, id: &str) {
        if self.subagents.iter().any(|s| s.id == id) {
            self.open_subagent = Some(id.to_string());
            self.mark_dirty();
        }
    }

    pub fn close_subagent_view(&mut self) {
        if self.open_subagent.take().is_some() {
            self.mark_dirty();
        }
    }

    pub fn toggle_tasks_pane(&mut self) {
        if self.sidebar.visible && self.sidebar.active_tab == SidebarTab::Agents {
            self.sidebar.visible = false;
        } else {
            self.sidebar.visible = true;
            self.sidebar.active_tab = SidebarTab::Agents;
        }
        self.mark_dirty();
    }

    /// Refresh `git_branch` from the working tree (cheap; call on start / idle).
    pub fn refresh_git_branch(&mut self) {
        self.git_branch = resolve_git_branch(&self.project_dir);
    }

    /// True if terminal cell `(col, row)` is inside the clickable cwd path.
    pub fn cwd_contains(&self, col: u16, row: u16) -> bool {
        self.cwd_hit.contains(col, row)
    }

    /// Sticky: pointer is over the context meter.
    pub fn context_hovered(&self) -> bool {
        self.context_hit.hovered
    }

    /// Update sticky hover flags for all chrome HitAreas. Returns true if any flipped.
    pub fn update_chrome_hover(&mut self) -> bool {
        let Some((c, r)) = self.mouse_pos else {
            let mut changed = false;
            if self.context_hit.hovered {
                self.context_hit.hovered = false;
                changed = true;
            }
            if self.cwd_hit.hovered {
                self.cwd_hit.hovered = false;
                changed = true;
            }
            if self.turn_stop_hit.hovered {
                self.turn_stop_hit.hovered = false;
                changed = true;
            }
            if self.agent_hit.hovered {
                self.agent_hit.hovered = false;
                changed = true;
            }
            if self.model_hit.hovered {
                self.model_hit.hovered = false;
                changed = true;
            }
            if self.todos_hit.hovered {
                self.todos_hit.hovered = false;
                changed = true;
            }
            return changed;
        };
        let mut changed = false;
        changed |= self.context_hit.update_hover(c, r);
        changed |= self.cwd_hit.update_hover(c, r);
        changed |= self.turn_stop_hit.update_hover(c, r);
        changed |= self.agent_hit.update_hover(c, r);
        changed |= self.model_hit.update_hover(c, r);
        changed |= self.effort_hit.update_hover(c, r);
        changed |= self.todos_hit.update_hover(c, r);
        // Slash dropdown hover row (index into matches, not absolute cmd).
        if self.slash_suggest.active {
            if let Some(idx) = self.slash_suggest.row_index_at(c, r) {
                if self.slash_suggest.hovered != Some(idx) {
                    self.slash_suggest.hovered = Some(idx);
                    changed = true;
                }
            } else if self.slash_suggest.hovered.is_some() {
                self.slash_suggest.hovered = None;
                changed = true;
            }
        } else if self.slash_suggest.hovered.is_some() {
            self.slash_suggest.hovered = None;
            changed = true;
        }
        // File picker hover row (index into matches).
        if self.file_suggest.active {
            if let Some(idx) = self.file_suggest.row_index_at(c, r) {
                if self.file_suggest.hovered != Some(idx) {
                    self.file_suggest.hovered = Some(idx);
                    changed = true;
                }
            } else if self.file_suggest.hovered.is_some() {
                self.file_suggest.hovered = None;
                changed = true;
            }
        } else if self.file_suggest.hovered.is_some() {
            self.file_suggest.hovered = None;
            changed = true;
        }
        changed
    }

    /// Install the workspace file index (called once at startup).
    pub fn set_file_index(&mut self, index: std::sync::Arc<whycode_index::WorkspaceIndex>) {
        self.file_suggest.set_index(index);
    }

    /// Update context fill from a provider usage event (per LLM step).
    ///
    /// Prefer this over [`Self::sync_context_estimate`] when the provider
    /// reported prompt-side tokens for the request that was just sent.
    pub fn set_context_from_usage(&mut self, usage: &whycode_core::types::Usage) {
        self.context_used = context_tokens_from_usage(usage);
    }

    /// Estimate context fill from the live transcript (chars/4 heuristic).
    ///
    /// Use when provider usage is missing or stale (resume, compact, undo,
    /// silent stream). Never use cumulative `session.usage` here — that is
    /// billed tokens across all turns, not current window fill.
    pub fn sync_context_estimate(&mut self, session: &whycode_session::session::Session) {
        self.context_used = session.token_count() as u64;
    }

    /// Percent of context window used (0–100+, whole numbers).
    pub fn context_percent(&self) -> u64 {
        let max = self.max_context_tokens.max(1);
        ((self.context_used as f64 / max as f64) * 100.0).round() as u64
    }

    /// Resolve and apply the context window for the active model.
    ///
    /// Priority: config → live `/v1/models` (same provider/model) → built-in → session fallback.
    pub fn apply_context_window(
        &mut self,
        provider: &str,
        model: &str,
        configured: Option<u32>,
        session_fallback: u64,
    ) {
        let api = self.api_context_window.filter(|_| {
            self.api_context_for
                .as_ref()
                .is_some_and(|(p, m)| p == provider && m == model)
        });
        self.max_context_tokens =
            whycode_llm::resolve_context_window(provider, model, configured, api, session_fallback);
    }

    /// Apply a single-model context window from `GET /v1/models` and re-resolve.
    pub fn set_api_context_window(
        &mut self,
        provider: &str,
        model: &str,
        window: u32,
        configured: Option<u32>,
        session_fallback: u64,
    ) {
        if window == 0 {
            return;
        }
        self.api_context_window = Some(window);
        self.api_context_for = Some((provider.to_string(), model.to_string()));
        self.apply_context_window(provider, model, configured, session_fallback);
    }

    /// Drop live API window (e.g. provider switch) so we do not reuse a stale max.
    pub fn clear_api_context_window(&mut self) {
        self.api_context_window = None;
        self.api_context_for = None;
    }

    /// Mark wall-clock start of a new agent turn (call when the request is sent).
    pub fn mark_turn_started(&mut self) {
        self.turn_started_at = Some(std::time::Instant::now());
        self.turn_usage = None;
    }

    /// Live elapsed ms for the in-flight turn, if any.
    pub fn turn_elapsed_ms(&self) -> Option<u128> {
        self.turn_started_at.map(|t| t.elapsed().as_millis())
    }

    /// Finish turn timing: stamp duration on the last assistant message and clear the timer.
    ///
    /// Returns the measured duration when a turn was in progress.
    pub fn complete_turn_timing(&mut self) -> Option<u128> {
        let ms = self.turn_elapsed_ms()?;
        Some(self.complete_turn_timing_ms(ms))
    }

    /// Stamp a pre-measured work duration (excludes post-turn title refine).
    ///
    /// Prefer this when the agent task reports `work_ms` separately so
    /// "Worked for Xs" is the real turn, not wall time until the UI is free.
    pub fn complete_turn_timing_ms(&mut self, ms: u128) -> u128 {
        self.turn_started_at = None;
        if let Some(last) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.role == ChatRole::Assistant)
        {
            last.duration_ms = Some(ms);
        }
        ms
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.current_agent_state,
            AgentState::Generating
                | AgentState::Thinking
                | AgentState::WaitingForPermission
                | AgentState::WaitingForQuestion
        )
    }

    /// Reset modal mouse targets (call at the start of each dialog paint).
    ///
    /// Does **not** clear `dialog_scrollbar_grab` — a drag spans frames and
    /// must survive repaint.
    pub fn clear_dialog_hits(&mut self) {
        self.dialog_close_hit = None;
        self.dialog_modal_hit = None;
        self.dialog_list_hit = None;
        self.dialog_scrollbar_hit = None;
        self.dialog_list_scroll_start = 0;
        self.dialog_list_visible = 0;
        self.dialog_list_total = 0;
    }

    /// Apply chrome hit boxes shared by every modal (`dialog_frame` result).
    ///
    /// All popups — help, confirm, permission, pickers — must call this (or
    /// [`apply_select_paint`]) so mouse selection is clipped to the modal and
    /// `[✗]` / scrollbar work the same way.
    pub fn apply_modal_chrome(
        &mut self,
        close_hit: Option<Rect>,
        modal: Rect,
        scrollbar_hit: Option<Rect>,
    ) {
        self.dialog_close_hit = close_hit;
        self.dialog_modal_hit = Some(modal);
        self.dialog_scrollbar_hit = scrollbar_hit;
    }

    /// Apply hit boxes from a select-style list paint.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_select_paint(
        &mut self,
        close_hit: Option<Rect>,
        list_area: Option<Rect>,
        scrollbar_hit: Option<Rect>,
        scroll_start: usize,
        visible: usize,
        total: usize,
        modal: Option<Rect>,
    ) {
        self.dialog_close_hit = close_hit;
        self.dialog_modal_hit = modal;
        self.dialog_list_hit = list_area;
        self.dialog_scrollbar_hit = scrollbar_hit;
        self.dialog_list_scroll_start = scroll_start;
        self.dialog_list_visible = visible;
        self.dialog_list_total = total;
    }

    /// True while any modal owns the screen (dialog stack or Help overlay).
    pub fn modal_is_open(&self) -> bool {
        self.dialogs.is_open()
            || self.mode == AppMode::Help
            || self.key_context == KeymapContext::Dialog
    }

    /// Whether `(col, row)` lands on the painted modal close control.
    pub fn dialog_close_contains(&self, col: u16, row: u16) -> bool {
        self.dialog_close_hit
            .map(|h| col >= h.x && col < h.x.saturating_add(h.width) && row == h.y)
            .unwrap_or(false)
    }

    /// Whether `(col, row)` is inside the active modal (border inclusive).
    pub fn dialog_modal_contains(&self, col: u16, row: u16) -> bool {
        self.dialog_modal_hit
            .map(|h| {
                col >= h.x
                    && col < h.x.saturating_add(h.width)
                    && row >= h.y
                    && row < h.y.saturating_add(h.height)
            })
            .unwrap_or(false)
    }

    /// Clamp a screen cell into the modal rect (or return as-is if no modal).
    pub fn clamp_to_dialog_modal(&self, col: u16, row: u16) -> (u16, u16) {
        let Some(h) = self.dialog_modal_hit else {
            return (col, row);
        };
        let x1 = h.x.saturating_add(h.width.saturating_sub(1));
        let y1 = h.y.saturating_add(h.height.saturating_sub(1));
        (col.clamp(h.x, x1), row.clamp(h.y, y1))
    }

    /// Whether the pointer is over the close control (for hover repaint).
    pub fn dialog_close_hovered(&self) -> bool {
        self.mouse_pos
            .map(|(c, r)| self.dialog_close_contains(c, r))
            .unwrap_or(false)
    }

    /// Whether `(col, row)` lands on the scrollbar track.
    pub fn dialog_scrollbar_contains(&self, col: u16, row: u16) -> bool {
        self.dialog_scrollbar_hit
            .map(|h| crate::ui::scrollbar::scrollbar_contains(h, col, row))
            .unwrap_or(false)
    }

    /// Whether `(col, row)` lands on the chat transcript scrollbar track.
    pub fn chat_scrollbar_contains(&self, col: u16, row: u16) -> bool {
        self.chat_scrollbar_hit
            .map(|h| crate::ui::scrollbar::scrollbar_contains(h, col, row))
            .unwrap_or(false)
    }

    /// Whether `(col, row)` is inside the message viewport (not prompt/footer).
    pub fn chat_area_contains(&self, col: u16, row: u16) -> bool {
        self.chat_area
            .map(|a| {
                col >= a.x
                    && col < a.x.saturating_add(a.width)
                    && row >= a.y
                    && row < a.y.saturating_add(a.height)
            })
            .unwrap_or(false)
    }

    /// Publish chat viewport + optional scrollbar hit box after a session paint.
    ///
    /// Keeps `chat_scrollbar_grab` across frames while the bar is still shown
    /// so a thumb drag survives repaints; drops the grab when the bar vanishes.
    /// Also clamps `scroll_offset` to the painted document size so a shrink
    /// (resize, collapse) cannot leave the viewport past the end.
    pub fn apply_chat_paint(&mut self, area: Rect, scrollbar_hit: Option<Rect>, total_rows: usize) {
        self.chat_area = Some(area);
        self.chat_scrollbar_hit = scrollbar_hit;
        self.chat_scroll_total = total_rows;
        // Paint is the source of truth for viewport metrics used by scroll.
        self.chat_viewport_rows = area.height;
        self.chat_content_width = area.width;
        if scrollbar_hit.is_none() {
            self.chat_scrollbar_grab = None;
        }
        self.clamp_chat_scroll();
    }

    /// Clear chat mouse targets (home / empty session).
    pub fn clear_chat_hits(&mut self) {
        self.chat_area = None;
        self.chat_scrollbar_hit = None;
        self.chat_scroll_total = 0;
        // End any in-progress thumb drag when the bar goes away.
        self.chat_scrollbar_grab = None;
    }

    /// `(total_rows, viewport_height, max_scroll_offset)` for the transcript.
    ///
    /// Prefers last-paint totals so wheel/page math matches what is on screen.
    pub fn chat_scroll_metrics(&mut self) -> (usize, usize, usize) {
        let height = self.chat_viewport_rows.max(1) as usize;
        let total = if self.chat_scroll_total > 0 {
            self.chat_scroll_total
        } else {
            let width = self.chat_content_width.max(20);
            crate::ui::chat::session_line_count_mut(self, width)
        };
        let max_off = total.saturating_sub(height);
        (total, height, max_off)
    }

    /// Clamp `scroll_offset` into `[0, max_off]` after layout/paint changes.
    pub fn clamp_chat_scroll(&mut self) {
        let (_total, _height, max_off) = self.chat_scroll_metrics();
        if self.scroll_offset > max_off {
            self.scroll_offset = max_off;
        }
        if self.scroll_offset == 0 {
            self.auto_scroll = true;
        }
    }

    /// Map a screen cell to a list index when it falls inside the list body.
    pub fn dialog_list_index_at(&self, col: u16, row: u16) -> Option<usize> {
        let hit = self.dialog_list_hit?;
        if col < hit.x
            || col >= hit.x.saturating_add(hit.width)
            || row < hit.y
            || row >= hit.y.saturating_add(hit.height)
        {
            return None;
        }
        let row_off = (row - hit.y) as usize;
        let idx = self.dialog_list_scroll_start.saturating_add(row_off);
        if idx < self.dialog_list_total {
            Some(idx)
        } else {
            None
        }
    }

    pub fn focus_prompt(&mut self) {
        self.focus = FocusPane::Prompt;
        self.selected_msg = None;
    }

    pub fn focus_scrollback(&mut self) {
        if self.messages.is_empty() {
            return;
        }
        self.focus = FocusPane::Scrollback;
        if self.selected_msg.is_none() {
            self.selected_msg = Some(self.messages.len().saturating_sub(1));
        }
        self.auto_scroll = false;
    }

    pub fn toggle_focus(&mut self) {
        match self.focus {
            FocusPane::Prompt => self.focus_scrollback(),
            FocusPane::Scrollback => self.focus_prompt(),
        }
    }

    /// Save non-empty draft to history and clear the prompt (Grok double-Esc).
    pub fn clear_prompt_draft(&mut self) {
        let text = self.input_buffer.trim();
        if !text.is_empty() {
            // History stores expanded text so re-selecting a past entry is usable.
            let expanded = crate::paste::expand(text, &self.pending_pastes);
            self.input_history.push(expanded);
            self.input_history_idx = self.input_history.len();
        }
        self.input_buffer.clear();
        self.input_lines.clear();
        self.input_cursor = 0;
        self.pending_images.clear();
        self.pending_pastes.clear();
        self.slash_suggest.dismiss();
        self.file_suggest.dismiss();
        self.esc_armed_at = None;
        self.request_full_clear(1);
    }

    /// Insert text at the cursor. Large pastes become a collapsed `[pasted #N ~ L lines]`
    /// token so the prompt stays short and does not reflow/flicker.
    pub fn insert_paste_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let pos = {
            let idx = self.input_cursor.min(self.input_buffer.len());
            if self.input_buffer.is_char_boundary(idx) {
                idx
            } else {
                // Align to char boundary (same as input::clamp_cursor).
                let mut i = idx;
                while i > 0 && !self.input_buffer.is_char_boundary(i) {
                    i -= 1;
                }
                i
            }
        };
        if crate::paste::should_collapse(text) {
            let id = crate::paste::next_id();
            let lines = crate::paste::line_count(text);
            let token = crate::paste::placeholder(id, lines);
            self.pending_pastes.push(crate::paste::PastedBlock {
                id,
                content: text.to_string(),
            });
            self.input_buffer.insert_str(pos, &token);
            self.input_cursor = pos + token.len();
        } else {
            self.input_buffer.insert_str(pos, text);
            self.input_cursor = pos + text.len();
        }
        crate::paste::prune_unused(&mut self.pending_pastes, &self.input_buffer);
        // Two frames: some hosts echo the payload after delivering Paste.
        self.request_full_clear(2);
    }

    /// Expand collapsed paste tokens for the agent / history.
    pub fn expand_input(&self) -> String {
        crate::paste::expand(&self.input_buffer, &self.pending_pastes)
    }

    /// Remove a paste placeholder span and drop its stored body.
    pub fn remove_paste_span(&mut self, start: usize, end: usize, id: u32) {
        if start > end || end > self.input_buffer.len() {
            return;
        }
        self.input_buffer.replace_range(start..end, "");
        self.pending_pastes.retain(|b| b.id != id);
        self.input_cursor = start;
        crate::paste::prune_unused(&mut self.pending_pastes, &self.input_buffer);
        self.request_full_clear(1);
    }

    /// Attach an image path to the prompt (deduped by path). Returns true if added.
    pub fn attach_image(&mut self, path: &std::path::Path) -> Result<(), String> {
        if self.pending_images.len() >= crate::images::MAX_ATTACHMENTS {
            return Err(format!(
                "max {} images per message",
                crate::images::MAX_ATTACHMENTS
            ));
        }
        let img = crate::images::load_prompt_image(path)?;
        if self.pending_images.iter().any(|p| p.path == img.path) {
            return Ok(()); // already attached
        }
        self.pending_images.push(img);
        Ok(())
    }

    /// Remove the last staged image (Backspace on empty buffer).
    pub fn pop_pending_image(&mut self) -> Option<crate::images::PromptImage> {
        self.pending_images.pop()
    }

    /// True when the prompt has staged images and/or non-empty text.
    pub fn prompt_has_content(&self) -> bool {
        !self.input_buffer.trim().is_empty() || !self.pending_images.is_empty()
    }

    /// Jump selection to previous/next user message (Grok turn navigation).
    pub fn jump_user_turn(&mut self, forward: bool) {
        if self.messages.is_empty() {
            return;
        }
        self.focus = FocusPane::Scrollback;
        let cur = self
            .selected_msg
            .unwrap_or(self.messages.len().saturating_sub(1));
        let next = if forward {
            (cur + 1..self.messages.len()).find(|&i| self.messages[i].role == ChatRole::User)
        } else {
            (0..cur)
                .rev()
                .find(|&i| self.messages[i].role == ChatRole::User)
        };
        if let Some(i) = next {
            self.selected_msg = Some(i);
            self.ensure_selected_visible();
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.messages.is_empty() {
            return;
        }
        self.focus = FocusPane::Scrollback;
        let len = self.messages.len() as isize;
        let cur = self.selected_msg.unwrap_or(len as usize - 1) as isize;
        let next = (cur + delta).clamp(0, len - 1) as usize;
        self.selected_msg = Some(next);
        self.ensure_selected_visible();
    }

    pub fn toggle_selected_thinking(&mut self) {
        if let Some(i) = self.selected_msg
            && let Some(msg) = self.messages.get_mut(i)
        {
            // Grok `e`/`h` also unfolds a collapsed user prompt.
            if msg.role == ChatRole::User {
                msg.results_expanded = !msg.results_expanded;
                msg.invalidate_layout();
                self.mark_dirty();
                return;
            }
            // Flip all thinking blocks in the selected message to the opposite
            // of the last one's state (per-block fold, coordinated toggle).
            let target = msg
                .blocks
                .iter()
                .rev()
                .find_map(|b| match b {
                    ChatBlock::Thinking(t) => Some(!t.collapsed),
                    _ => None,
                })
                .unwrap_or(false);
            for block in &mut msg.blocks {
                if let ChatBlock::Thinking(t) = block {
                    t.collapsed = target;
                }
            }
            // Height changes — clear cache and request a frame (same as tools).
            msg.invalidate_layout();
            self.mark_dirty();
        }
    }

    /// Freeze any open (streaming) thinking blocks on the last assistant message.
    pub fn finish_open_thinking(&mut self) {
        if let Some(last) = self.messages.last_mut()
            && last.role == ChatRole::Assistant
        {
            let mut changed = false;
            for block in &mut last.blocks {
                if let ChatBlock::Thinking(t) = block
                    && t.is_running()
                {
                    t.finish();
                    changed = true;
                }
            }
            if changed {
                last.invalidate_layout();
                self.mark_dirty();
            }
        }
    }

    pub fn toggle_selected_tools(&mut self) {
        if let Some(i) = self.selected_msg
            && let Some(msg) = self.messages.get_mut(i)
        {
            msg.results_expanded = !msg.results_expanded;
            for tc in &mut msg.tool_calls {
                tc.collapsed = !msg.results_expanded;
            }
            msg.invalidate_layout();
            self.mark_dirty();
        }
    }

    /// Copy selected message (or last) to the system clipboard.
    pub fn copy_selected_message(&mut self) -> bool {
        let idx = self
            .selected_msg
            .or_else(|| self.messages.len().checked_sub(1));
        let Some(i) = idx else {
            return false;
        };
        let Some(msg) = self.messages.get(i) else {
            return false;
        };
        let mut text = msg.content.clone();
        for block in &msg.blocks {
            match block {
                ChatBlock::Thinking(t) if t.show_body() || !t.collapsed => {
                    text.push_str(&format!("\n\n[thinking · {}]\n", t.format_elapsed()));
                    text.push_str(&t.text);
                }
                ChatBlock::Thinking(t) => {
                    // Collapsed finished: still leave a one-line summary.
                    text.push_str(&format!("\n\n[{}]", t.header_label()));
                }
                ChatBlock::ToolUse { name, input, .. } => {
                    text.push_str(&format!("\n\n[{name}]\n{input}\n"));
                }
                ChatBlock::ToolResult { content, .. } => {
                    text.push('\n');
                    text.push_str(content);
                }
                ChatBlock::Subagent {
                    kind,
                    description,
                    status,
                    ..
                } => {
                    text.push_str(&format!("\n\n[subagent {status} {kind}] {description}"));
                }
                _ => {}
            }
        }
        for tc in &msg.tool_calls {
            if let Some(ref r) = tc.result {
                text.push_str(&format!("\n\n[{} result]\n{r}", tc.name));
            }
        }
        if text.trim().is_empty() {
            return false;
        }
        if crate::clipboard::copy_text(&text) {
            self.toasts.push(
                crate::toast::ToastKind::Info,
                format!("Copied message ({} chars)", text.chars().count()),
            );
            true
        } else {
            self.toasts.push(
                crate::toast::ToastKind::Warning,
                "Copy failed — no clipboard",
            );
            false
        }
    }

    /// Keep the selected message inside the chat viewport (row-based).
    pub fn ensure_selected_visible(&mut self) {
        let Some(sel) = self.selected_msg else {
            return;
        };
        let width = self.chat_content_width.max(20);
        let height = self.chat_viewport_rows.max(1) as usize;
        let (starts, total) = crate::ui::chat::message_row_layout_mut(self, width);
        if total == 0 || sel >= starts.len() {
            return;
        }
        let start = starts[sel];
        let end = if sel + 1 < starts.len() {
            starts[sel + 1]
        } else {
            total
        };
        // Viewport shows [view_top, view_bottom) in top-origin coordinates.
        let view_bottom = total.saturating_sub(self.scroll_offset);
        let view_top = view_bottom.saturating_sub(height);
        if start < view_top {
            self.scroll_offset = total.saturating_sub(start.saturating_add(height));
            self.auto_scroll = false;
        } else if end > view_bottom {
            self.scroll_offset = total.saturating_sub(end);
            self.auto_scroll = false;
        }
    }

    /// Scroll by display rows (positive = older / up toward history).
    ///
    /// Prefer the last-paint line total (`chat_scroll_total`) so max offset
    /// matches what the user actually sees; fall back to a live layout count
    /// before the first session paint.
    pub fn scroll_rows(&mut self, delta: isize) {
        let (_total, _height, max_off) = self.chat_scroll_metrics();
        let prev = self.scroll_offset;
        if delta > 0 {
            self.scroll_offset = (self.scroll_offset + delta as usize).min(max_off);
        } else {
            let down = (-delta) as usize;
            self.scroll_offset = self.scroll_offset.saturating_sub(down);
        }
        // At bottom (offset 0) always follow the stream — including the no-op
        // case where content already fits the viewport (max_off == 0).
        self.auto_scroll = self.scroll_offset == 0;
        if self.scroll_offset != prev {
            self.mark_dirty();
        }
    }

    pub fn scroll_page(&mut self, up: bool) {
        let page = self.chat_viewport_rows.max(1) as isize;
        self.scroll_rows(if up { page } else { -page });
    }

    pub fn scroll_to_top(&mut self) {
        let (_total, _height, max_off) = self.chat_scroll_metrics();
        self.scroll_offset = max_off;
        self.auto_scroll = false;
        if !self.messages.is_empty() {
            self.selected_msg = Some(0);
        }
        self.mark_dirty();
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll = true;
        self.mark_dirty();
        if !self.messages.is_empty() {
            self.selected_msg = Some(self.messages.len() - 1);
        }
    }

    /// Open a permission dialog for tool approval.
    pub fn ask_permission(&mut self, tool_name: impl Into<String>, detail: impl Into<String>) {
        self.mode = AppMode::Dialog;
        self.key_context = KeymapContext::Dialog;
        self.current_agent_state = AgentState::WaitingForPermission;
        self.dialogs.push(DialogKind::Permission {
            tool_name: tool_name.into(),
            detail: detail.into(),
        });
    }

    /// Open an interactive questionnaire (agent `question` tool).
    pub fn ask_question(&mut self, questions: Vec<QuestionSpec>) {
        self.mode = AppMode::Dialog;
        self.key_context = KeymapContext::Dialog;
        self.current_agent_state = AgentState::WaitingForQuestion;
        self.dialogs
            .push(DialogKind::Question(QuestionDialogState::new(questions)));
        self.status_message = "Waiting for your answer…".into();
    }

    /// Push a simple alert dialog.
    pub fn alert(&mut self, title: impl Into<String>, message: impl Into<String>) {
        self.mode = AppMode::Dialog;
        self.key_context = KeymapContext::Dialog;
        self.dialogs.push(DialogKind::Alert {
            title: title.into(),
            message: message.into(),
        });
    }

    /// Push a confirmation dialog.
    pub fn confirm(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
        action: ConfirmAction,
    ) {
        self.mode = AppMode::Dialog;
        self.key_context = KeymapContext::Dialog;
        self.dialogs.push(DialogKind::Confirm {
            title: title.into(),
            message: message.into(),
            on_confirm: action,
        });
    }

    /// Replace the chat view with a reconstructed transcript from a stored session.
    ///
    /// Used when resuming via the session picker, `/resume`, or `--continue`.
    /// Tool-role messages are folded into the matching assistant tool-call when
    /// possible so the UI matches live turns.
    pub fn load_messages_from_session(&mut self, session: &whycode_session::session::Session) {
        self.messages = chat_messages_from_session(session);
        self.session_title = session.title.clone();
        self.session_id = session.id.clone();
        self.replace_todos(whycode_core::todo::load_todos(
            &self.project_dir,
            if session.id.is_empty() {
                None
            } else {
                Some(session.id.as_str())
            },
        ));
        self.scroll_offset = 0;
        self.auto_scroll = true;
        self.selected_msg = None;
        // Context fill = current transcript size, not cumulative billed usage.
        // Session totals remain available via /cost (session.usage).
        self.sync_context_estimate(session);
        self.turn_usage = None;
        self.mark_dirty();
    }

    /// Snapshot the per-session view state into `view` (switching away).
    pub fn save_view(&self, view: &mut crate::session_runtime::ViewSnapshot) {
        view.messages = self.messages.clone();
        view.session_title = self.session_title.clone();
        view.status_message = self.status_message.clone();
        view.current_agent_state = self.current_agent_state.clone();
        view.scroll_offset = self.scroll_offset;
        view.auto_scroll = self.auto_scroll;
        view.selected_msg = self.selected_msg;
        view.input_buffer = self.input_buffer.clone();
        view.input_lines = self.input_lines.clone();
        view.input_cursor = self.input_cursor;
        view.intent_badge = self.intent_badge.clone();
        view.intent_kind = self.intent_kind.clone();
        view.turn_usage = self.turn_usage.clone();
        view.context_used = self.context_used;
        view.pending_suggestion = self.pending_suggestion.clone();
        view.session_id = self.session_id.clone();
        view.todos = self.todos.clone();
        view.todos_collapsed = self.todos_collapsed;
    }

    /// Restore a previously saved per-session view state (switching back).
    pub fn restore_view(&mut self, view: &crate::session_runtime::ViewSnapshot) {
        self.messages = view.messages.clone();
        self.session_title = view.session_title.clone();
        self.status_message = view.status_message.clone();
        self.current_agent_state = view.current_agent_state.clone();
        self.scroll_offset = view.scroll_offset;
        self.auto_scroll = view.auto_scroll;
        self.selected_msg = view.selected_msg;
        self.input_buffer = view.input_buffer.clone();
        self.input_lines = view.input_lines.clone();
        self.input_cursor = view.input_cursor;
        self.intent_badge = view.intent_badge.clone();
        self.intent_kind = view.intent_kind.clone();
        self.turn_usage = view.turn_usage.clone();
        self.context_used = view.context_used;
        self.pending_suggestion = view.pending_suggestion.clone();
        self.session_id = view.session_id.clone();
        self.todos = view.todos.clone();
        self.todos_collapsed = view.todos_collapsed;
        self.dialogs.clear();
        self.mark_dirty();
    }

    /// Move `view` into this app (no transcript clone). Pair with
    /// [`Self::yield_view`] after applying background turn events.
    pub fn adopt_view(&mut self, view: &mut crate::session_runtime::ViewSnapshot) {
        self.messages = std::mem::take(&mut view.messages);
        self.session_title = std::mem::take(&mut view.session_title);
        self.status_message = std::mem::take(&mut view.status_message);
        self.current_agent_state =
            std::mem::replace(&mut view.current_agent_state, AgentState::Idle);
        self.scroll_offset = view.scroll_offset;
        self.auto_scroll = view.auto_scroll;
        self.selected_msg = view.selected_msg;
        self.input_buffer = std::mem::take(&mut view.input_buffer);
        self.input_lines = std::mem::take(&mut view.input_lines);
        self.input_cursor = view.input_cursor;
        self.intent_badge = view.intent_badge.take();
        self.intent_kind = view.intent_kind.take();
        self.turn_usage = view.turn_usage.take();
        self.context_used = view.context_used;
        self.pending_suggestion = view.pending_suggestion.take();
        self.session_id = std::mem::take(&mut view.session_id);
        self.todos = std::mem::take(&mut view.todos);
        self.todos_collapsed = view.todos_collapsed;
    }

    /// Move this app's view fields back into `view` (no transcript clone).
    pub fn yield_view(&mut self, view: &mut crate::session_runtime::ViewSnapshot) {
        view.messages = std::mem::take(&mut self.messages);
        view.session_title = std::mem::take(&mut self.session_title);
        view.status_message = std::mem::take(&mut self.status_message);
        view.current_agent_state =
            std::mem::replace(&mut self.current_agent_state, AgentState::Idle);
        view.scroll_offset = self.scroll_offset;
        view.auto_scroll = self.auto_scroll;
        view.selected_msg = self.selected_msg;
        view.input_buffer = std::mem::take(&mut self.input_buffer);
        view.input_lines = std::mem::take(&mut self.input_lines);
        view.input_cursor = self.input_cursor;
        view.intent_badge = self.intent_badge.take();
        view.intent_kind = self.intent_kind.take();
        view.turn_usage = self.turn_usage.take();
        view.context_used = self.context_used;
        view.pending_suggestion = self.pending_suggestion.take();
        view.session_id = std::mem::take(&mut self.session_id);
        view.todos = std::mem::take(&mut self.todos);
        view.todos_collapsed = self.todos_collapsed;
    }

    /// Replace the session todo list. Auto-collapses when every item is
    /// terminal; unfolds again when new open work arrives.
    pub fn replace_todos(&mut self, todos: Vec<whycode_core::TodoItem>) {
        let was_all_done = whycode_core::todo::all_terminal(&self.todos);
        let all_done = whycode_core::todo::all_terminal(&todos);
        self.todos = todos;
        if self.todos.is_empty() {
            self.todos_collapsed = false;
            self.todos_hit.clear();
        } else if all_done && !was_all_done {
            // Just finished (or loaded a completed list) — fold to the header.
            self.todos_collapsed = true;
        } else if !all_done && was_all_done {
            // New open work after a completed list — show the items.
            self.todos_collapsed = false;
        }
        self.mark_dirty();
    }

    /// Fold / unfold the sticky todo panel (header click or `t`).
    pub fn toggle_todos_panel(&mut self) {
        if self.todos.is_empty() {
            return;
        }
        self.todos_collapsed = !self.todos_collapsed;
        self.mark_dirty();
    }

    /// Add a message to the chat view.
    pub fn add_message(&mut self, role: ChatRole, content: impl Into<String>) {
        // Previous last assistant loses the `is_last` token footer.
        if let Some(last) = self.messages.last_mut() {
            last.invalidate_layout();
        }
        self.messages.push(ChatMessage::blank(role, content));
        self.mark_dirty();
    }

    pub fn add_user_message_with_images(
        &mut self,
        content: impl Into<String>,
        image_labels: Vec<String>,
    ) {
        if let Some(last) = self.messages.last_mut() {
            last.invalidate_layout();
        }
        let mut msg = ChatMessage::blank(ChatRole::User, content);
        msg.image_labels = image_labels;
        self.messages.push(msg);
        self.mark_dirty();
    }

    /// Append text to the last assistant message (streaming).
    pub fn append_to_last(&mut self, text: &str) {
        self.finish_open_thinking();
        if let Some(last) = self.messages.last_mut() {
            if last.role == ChatRole::Assistant {
                last.content.push_str(text);
                last.invalidate_layout();
            } else {
                self.add_message(ChatRole::Assistant, text);
                return;
            }
        } else {
            self.add_message(ChatRole::Assistant, text);
            return;
        }
        self.mark_dirty();
    }

    /// Append a thinking block to the last assistant message.
    ///
    /// Empty / whitespace-only fragments are ignored (no empty "Thinking…"
    /// flash). Open blocks use [`ThinkingBlock::push_delta`] so full-snapshot
    /// providers do not quadratic-duplicate reasoning.
    pub fn append_thinking(&mut self, text: &str) {
        if text.is_empty() || text.chars().all(|c| c.is_whitespace()) {
            return;
        }
        if let Some(last) = self.messages.last_mut()
            && last.role == ChatRole::Assistant
        {
            // Append to the last *open* thinking block; start a new one after tools/text.
            match last.blocks.last_mut() {
                Some(ChatBlock::Thinking(t)) if t.is_running() => {
                    t.push_delta(text);
                }
                _ => {
                    last.blocks
                        .push(ChatBlock::Thinking(ThinkingBlock::new(text)));
                }
            }
            last.invalidate_layout();
            self.mark_dirty();
            return;
        }
        // No assistant message yet — create one.
        let mut msg = ChatMessage::blank(ChatRole::Assistant, String::new());
        msg.blocks = vec![ChatBlock::Thinking(ThinkingBlock::new(text))];
        self.messages.push(msg);
        self.mark_dirty();
    }

    /// Add a tool-call to the last assistant message.
    pub fn add_tool_call(&mut self, id: String, name: String, arguments: serde_json::Value) {
        self.finish_open_thinking();
        let tc = ChatToolCall {
            id: id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
            collapsed: true,
            result: None,
            is_error: false,
        };

        if let Some(last) = self.messages.last_mut()
            && last.role == ChatRole::Assistant
        {
            last.blocks.push(ChatBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.name.clone(),
                input: tc.arguments.clone(),
            });
            last.tool_calls.push(tc);
            last.invalidate_layout();
            self.mark_dirty();
            return;
        }

        let mut msg = ChatMessage::blank(ChatRole::Assistant, String::new());
        msg.blocks = vec![ChatBlock::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: arguments.clone(),
        }];
        msg.tool_calls = vec![tc];
        self.messages.push(msg);
        self.mark_dirty();
    }

    /// Add a tool result to the most recent tool-call.
    pub fn add_tool_result(
        &mut self,
        tool_use_id: &str,
        content: impl Into<String>,
        is_error: bool,
    ) {
        let content = content.into();
        // Search backwards for the matching tool-call
        for msg in self.messages.iter_mut().rev() {
            for tc in msg.tool_calls.iter_mut() {
                if tc.id == tool_use_id {
                    tc.result = Some(content.clone());
                    tc.is_error = is_error;
                    msg.blocks.push(ChatBlock::ToolResult {
                        id: tool_use_id.to_string(),
                        content,
                        is_error,
                    });
                    msg.invalidate_layout();
                    self.mark_dirty();
                    return;
                }
            }
        }
    }

    /// Submit current input as user message and queue it for the agent.
    pub fn submit_input(&mut self) {
        // OAuth paste-code flow intercepts before anything else: the input
        // line is the sign-in code, not a prompt for the agent.
        if let Some(sink) = self.auth_code_sink.take() {
            let code = self.input_buffer.trim().to_string();
            self.input_buffer.clear();
            self.input_lines.clear();
            self.input_cursor = 0;
            self.pending_pastes.clear();
            if code.is_empty() {
                // Dropping the sender cancels the flow.
                self.status_message = "sign-in cancelled".into();
            } else if sink.send(code).is_err() {
                // Receiver dropped: the flow task already went away.
                self.status_message = "sign-in flow already closed".into();
            } else {
                self.status_message = "code received — finishing sign-in…".into();
            }
            self.mark_dirty();
            return;
        }
        let display_text = self.input_buffer.trim().to_string();
        let has_images = !self.pending_images.is_empty();
        if display_text.is_empty() && !has_images {
            return;
        }
        // Slash commands are handled by the run loop before submit (text-only).
        if display_text.starts_with('/') && !has_images {
            return;
        }
        // Agent gets fully expanded pastes; chat keeps the compact placeholder
        // so huge dumps do not bloat the transcript.
        let expanded = crate::paste::expand(&display_text, &self.pending_pastes);
        if !expanded.is_empty() {
            self.input_history.push(expanded.clone());
        }
        let labels: Vec<String> = self
            .pending_images
            .iter()
            .map(|i| i.label.clone())
            .collect();
        let display = if display_text.is_empty() && has_images {
            // Chat bubble still needs a line of content.
            if labels.len() == 1 {
                format!("[Image: {}]", labels[0])
            } else {
                format!("[Images: {}]", labels.join(", "))
            }
        } else {
            display_text
        };
        self.add_user_message_with_images(display, labels);
        self.pending_submit_images = std::mem::take(&mut self.pending_images);
        self.pending_prompt = Some(expanded);
        self.input_buffer.clear();
        self.input_lines.clear();
        self.input_cursor = 0;
        self.pending_pastes.clear();
        self.input_history_idx = self.input_history.len();
        self.auto_scroll = true;
        self.scroll_offset = 0;
        self.focus_prompt();
        self.esc_armed_at = None;
    }
}

/// Build display messages from a persisted session transcript.
pub fn chat_messages_from_session(session: &whycode_session::session::Session) -> Vec<ChatMessage> {
    use whycode_core::types::{ContentBlock, MessageContent, Role};

    let mut out: Vec<ChatMessage> = Vec::new();

    for msg in &session.messages {
        let role = match msg.role {
            Role::User => ChatRole::User,
            Role::Assistant => ChatRole::Assistant,
            Role::System => ChatRole::System,
            Role::Tool => ChatRole::Tool,
        };

        // Tool results: fold into the matching assistant tool-call when present.
        if role == ChatRole::Tool {
            let content = match &msg.content {
                MessageContent::Text(t) => t.clone(),
                MessageContent::Blocks(blocks) => blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            if let Some(id) = msg.tool_call_id.as_deref() {
                let mut attached = false;
                for m in out.iter_mut().rev() {
                    for tc in m.tool_calls.iter_mut() {
                        if tc.id == id {
                            tc.result = Some(content.clone());
                            m.blocks.push(ChatBlock::ToolResult {
                                id: id.to_string(),
                                content: content.clone(),
                                is_error: false,
                            });
                            m.invalidate_layout();
                            attached = true;
                            break;
                        }
                    }
                    if attached {
                        break;
                    }
                }
                if attached {
                    continue;
                }
            }
            let mut row = ChatMessage::blank(ChatRole::Tool, content);
            row.created_at = msg.created_at;
            out.push(row);
            continue;
        }

        match &msg.content {
            MessageContent::Text(t) => {
                // Compact carriers are model-facing user_meta. Grok paints a
                // session event, not a collapsed ❯ prompt — show the summary
                // body as a system card so the 9 sections stay readable.
                let (role, content) =
                    if role == ChatRole::User && whycode_session::is_compact_summary_text(t) {
                        (
                            ChatRole::System,
                            whycode_session::compact_summary_display_text(t),
                        )
                    } else {
                        (role, t.clone())
                    };
                let mut row = ChatMessage::blank(role, content);
                row.created_at = msg.created_at;
                out.push(row);
            }
            MessageContent::Blocks(blocks) => {
                let mut content = String::new();
                let mut ui_blocks: Vec<ChatBlock> = Vec::new();
                let mut tool_calls: Vec<ChatToolCall> = Vec::new();
                let mut image_labels: Vec<String> = Vec::new();

                for b in blocks {
                    match b {
                        ContentBlock::Text { text } => {
                            if !content.is_empty() && !text.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(text);
                            if !text.is_empty() {
                                ui_blocks.push(ChatBlock::Text(text.clone()));
                            }
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            tool_calls.push(ChatToolCall {
                                id: id.clone(),
                                name: name.clone(),
                                arguments: input.clone(),
                                collapsed: true,
                                result: None,
                                is_error: false,
                            });
                            ui_blocks.push(ChatBlock::ToolUse {
                                id: id.clone(),
                                name: name.clone(),
                                input: input.clone(),
                            });
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content: c,
                            is_error,
                        } => {
                            let err = is_error.unwrap_or(false);
                            if let Some(tc) = tool_calls.iter_mut().find(|t| t.id == *tool_use_id) {
                                tc.result = Some(c.clone());
                                tc.is_error = err;
                            }
                            ui_blocks.push(ChatBlock::ToolResult {
                                id: tool_use_id.clone(),
                                content: c.clone(),
                                is_error: err,
                            });
                        }
                        ContentBlock::Image { source } => {
                            let label = match source {
                                whycode_core::types::ImageSource::Url { url } => {
                                    url.rsplit('/').next().unwrap_or("image").to_string()
                                }
                                whycode_core::types::ImageSource::Base64 { media_type, .. } => {
                                    format!("image ({media_type})")
                                }
                            };
                            image_labels.push(label);
                        }
                        ContentBlock::Thinking { text, .. } => {
                            if !text.is_empty() {
                                ui_blocks.push(ChatBlock::Thinking(ThinkingBlock::finished(
                                    text.clone(),
                                )));
                            }
                        }
                        ContentBlock::RedactedThinking { .. } => {
                            ui_blocks.push(ChatBlock::Thinking(ThinkingBlock::finished(
                                "[redacted]".to_string(),
                            )));
                        }
                    }
                }

                // Assistant text often lives only in Text blocks; keep content for
                // the bubble and blocks for tools/thinking layout.
                let mut row = ChatMessage::blank(role, content);
                row.blocks = ui_blocks;
                row.tool_calls = tool_calls;
                row.image_labels = image_labels;
                row.created_at = msg.created_at;
                out.push(row);
            }
        }
    }

    out
}

/// How long we will wait on `git` before giving up (Grok/jcode: a wedged
/// index.lock must not freeze the TUI).
const GIT_BRANCH_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

/// Resolve the current branch name for `dir`, if it is a git work tree.
fn resolve_git_branch(dir: &std::path::Path) -> Option<String> {
    use std::process::Command;

    let out = git_output_timeout(
        Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(dir),
        GIT_BRANCH_TIMEOUT,
    )?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() {
        return None;
    }
    // Detached HEAD → short SHA is more useful than the literal "HEAD".
    if name == "HEAD" {
        let sha = git_output_timeout(
            Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .current_dir(dir),
            GIT_BRANCH_TIMEOUT,
        )?;
        if sha.status.success() {
            let s = String::from_utf8_lossy(&sha.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    Some(name)
}

/// `Command::output` with a wall-clock cap. On timeout the child is killed.
pub(crate) fn git_output_timeout(
    cmd: &mut std::process::Command,
    timeout: std::time::Duration,
) -> Option<std::process::Output> {
    use std::process::Stdio;
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                if let Some(mut s) = child.stdout.take()
                    && let Err(e) = std::io::Read::read_to_end(&mut s, &mut stdout)
                {
                    tracing::debug!(error = %e, "git stdout read after exit");
                }
                return Some(std::process::Output {
                    status,
                    stdout,
                    stderr: Vec::new(),
                });
            }
            Ok(None) if start.elapsed() >= timeout => {
                if let Err(e) = child.kill() {
                    tracing::debug!(error = %e, "git timeout kill");
                }
                if let Err(e) = child.wait() {
                    tracing::debug!(error = %e, "git timeout wait");
                }
                return None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(5)),
            Err(e) => {
                tracing::debug!(error = %e, "git try_wait");
                return None;
            }
        }
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;
    use crate::config::TuiAppConfig;
    use whycode_tools::question::QuestionOption;

    fn app() -> TuiApp {
        TuiApp::from_config(TuiAppConfig::default())
    }

    fn question(prompt: &str, labels: &[&str], multi_select: bool) -> QuestionSpec {
        QuestionSpec {
            prompt: prompt.into(),
            options: labels
                .iter()
                .map(|label| QuestionOption {
                    label: (*label).into(),
                    description: String::new(),
                    preview: None,
                })
                .collect(),
            multi_select,
        }
    }

    #[test]
    fn question_answers_rehydrate_when_navigating_back_and_forward() {
        let mut state = QuestionDialogState::new(vec![
            question("Pick", &["A", "B"], false),
            question("Explain", &[], false),
        ]);

        state.set_cursor(1);
        assert!(state.confirm_current().is_none());
        assert_eq!(state.index, 1);
        assert!(state.free_text_focus);
        state.free_text = "discarded draft".into();

        assert!(state.go_prev_question());
        assert_eq!(state.cursor, 1);
        assert!(!state.free_text_focus);
        assert!(state.go_next_question());
        assert_eq!(state.free_text, "");
        assert!(state.free_text_focus);

        state.free_text = " because ".into();
        let answers = state.confirm_current().expect("all questions answered");
        assert_eq!(answers[0].selected, ["B"]);
        assert_eq!(answers[1].free_text.as_deref(), Some("because"));
    }

    #[test]
    fn multi_question_requires_a_choice_and_collects_selected_labels() {
        let mut state =
            QuestionDialogState::new(vec![question("Pick several", &["A", "B", "C"], true)]);

        assert!(state.confirm_current().is_none());
        state.set_cursor(2);
        state.toggle_multi_at_cursor();
        state.set_cursor(0);
        state.toggle_multi_at_cursor();
        let answers = state.confirm_current().expect("selection completes dialog");

        let selected: HashSet<_> = answers[0].selected.iter().map(String::as_str).collect();
        assert_eq!(selected, HashSet::from(["A", "C"]));
        assert_eq!(answers[0].free_text, None);
    }

    #[test]
    fn slash_suggestions_filter_wrap_hit_test_and_dismiss() {
        let mut state = SlashSuggestState::default();
        state.refresh("/he");
        assert!(state.active);
        assert_eq!(state.current().map(|cmd| cmd.name), Some("/help"));

        state.step(-1);
        assert_eq!(state.selected, state.matches.len() - 1);
        state.list_hit = Some(Rect::new(4, 10, 12, 2));
        state.list_scroll_start = state.matches.len().saturating_sub(1);
        assert_eq!(state.row_index_at(5, 10), Some(state.matches.len() - 1));
        assert_eq!(state.row_index_at(3, 10), None);
        assert_eq!(state.row_index_at(5, 11), None);

        state.dismiss();
        assert!(!state.active);
        assert!(state.matches.is_empty());
        state.refresh("/help now");
        assert!(!state.active, "arguments close command completion");
    }

    #[test]
    fn focus_and_scroll_transitions_preserve_bottom_following_contract() {
        let mut app = app();
        app.toggle_focus();
        assert_eq!(app.focus, FocusPane::Prompt, "empty chat cannot take focus");

        app.add_message(ChatRole::User, "one");
        app.add_message(ChatRole::Assistant, "two");
        app.focus_scrollback();
        assert_eq!(app.focus, FocusPane::Scrollback);
        assert_eq!(app.selected_msg, Some(1));
        assert!(!app.auto_scroll);

        app.chat_scroll_total = 100;
        app.chat_viewport_rows = 20;
        app.scroll_rows(500);
        assert_eq!(app.scroll_offset, 80);
        app.scroll_page(false);
        assert_eq!(app.scroll_offset, 60);
        app.scroll_to_bottom();
        assert_eq!(app.scroll_offset, 0);
        assert!(app.auto_scroll);
        assert_eq!(app.selected_msg, Some(1));
    }

    #[test]
    fn prompt_draft_clear_expands_paste_and_resets_transient_state() {
        let mut app = app();
        let pasted = "one\ntwo\nthree\nfour";
        app.insert_paste_text(pasted);
        app.slash_suggest.active = true;
        app.file_suggest.active = true;
        app.esc_armed_at = Some(Instant::now());

        app.clear_prompt_draft();

        assert_eq!(app.input_history, [pasted]);
        assert_eq!(app.input_history_idx, 1);
        assert!(app.input_buffer.is_empty());
        assert!(app.pending_pastes.is_empty());
        assert!(!app.slash_suggest.active);
        assert!(!app.file_suggest.active);
        assert!(app.esc_armed_at.is_none());
        assert!(app.pending_full_clears >= 1);
    }

    #[test]
    fn insert_paste_requests_two_full_clears() {
        let mut app = app();
        app.pending_full_clears = 0;
        app.insert_paste_text(&"x".repeat(200));
        assert_eq!(app.pending_full_clears, 2);
        assert!(app.needs_redraw);
    }
}
