// ── app.rs: Main application state ────────────────────────────────────
// TuiApp holds all mutable state for the TUI application, including
// the focused mode, dialog stack, session messages, input buffer,
// sidebar visibility, theme, and keybinding context.

use crate::keymap::KeymapContext;
use crate::theme::ThemeName;
use ratatui::layout::Rect;
use std::path::PathBuf;
use std::time::Instant;

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
/// Centralized dialog stack — OpenCode pattern with DialogProvider.
/// Only one dialog is "active" at a time but we track them via an enum.
#[derive(Debug, Clone)]
pub enum DialogKind {
    Provider,
    Model,
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
    /// OpenCode-style tool permission prompt (y/n)
    Permission {
        tool_name: String,
        detail: String,
    },
    SessionList,
    Status,
    Theme,
    Workspace,
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
    /// TODO items.
    pub todos: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarTab {
    Files,
    Diagnostics,
    Mcp,
    Todos,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            visible: false,
            active_tab: SidebarTab::Files,
            file_tree: vec![],
            diagnostics: 0,
            mcp_status: vec![],
            todos: vec![],
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
    /// Cached display row count for `(width, busy_epilogue)` — see
    /// [`ChatMessage::invalidate_layout`].
    pub layout_cache: Option<(u16, bool, usize)>,
}

impl ChatMessage {
    /// Drop cached height so the next layout pass re-measures this message.
    pub fn invalidate_layout(&mut self) {
        self.layout_cache = None;
    }
}

/// How many trailing reasoning lines to show while the block is still streaming.
/// Matches Grok Build default `truncated_lines: 3`.
pub const THINKING_LIVE_TAIL_LINES: usize = 3;

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
        Self {
            text: text.into(),
            started_at: Instant::now(),
            finished_at: None,
            collapsed: true,
        }
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
    /// Grok-style: running is always `Thinking…` (no live timer in the label);
    /// finished is `Thought for Xs`.
    pub fn header_label(&self) -> String {
        if self.is_running() {
            "Thinking…".into()
        } else {
            format!("Thought for {}", self.format_elapsed())
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
        }
        lines
    }

    pub fn is_truncated_live(&self) -> bool {
        self.is_running() && self.collapsed && self.text.lines().count() > THINKING_LIVE_TAIL_LINES
    }
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

/// Tokens currently in context (prompt side) from a provider usage report.
///
/// Cache read/write are billed separately but still occupy the context window.
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
    /// Last drawn frame as cell symbols `[row][col]`, used to build clipboard text.
    pub screen_cells: Vec<Vec<String>>,

    // ── dialogs ──
    pub dialogs: DialogManager,
    pub provider_dialog: ProviderDialogState,
    pub model_selection: ModelSelectionState,
    pub session_list: SessionListState,
    /// Last-paint hit box for the modal `[✗]` control (click = cancel).
    pub dialog_close_hit: Option<Rect>,
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

    // ── slash suggestion popup ──
    pub slash_suggest: SlashSuggestState,

    // ── transient notices ──
    pub toasts: crate::toast::Toasts,
    pub help_scroll: usize,

    // ── sidebar ──
    pub sidebar: SidebarState,

    // ── command ──
    pub command: CommandState,

    // ── theme ──
    pub theme: ThemeName,

    // ── config ──
    pub config: crate::config::TuiAppConfig,

    /// Prompt waiting to be sent to the agent (set by submit / slash commands).
    /// Images for this turn live in `pending_submit_images` until the run loop takes them.
    pub pending_prompt: Option<String>,
    /// Model switch from the picker dialog: `(provider, model)`.
    pub pending_model: Option<(String, String)>,
    /// Session id to load from the DB (picker Enter or `/resume <id>`).
    pub pending_session_id: Option<String>,
    /// Re-fetch `GET /v1/models` for the active provider (config base + key).
    pub pending_catalog_refresh: bool,

    /// Primary agent names for Ctrl+T cycling (build/plan).
    pub primary_agents: Vec<String>,
    pub agent_cycle_idx: usize,

