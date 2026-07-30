// ── keymap.rs: Centralized keybinding registry ────────────────────────
// Context-aware bindings (normal / session / dialog modes).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// ── Keybinding Context ─────────────────────────────────────────────────
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeymapContext {
    Normal,
    Dialog,
    Command,
    Session,
    Help,
}

// ── Named Actions ──────────────────────────────────────────────────────
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    ToggleHelp,
    EnterCommand,
    EscapeMode,
    SubmitInput,
    ScrollUp,
    ScrollDown,
    ScrollPageUp,
    ScrollPageDown,
    ScrollToBottom,
    ScrollToTop,
    ToggleAutoScroll,
    ClearSession,
    ToggleSidebar,
    OpenProviderDialog,
    OpenModelDialog,
    DialogConfirm,
    DialogCancel,
    DialogNextField,
    DialogPrevField,
    DialogSelect,
    InputBackspace,
    InputDelete,
    InputHome,
    InputEnd,
    InputLeft,
    InputRight,
    InputHistoryPrev,
    InputHistoryNext,
    InputNewline,
    InputClear,
    ToggleToolCall,
    ToggleThinking,
    ToggleToolResult,
    /// Cycle primary agents (OpenCode Tab)
    SwitchAgent,
}

/// A single keybinding description for the help overlay.
#[derive(Debug, Clone)]
pub struct KeyBinding {
    pub key: String,
    pub description: String,
    pub context: KeymapContext,
}

impl KeyBinding {
    pub fn new(
        key: impl Into<String>,
        description: impl Into<String>,
        context: KeymapContext,
    ) -> Self {
        Self {
            key: key.into(),
            description: description.into(),
            context,
        }
    }
}

/// Returns all keybindings for a given context (for help display).
pub fn bindings_for_context(context: KeymapContext) -> Vec<KeyBinding> {
    let mut all = vec![
        KeyBinding::new("Ctrl+C", "Quit", KeymapContext::Normal),
        KeyBinding::new("Ctrl+Q", "Force quit", KeymapContext::Normal),
    ];

    match context {
        KeymapContext::Normal | KeymapContext::Session => {
            all.extend(normal_bindings());
        }
        KeymapContext::Dialog => {
            all.extend(dialog_bindings());
        }
        KeymapContext::Command => {
            all.extend(command_bindings());
        }
        KeymapContext::Help => {
            all.extend(help_bindings());
        }
    }
    all
}

// ── Keymap ─────────────────────────────────────────────────────────────
pub struct Keymap;

impl Default for Keymap {
    fn default() -> Self {
        Self::new()
    }
}

impl Keymap {
    pub fn new() -> Self {
        Self
    }

