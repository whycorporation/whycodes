// ── app.rs: Main application state ────────────────────────────────────
// TuiApp holds all mutable state for the TUI application, including
// the focused mode, dialog stack, session messages, input buffer,
// sidebar visibility, theme, and keybinding context.

use crate::keymap::KeymapContext;
use crate::theme::ThemeName;

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Whether thinking text is collapsed.
    pub thinking_collapsed: bool,
    /// Whether tool results are expanded beyond truncation.
    pub results_expanded: bool,
    /// Tool calls that were made in this message.
    pub tool_calls: Vec<ChatToolCall>,
    /// Error associated with this message.
    pub error: Option<String>,
}

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
    Thinking(String),
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

// ── TuiApp ─────────────────────────────────────────────────────────────
pub struct TuiApp {
    // ── runtime ──
    pub running: bool,
    pub mode: AppMode,
    pub key_context: KeymapContext,

    // ── session ──
    pub messages: Vec<ChatMessage>,
    pub current_agent_state: AgentState,
    pub status_message: String,

    // ── input ──
    pub input_buffer: String,
    /// Multi-line input lines beyond the first.
    pub input_lines: Vec<String>,
    pub input_history: Vec<String>,
    pub input_history_idx: usize,
    /// Cursor column in the current input line.
    pub input_cursor: usize,

    // ── scroll ──
    pub scroll_offset: usize,
    pub auto_scroll: bool,

    // ── dialogs ──
    pub dialogs: DialogManager,
    pub provider_dialog: ProviderDialogState,
    pub model_selection: ModelSelectionState,
    pub session_list: SessionListState,
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
    pub pending_prompt: Option<String>,

    /// Primary agent names for Tab cycling (OpenCode build/plan).
    pub primary_agents: Vec<String>,
    pub agent_cycle_idx: usize,

    // ── session chrome (OpenCode status header/footer) ──
    pub provider_name: String,
    pub model_name: String,
    pub agent_name: String,
    pub project_label: String,
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
    pub fn new(config: crate::config::TuiAppConfig) -> Self {
        Self {
            running: true,
            mode: AppMode::Normal,
            key_context: KeymapContext::Normal,
            messages: vec![],
            current_agent_state: AgentState::Idle,
            status_message: String::from("Ready — press ? for help"),
            input_buffer: String::new(),
            input_lines: vec![],
            input_history: vec![],
            input_history_idx: 0,
            input_cursor: 0,
            scroll_offset: 0,
            auto_scroll: true,
            dialogs: DialogManager::new(),
            provider_dialog: ProviderDialogState::default(),
            model_selection: ModelSelectionState::default(),
            session_list: SessionListState::default(),
            help_scroll: 0,
            sidebar: SidebarState::default(),
            command: CommandState::default(),
            theme: config.theme,
            config,
            pending_prompt: None,
            primary_agents: vec!["build".into(), "plan".into()],
            agent_cycle_idx: 0,
            provider_name: String::new(),
            model_name: String::new(),
            agent_name: String::from("build"),
            project_label: String::from("."),
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

    /// Add a message to the chat view.
    pub fn add_message(&mut self, role: ChatRole, content: impl Into<String>) {
        self.messages.push(ChatMessage {
            role,
            content: content.into(),
            blocks: vec![],
            thinking_collapsed: true,
            results_expanded: false,
            tool_calls: vec![],
            error: None,
        });
    }

    /// Append text to the last assistant message (streaming).
    pub fn append_to_last(&mut self, text: &str) {
        if let Some(last) = self.messages.last_mut() {
            if last.role == ChatRole::Assistant {
                last.content.push_str(text);
            } else {
                self.add_message(ChatRole::Assistant, text);
            }
        } else {
            self.add_message(ChatRole::Assistant, text);
        }
    }

    /// Append a thinking block to the last assistant message.
    pub fn append_thinking(&mut self, text: &str) {
        if let Some(last) = self.messages.last_mut()
            && last.role == ChatRole::Assistant
        {
            // Append to the last thinking block or create one.
            if let Some(ChatBlock::Thinking(t)) = last.blocks.last_mut() {
                t.push_str(text);
            } else {
                last.blocks.push(ChatBlock::Thinking(text.to_string()));
            }
            return;
        }
        // No assistant message yet — create one.
        let msg = ChatMessage {
            role: ChatRole::Assistant,
            content: String::new(),
            blocks: vec![ChatBlock::Thinking(text.to_string())],
            thinking_collapsed: true,
            results_expanded: false,
            tool_calls: vec![],
            error: None,
        };
        self.messages.push(msg);
    }

    /// Add a tool-call to the last assistant message.
    pub fn add_tool_call(&mut self, id: String, name: String, arguments: serde_json::Value) {
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
            thinking_collapsed: true,
            results_expanded: false,
            tool_calls: vec![tc],
            error: None,
        };
        self.messages.push(msg);
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
                    return;
                }
            }
        }
    }

    /// Submit current input as user message and queue it for the agent.
    pub fn submit_input(&mut self) {
        let text = self.input_buffer.trim().to_string();
        if text.is_empty() {
            return;
        }
        // Slash commands are handled by the run loop before submit.
        if text.starts_with('/') {
            return;
        }
        self.input_history.push(text.clone());
        self.add_message(ChatRole::User, text.clone());
        self.pending_prompt = Some(text);
        self.input_buffer.clear();
        self.input_lines.clear();
        self.input_cursor = 0;
        self.input_history_idx = self.input_history.len();
        self.auto_scroll = true;
        self.scroll_offset = 0;
    }
}