    // ── session chrome (OpenCode status header/footer) ──
    pub provider_name: String,
    pub model_name: String,
    pub agent_name: String,
    /// Current session display title (auto or manual).
    pub session_title: String,
    /// Short project name (basename) for the top status strip.
    pub project_label: String,
    /// Absolute working directory (bottom bar; click-to-copy target).
    pub project_dir: PathBuf,
    /// Current git branch, if the project is a repo.
    pub git_branch: Option<String>,
    /// Clickable cwd path (sticky hover → underline).
    pub cwd_hit: crate::hit::HitArea,
    /// Context-usage meter (hover → bar+%).
    pub context_hit: crate::hit::HitArea,
    /// Turn-status `[stop]` control (click → cancel turn).
    pub turn_stop_hit: crate::hit::HitArea,
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

/// One row of the session list dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    pub id: String,
    pub title: String,
    pub messages: usize,
}

/// Session list dialog state.
#[derive(Debug, Clone, Default)]
pub struct SessionListState {
    pub sessions: Vec<SessionEntry>,
    pub selected: usize,
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
        name: "/undo",
        hint: "Undo last turn via git",
    },
    SlashCommand {
        name: "/redo",
        hint: "Redo the undone turn",
    },
    SlashCommand {
        name: "/compact",
        hint: "Compact the conversation",
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
        name: "/agent",
        hint: "[args] Switch the primary agent (e.g. /agent plan)",
    },
    SlashCommand {
        name: "/connect",
        hint: "Provider / API key help",
    },
    SlashCommand {
        name: "/unshare",
        hint: "Delete local share files",
    },
];

/// Autocomplete state for slash commands while typing.
#[derive(Debug, Clone)]
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