    pub fn resolve(&self, ctx: KeymapContext, key: &KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match ctx {
            KeymapContext::Normal | KeymapContext::Session => match (ctrl, key.code) {
                // Global
                (true, KeyCode::Char('c')) => Some(Action::Quit),
                (false, KeyCode::Char('q')) => Some(Action::Quit),
                (false, KeyCode::Char('?')) => Some(Action::ToggleHelp),
                (false, KeyCode::Char(':')) => Some(Action::EnterCommand),
                (false, KeyCode::Esc) => Some(Action::EscapeMode),
                // Session / Chat
                (false, KeyCode::Enter) => Some(Action::SubmitInput),
                (false, KeyCode::Up) => Some(Action::ScrollDown),
                (false, KeyCode::Down) => Some(Action::ScrollUp),
                (false, KeyCode::Char('k')) => Some(Action::ScrollDown),
                (false, KeyCode::Char('j')) => Some(Action::ScrollUp),
                (false, KeyCode::PageUp) => Some(Action::ScrollPageUp),
                (false, KeyCode::PageDown) => Some(Action::ScrollPageDown),
                (false, KeyCode::Home) => Some(Action::ScrollToTop),
                (false, KeyCode::End) => Some(Action::ScrollToBottom),
                (true, KeyCode::Char('b')) => Some(Action::ToggleSidebar),
                (true, KeyCode::Char('p')) => Some(Action::OpenProviderDialog),
                (true, KeyCode::Char('m')) => Some(Action::OpenModelDialog),
                (true, KeyCode::Char('a')) => Some(Action::ToggleAutoScroll),
                (true, KeyCode::Char('l')) => Some(Action::ClearSession),
                // OpenCode: Tab cycles primary agents
                (false, KeyCode::Tab) => Some(Action::SwitchAgent),
                // Input editing
                (false, KeyCode::Backspace) => Some(Action::InputBackspace),
                (false, KeyCode::Delete) => Some(Action::InputDelete),
                (false, KeyCode::Left) => Some(Action::InputLeft),
                (false, KeyCode::Right) => Some(Action::InputRight),
                (true, KeyCode::Char('u')) => Some(Action::InputClear),
                _ => None,
            },
            KeymapContext::Dialog => match (ctrl, key.code) {
                (false, KeyCode::Esc) => Some(Action::DialogCancel),
                (false, KeyCode::Char('q')) => Some(Action::DialogCancel),
                (false, KeyCode::Enter) => Some(Action::DialogConfirm),
                (false, KeyCode::Tab) => Some(Action::DialogNextField),
                (false, KeyCode::BackTab) => Some(Action::DialogPrevField),
                (false, KeyCode::Up) => Some(Action::DialogPrevField),
                (false, KeyCode::Down) => Some(Action::DialogNextField),
                (false, KeyCode::Char('y')) => Some(Action::DialogConfirm),
                (false, KeyCode::Char('n')) => Some(Action::DialogCancel),
                (false, KeyCode::Char('a')) => Some(Action::DialogConfirm), // allow
                (false, KeyCode::Char('d')) => Some(Action::DialogCancel),  // deny
                (false, KeyCode::Backspace) => Some(Action::InputBackspace),
                (true, KeyCode::Char('s')) => Some(Action::DialogConfirm),
                (true, KeyCode::Char('c')) => Some(Action::DialogCancel),
                _ => None,
            },
            KeymapContext::Command => match (ctrl, key.code) {
                (false, KeyCode::Enter) => Some(Action::SubmitInput),
                (false, KeyCode::Esc) => Some(Action::EscapeMode),
                (false, KeyCode::Backspace) => Some(Action::InputBackspace),
                _ => None,
            },
            KeymapContext::Help => match key.code {
                KeyCode::Esc => Some(Action::EscapeMode),
                KeyCode::Char('q') => Some(Action::EscapeMode),
                KeyCode::Char('?') => Some(Action::ToggleHelp),
                _ => None,
            },
        }
    }
}

// ── Binding lists for help display ─────────────────────────────────────
fn normal_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("?", "Toggle help", KeymapContext::Normal),
        KeyBinding::new(":", "Enter command mode", KeymapContext::Normal),
        KeyBinding::new("Esc", "Clear input / exit mode", KeymapContext::Normal),
        KeyBinding::new("Enter", "Send message", KeymapContext::Normal),
        KeyBinding::new("Up/k, Down/j", "Scroll chat history", KeymapContext::Normal),
        KeyBinding::new("PgUp/PgDn", "Page scroll", KeymapContext::Normal),
        KeyBinding::new("Home/End", "Jump to top/bottom", KeymapContext::Normal),
        KeyBinding::new("Ctrl+P", "Provider setup", KeymapContext::Normal),
        KeyBinding::new("Ctrl+M", "Model selection", KeymapContext::Normal),
        KeyBinding::new("Ctrl+B", "Toggle sidebar", KeymapContext::Normal),
        KeyBinding::new("Ctrl+A", "Toggle auto scroll", KeymapContext::Normal),
        KeyBinding::new("Ctrl+L", "Clear session", KeymapContext::Normal),
    ]
}

fn dialog_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("Esc/q", "Close dialog", KeymapContext::Dialog),
        KeyBinding::new("Enter/y", "Confirm / Select", KeymapContext::Dialog),
        KeyBinding::new("Tab/Up/Down", "Navigate", KeymapContext::Dialog),
        KeyBinding::new("Ctrl+S", "Save (provider form)", KeymapContext::Dialog),
        KeyBinding::new("Ctrl+C", "Cancel", KeymapContext::Dialog),
    ]
}

fn command_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("Enter", "Execute command", KeymapContext::Command),
        KeyBinding::new("Esc", "Exit command mode", KeymapContext::Command),
        KeyBinding::new(":q", "Quit", KeymapContext::Command),
        KeyBinding::new(":provider", "Open provider dialog", KeymapContext::Command),
        KeyBinding::new(":model", "Open model dialog", KeymapContext::Command),
        KeyBinding::new(":theme", "Change theme", KeymapContext::Command),
        KeyBinding::new(":clear", "Clear session", KeymapContext::Command),
        KeyBinding::new(":sidebar", "Toggle sidebar", KeymapContext::Command),
    ]
}

fn help_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("Esc/?/q", "Close help", KeymapContext::Help),
        KeyBinding::new("Up/Down", "Scroll help", KeymapContext::Help),
    ]
}