impl Default for SlashSuggestState {
    fn default() -> Self {
        Self {
            active: false,
            matches: Vec::new(),
            selected: 0,
            hovered: None,
            list_hit: None,
            list_scroll_start: 0,
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

    pub fn new(config: crate::config::TuiAppConfig) -> Self {
        Self {
            running: true,
            mode: AppMode::Normal,
            key_context: KeymapContext::Normal,
            focus: FocusPane::Prompt,
            messages: vec![],
            current_agent_state: AgentState::Idle,
            status_message: String::from("Ready — press ? for help"),
            spinner_frame: 0,
            needs_redraw: true,
            input_buffer: String::new(),
            input_lines: vec![],
            input_history: vec![],
            input_history_idx: 0,
            input_cursor: 0,
            esc_armed_at: None,
            pending_images: vec![],
            pending_submit_images: vec![],
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
            screen_cells: Vec::new(),
            dialogs: DialogManager::new(),
            provider_dialog: ProviderDialogState::default(),
            model_selection: ModelSelectionState::default(),
            session_list: SessionListState::default(),
            dialog_close_hit: None,
            dialog_list_hit: None,
            dialog_scrollbar_hit: None,
            dialog_list_scroll_start: 0,
            dialog_list_visible: 0,
            dialog_list_total: 0,
            dialog_scrollbar_grab: None,
            slash_suggest: SlashSuggestState::default(),
            toasts: crate::toast::Toasts::default(),
            help_scroll: 0,
            sidebar: SidebarState::default(),
            command: CommandState::default(),
            theme: config.theme,
            config,
            pending_prompt: None,
            pending_model: None,
            pending_session_id: None,
            pending_catalog_refresh: false,
            primary_agents: vec!["build".into(), "plan".into()],
            agent_cycle_idx: 0,
            provider_name: String::new(),
            model_name: String::new(),
            agent_name: String::from("build"),
            session_title: String::new(),
            project_label: String::from("."),
            project_dir: PathBuf::from("."),
            git_branch: None,
            cwd_hit: crate::hit::HitArea::default(),
            context_hit: crate::hit::HitArea::default(),
            turn_stop_hit: crate::hit::HitArea::default(),
            mouse_pos: None,
            context_used: 0,
            max_context_tokens: 200_000,
            api_context_window: None,
            api_context_for: None,
            turn_started_at: None,
            turn_usage: None,
            pending_cancel: false,
        }
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
            return changed;
        };
        let mut changed = false;
        changed |= self.context_hit.update_hover(c, r);
        changed |= self.cwd_hit.update_hover(c, r);
        changed |= self.turn_stop_hit.update_hover(c, r);
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
        changed
    }

    /// Update context fill from a provider usage event (per LLM step).
    pub fn set_context_from_usage(&mut self, usage: &whycode_core::types::Usage) {
        self.context_used = context_tokens_from_usage(usage);
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
        self.turn_started_at = None;
        if let Some(last) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.role == ChatRole::Assistant)
        {
            last.duration_ms = Some(ms);
        }
        Some(ms)
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.current_agent_state,
            AgentState::Generating | AgentState::Thinking | AgentState::WaitingForPermission
        )
    }

    /// Reset modal mouse targets (call at the start of each dialog paint).
    ///
    /// Does **not** clear `dialog_scrollbar_grab` — a drag spans frames and
    /// must survive repaint.
    pub fn clear_dialog_hits(&mut self) {
        self.dialog_close_hit = None;
        self.dialog_list_hit = None;
        self.dialog_scrollbar_hit = None;
        self.dialog_list_scroll_start = 0;
        self.dialog_list_visible = 0;
        self.dialog_list_total = 0;
    }

    /// Apply hit boxes from a select-style list paint.
    pub fn apply_select_paint(
        &mut self,
        close_hit: Option<Rect>,
        list_area: Option<Rect>,
        scrollbar_hit: Option<Rect>,
        scroll_start: usize,
        visible: usize,
        total: usize,
    ) {
        self.dialog_close_hit = close_hit;
        self.dialog_list_hit = list_area;
        self.dialog_scrollbar_hit = scrollbar_hit;
        self.dialog_list_scroll_start = scroll_start;
        self.dialog_list_visible = visible;
        self.dialog_list_total = total;
    }

    /// Whether `(col, row)` lands on the painted modal close control.
    pub fn dialog_close_contains(&self, col: u16, row: u16) -> bool {
        self.dialog_close_hit
            .map(|h| col >= h.x && col < h.x.saturating_add(h.width) && row == h.y)
            .unwrap_or(false)
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
            self.input_history.push(text.to_string());
            self.input_history_idx = self.input_history.len();
        }
        self.input_buffer.clear();
        self.input_lines.clear();
        self.input_cursor = 0;
        self.pending_images.clear();
        self.slash_suggest.dismiss();
        self.esc_armed_at = None;
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
        if delta > 0 {
            self.scroll_offset = (self.scroll_offset + delta as usize).min(max_off);
        } else {
            let down = (-delta) as usize;
            self.scroll_offset = self.scroll_offset.saturating_sub(down);
        }
        // At bottom (offset 0) always follow the stream — including the no-op
        // case where content already fits the viewport (max_off == 0).
        self.auto_scroll = self.scroll_offset == 0;
        self.mark_dirty();
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
        self.scroll_offset = 0;
        self.auto_scroll = true;
        self.selected_msg = None;
        if session.usage.is_empty() {
            self.context_used = session.token_count() as u64;
            self.turn_usage = None;
        } else {
            self.set_context_from_usage(&session.usage);
            self.turn_usage = Some(session.usage.clone());
        }
        self.mark_dirty();
    }

    /// Add a message to the chat view.
    pub fn add_message(&mut self, role: ChatRole, content: impl Into<String>) {
        self.messages.push(ChatMessage {
            role,
            content: content.into(),
            blocks: vec![],
            results_expanded: false,
            tool_calls: vec![],
            error: None,
            duration_ms: None,
            image_labels: vec![],
            layout_cache: None,
        });
        self.mark_dirty();
    }

    pub fn add_user_message_with_images(
        &mut self,
        content: impl Into<String>,
        image_labels: Vec<String>,
    ) {
        self.messages.push(ChatMessage {
            role: ChatRole::User,
            content: content.into(),
            blocks: vec![],
            results_expanded: false,
            tool_calls: vec![],
            error: None,
            duration_ms: None,
            image_labels,
            layout_cache: None,
        });
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
    pub fn append_thinking(&mut self, text: &str) {
        if let Some(last) = self.messages.last_mut()
            && last.role == ChatRole::Assistant
        {
            // Append to the last *open* thinking block; start a new one after tools/text.
            match last.blocks.last_mut() {
                Some(ChatBlock::Thinking(t)) if t.is_running() => {
                    t.text.push_str(text);
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
        let msg = ChatMessage {
            role: ChatRole::Assistant,
            content: String::new(),
            blocks: vec![ChatBlock::Thinking(ThinkingBlock::new(text))],
            results_expanded: false,
            tool_calls: vec![],
            error: None,
            duration_ms: None,
            image_labels: vec![],
            layout_cache: None,
        };
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
            collapsed: false,
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

        let msg = ChatMessage {
            role: ChatRole::Assistant,
            content: String::new(),
            blocks: vec![ChatBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: arguments.clone(),
            }],
            results_expanded: false,
            tool_calls: vec![tc],
            error: None,
            duration_ms: None,
            image_labels: vec![],
            layout_cache: None,
        };
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
        let text = self.input_buffer.trim().to_string();
        let has_images = !self.pending_images.is_empty();
        if text.is_empty() && !has_images {
            return;
        }
        // Slash commands are handled by the run loop before submit (text-only).
        if text.starts_with('/') && !has_images {
            return;
        }
        if !text.is_empty() {
            self.input_history.push(text.clone());
        }
        let labels: Vec<String> = self
            .pending_images
            .iter()
            .map(|i| i.label.clone())
            .collect();
        let display = if text.is_empty() && has_images {
            // Chat bubble still needs a line of content.
            if labels.len() == 1 {
                format!("[Image: {}]", labels[0])
            } else {
                format!("[Images: {}]", labels.join(", "))
            }
        } else {
            text.clone()
        };
        self.add_user_message_with_images(display, labels);
        self.pending_submit_images = std::mem::take(&mut self.pending_images);
        self.pending_prompt = Some(text);
        self.input_buffer.clear();
        self.input_lines.clear();
        self.input_cursor = 0;
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
            out.push(ChatMessage {
                role: ChatRole::Tool,
                content,
                blocks: vec![],
                results_expanded: false,
                tool_calls: vec![],
                error: None,
                duration_ms: None,
                image_labels: vec![],
                layout_cache: None,
            });
            continue;
        }

        match &msg.content {
            MessageContent::Text(t) => {
                out.push(ChatMessage {
                    role,
                    content: t.clone(),
                    blocks: vec![],
                    results_expanded: false,
                    tool_calls: vec![],
                    error: None,
                    duration_ms: None,
                    image_labels: vec![],
                    layout_cache: None,
                });
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
                    }
                }

                // Assistant text often lives only in Text blocks; keep content for
                // the bubble and blocks for tools/thinking layout.
                out.push(ChatMessage {
                    role,
                    content,
                    blocks: ui_blocks,
                    results_expanded: false,
                    tool_calls,
                    error: None,
                    duration_ms: None,
                    image_labels,
                    layout_cache: None,
                });
            }
        }
    }

    out
}

/// Resolve the current branch name for `dir`, if it is a git work tree.
fn resolve_git_branch(dir: &std::path::Path) -> Option<String> {
    use std::process::Command;

    let out = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() {
        return None;
    }
    // Detached HEAD → short SHA is more useful than the literal "HEAD".
    if name == "HEAD" {
        let sha = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(dir)
            .output()
            .ok()?;
        if sha.status.success() {
            let s = String::from_utf8_lossy(&sha.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    Some(name)
}
