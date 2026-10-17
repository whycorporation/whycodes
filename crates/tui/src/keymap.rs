// ── keymap.rs: Centralized keybinding registry ────────────────────────
// Context-aware bindings (normal / session / dialog modes).
// Focus-aware: Prompt vs Scrollback vs overflowing Todos.

use std::collections::HashMap;

use crate::app::FocusPane;
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
    /// Ctrl+Q — skip the quit confirm and leave immediately.
    ForceQuit,
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
    SidebarNextTab,
    SidebarPrevTab,
    /// Jump to sidebar tab 1–6 (scrollback; prompt still types digits).
    SidebarTab1,
    SidebarTab2,
    SidebarTab3,
    SidebarTab4,
    SidebarTab5,
    SidebarTab6,
    /// Ctrl+G — sticky tasks panel (subagents + background jobs).
    ToggleTasksPane,
    /// Open the framed child transcript (Enter on a subagent row).
    OpenSubagent,
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
    /// Delete the previous whitespace-delimited word (Ctrl+W / Ctrl+Backspace).
    InputKillWordBack,
    /// Delete the next whitespace-delimited word (Ctrl+Delete / Alt+D).
    InputKillWordForward,
    /// Move to the previous word start (Ctrl+Left / Alt+B).
    InputWordLeft,
    /// Move to the next word start (Ctrl+Right / Alt+F).
    InputWordRight,
    ToggleToolCall,
    ToggleThinking,
    ToggleToolResult,
    /// Fold / unfold the sticky todo panel (`t` in scrollback).
    ToggleTodosPanel,
    /// Cycle primary agents (Ctrl+T; Tab is focus toggle)
    SwitchAgent,
    /// Tab: Prompt ↔ Todos (when overflowing) ↔ Scrollback
    ToggleFocus,
    /// Ctrl+Space: open the `@file` picker at the cursor.
    FileComplete,
    FocusPrompt,
    FocusScrollback,
    /// Move selection in scrollback (j/k)
    SelectPrev,
    SelectNext,
    /// Jump to prev/next user turn (Shift+Left/Right)
    JumpPrevTurn,
    JumpNextTurn,
    /// Copy selected message (y in scrollback)
    CopySelection,
    /// Ctrl+V — attach a bitmap from the OS clipboard (screenshot / browser copy).
    /// Text paste stays on bracketed `Event::Paste` so hosts that intercept
    /// Ctrl+V do not double-insert.
    PasteClipboard,
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

// ── User bindings ([tui.key_bindings]) ────────────────────────────────

/// Config names for actions a user may bind. Named by what the key does,
/// so `scroll_up` is [`Action::ScrollDown`] (the offset counts from the bottom).
/// Dialog actions are not bindable: approval prompts keep fixed keys.
const ACTION_NAMES: &[(&str, Action)] = &[
    ("quit", Action::Quit),
    ("force_quit", Action::ForceQuit),
    ("escape", Action::EscapeMode),
    ("submit", Action::SubmitInput),
    ("scroll_up", Action::ScrollDown),
    ("scroll_down", Action::ScrollUp),
    ("page_up", Action::ScrollPageUp),
    ("page_down", Action::ScrollPageDown),
    ("scroll_to_top", Action::ScrollToTop),
    ("scroll_to_bottom", Action::ScrollToBottom),
    ("toggle_auto_scroll", Action::ToggleAutoScroll),
    ("clear_session", Action::ClearSession),
    ("toggle_sidebar", Action::ToggleSidebar),
    ("sidebar_next_tab", Action::SidebarNextTab),
    ("sidebar_prev_tab", Action::SidebarPrevTab),
    ("sidebar_tab_1", Action::SidebarTab1),
    ("sidebar_tab_2", Action::SidebarTab2),
    ("sidebar_tab_3", Action::SidebarTab3),
    ("sidebar_tab_4", Action::SidebarTab4),
    ("sidebar_tab_5", Action::SidebarTab5),
    ("sidebar_tab_6", Action::SidebarTab6),
    ("toggle_tasks", Action::ToggleTasksPane),
    ("provider_dialog", Action::OpenProviderDialog),
    ("model_dialog", Action::OpenModelDialog),
    ("input_home", Action::InputHome),
    ("input_end", Action::InputEnd),
    ("input_left", Action::InputLeft),
    ("input_right", Action::InputRight),
    ("history_prev", Action::InputHistoryPrev),
    ("history_next", Action::InputHistoryNext),
    ("newline", Action::InputNewline),
    ("clear_input", Action::InputClear),
    ("kill_word_back", Action::InputKillWordBack),
    ("kill_word_forward", Action::InputKillWordForward),
    ("word_left", Action::InputWordLeft),
    ("word_right", Action::InputWordRight),
    ("toggle_thinking", Action::ToggleThinking),
    ("toggle_tool_result", Action::ToggleToolResult),
    ("toggle_todos", Action::ToggleTodosPanel),
    ("switch_agent", Action::SwitchAgent),
    ("toggle_focus", Action::ToggleFocus),
    ("file_complete", Action::FileComplete),
    ("focus_prompt", Action::FocusPrompt),
    ("select_prev", Action::SelectPrev),
    ("select_next", Action::SelectNext),
    ("prev_turn", Action::JumpPrevTurn),
    ("next_turn", Action::JumpNextTurn),
    ("copy_selection", Action::CopySelection),
    ("paste_clipboard", Action::PasteClipboard),
];

/// Config name for `action` (`toggle_sidebar`); actions with no config name
/// fall back to their snake-cased variant name.
pub fn action_name(action: Action) -> String {
    if let Some((name, _)) = ACTION_NAMES.iter().find(|(_, a)| *a == action) {
        return (*name).to_string();
    }
    let mut out = String::new();
    for (i, c) in format!("{action:?}").chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// Bindable action for a config name (`"none"` is handled by the caller).
pub fn action_from_name(name: &str) -> Option<Action> {
    let name = name.trim().to_ascii_lowercase().replace('-', "_");
    ACTION_NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, action)| *action)
}

/// A key plus Ctrl / Alt / Shift. Letters are stored lowercase with Shift
/// set for capitals; Shift is dropped for other characters (`?`, `+`) since
/// terminals disagree on reporting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    code: KeyCode,
    mods: KeyModifiers,
}

impl Chord {
    fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        let mut mods = mods & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        let code = match code {
            KeyCode::Char(c) if c.is_ascii_uppercase() => {
                mods |= KeyModifiers::SHIFT;
                KeyCode::Char(c.to_ascii_lowercase())
            }
            KeyCode::Char(c) => {
                if !c.is_ascii_alphabetic() {
                    mods -= KeyModifiers::SHIFT;
                }
                KeyCode::Char(c)
            }
            KeyCode::BackTab => {
                mods |= KeyModifiers::SHIFT;
                KeyCode::Tab
            }
            other => other,
        };
        Self { code, mods }
    }

    fn from_event(key: &KeyEvent) -> Self {
        Self::new(key.code, key.modifiers)
    }

    pub fn code(&self) -> KeyCode {
        self.code
    }

    pub fn mods(&self) -> KeyModifiers {
        self.mods
    }

    /// The key event a terminal sends for this chord (capitals for Shift+letter).
    pub fn to_event(self) -> KeyEvent {
        let code = match self.code {
            KeyCode::Char(c) if self.mods.contains(KeyModifiers::SHIFT) => {
                KeyCode::Char(c.to_ascii_uppercase())
            }
            other => other,
        };
        KeyEvent::new(code, self.mods)
    }

    /// Config spelling: `ctrl+shift+t`, `alt+enter`, `pageup`, `f2`.
    pub fn label(&self) -> String {
        let mut out = String::new();
        for (flag, name) in [
            (KeyModifiers::CONTROL, "ctrl+"),
            (KeyModifiers::ALT, "alt+"),
            (KeyModifiers::SHIFT, "shift+"),
        ] {
            if self.mods.contains(flag) {
                out.push_str(name);
            }
        }
        match self.code {
            KeyCode::Char(' ') => out.push_str("space"),
            KeyCode::Char(c) => out.push(c),
            KeyCode::F(n) => out.push_str(&format!("f{n}")),
            KeyCode::PageUp => out.push_str("pageup"),
            KeyCode::PageDown => out.push_str("pagedown"),
            other => out.push_str(&format!("{other:?}").to_ascii_lowercase()),
        }
        out
    }

    /// Parse `ctrl+k`, `ctrl-shift-g`, `alt+enter`, `f2`, `G`, `ctrl++`.
    pub fn parse(spec: &str) -> Option<Self> {
        let spec = spec.trim();
        let split = spec
            .char_indices()
            .rev()
            .skip(1)
            .find(|(_, c)| *c == '+' || *c == '-');
        let (mods_part, key) = match split {
            Some((i, _)) => (&spec[..i], &spec[i + 1..]),
            None => ("", spec),
        };
        let mut mods = KeyModifiers::NONE;
        for m in mods_part.split(['+', '-']).filter(|m| !m.is_empty()) {
            mods |= match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" | "meta" | "option" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                _ => return None,
            };
        }
        Some(Self::new(key_code_from_name(key)?, mods))
    }
}

fn key_code_from_name(key: &str) -> Option<KeyCode> {
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(KeyCode::Char(c));
    }
    let lower = key.to_ascii_lowercase();
    let code = match lower.as_str() {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "space" => KeyCode::Char(' '),
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        _ => {
            let n: u8 = lower.strip_prefix('f')?.parse().ok()?;
            if !(1..=24).contains(&n) {
                return None;
            }
            KeyCode::F(n)
        }
    };
    Some(code)
}

// ── Keymap ─────────────────────────────────────────────────────────────

/// Default bindings plus `[tui.key_bindings]` overrides.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    /// Chord → action; `None` unbinds the chord.
    overrides: HashMap<Chord, Option<Action>>,
}

impl Keymap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build from `[tui.key_bindings]` (`"ctrl+o" = "toggle_sidebar"`,
    /// `"ctrl+l" = "none"`). Unknown keys or actions are logged and skipped.
    pub fn from_bindings(bindings: &HashMap<String, String>) -> Self {
        let mut overrides = HashMap::new();
        for (key, name) in bindings {
            let Some(chord) = Chord::parse(key) else {
                tracing::warn!(key = %key, "[tui.key_bindings] unknown key");
                continue;
            };
            let action = match name.trim().to_ascii_lowercase().as_str() {
                "none" | "unbind" | "" => None,
                _ => match action_from_name(name) {
                    Some(action) => Some(action),
                    None => {
                        tracing::warn!(key = %key, action = %name, "[tui.key_bindings] unknown action");
                        continue;
                    }
                },
            };
            overrides.insert(chord, action);
        }
        Self { overrides }
    }

    /// A user binding for `key`, if any. Plain printable keys only apply
    /// outside the prompt so typing is never captured.
    fn custom(&self, focus: FocusPane, key: &KeyEvent) -> Option<Option<Action>> {
        let chord = Chord::from_event(key);
        let printable = matches!(chord.code, KeyCode::Char(_))
            && !chord
                .mods
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if printable && focus == FocusPane::Prompt {
            return None;
        }
        self.overrides.get(&chord).copied()
    }

    pub fn resolve(&self, ctx: KeymapContext, focus: FocusPane, key: &KeyEvent) -> Option<Action> {
        if matches!(ctx, KeymapContext::Normal | KeymapContext::Session)
            && let Some(found) = self.custom(focus, key)
        {
            return found;
        }
        self.resolve_default(ctx, focus, key)
    }

    fn resolve_default(
        &self,
        ctx: KeymapContext,
        focus: FocusPane,
        key: &KeyEvent,
    ) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        match ctx {
            KeymapContext::Normal | KeymapContext::Session => {
                // Shift/Alt+Enter insert a newline. Shift needs a terminal
                // with the Kitty keyboard protocol; Alt works everywhere.
                if key.code == KeyCode::Enter
                    && key
                        .modifiers
                        .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT)
                {
                    return Some(Action::InputNewline);
                }
                // Global chords first (both focus panes)
                match (ctrl, key.code) {
                    (true, KeyCode::Char('c')) => return Some(Action::Quit),
                    (true, KeyCode::Char('q')) => return Some(Action::ForceQuit),
                    (true, KeyCode::Char('v')) => return Some(Action::PasteClipboard),
                    (false, KeyCode::Esc) => return Some(Action::EscapeMode),
                    (true, KeyCode::Char('b')) => return Some(Action::ToggleSidebar),
                    (true, KeyCode::Char('g')) => return Some(Action::ToggleTasksPane),
                    // Ctrl+. / Ctrl+, cycle tabs from either focus (prompt still
                    // types bare `.` / `,`). Avoid Ctrl+[ (CSI/Esc) and Ctrl+Tab
                    // (MRU session switch in the run loop).
                    (true, KeyCode::Char('.')) => return Some(Action::SidebarNextTab),
                    (true, KeyCode::Char(',')) => return Some(Action::SidebarPrevTab),
                    // Sidebar tabs: [ / ] in scrollback (prompt still types them).
                    (false, KeyCode::Char(']')) if focus == FocusPane::Scrollback => {
                        return Some(Action::SidebarNextTab);
                    }
                    (false, KeyCode::Char('[')) if focus == FocusPane::Scrollback => {
                        return Some(Action::SidebarPrevTab);
                    }
                    (false, KeyCode::Char('1')) if focus == FocusPane::Scrollback => {
                        return Some(Action::SidebarTab1);
                    }
                    (false, KeyCode::Char('2')) if focus == FocusPane::Scrollback => {
                        return Some(Action::SidebarTab2);
                    }
                    (false, KeyCode::Char('3')) if focus == FocusPane::Scrollback => {
                        return Some(Action::SidebarTab3);
                    }
                    (false, KeyCode::Char('4')) if focus == FocusPane::Scrollback => {
                        return Some(Action::SidebarTab4);
                    }
                    (false, KeyCode::Char('5')) if focus == FocusPane::Scrollback => {
                        return Some(Action::SidebarTab5);
                    }
                    (false, KeyCode::Char('6')) if focus == FocusPane::Scrollback => {
                        return Some(Action::SidebarTab6);
                    }
                    (true, KeyCode::Char('p')) => return Some(Action::OpenProviderDialog),
                    (true, KeyCode::Char('m')) => return Some(Action::OpenModelDialog),
                    (true, KeyCode::Char('a')) => return Some(Action::ToggleAutoScroll),
                    (true, KeyCode::Char('l')) => return Some(Action::ClearSession),
                    // Tab toggles focus; Ctrl+T cycles agent (was bare Tab)
                    (false, KeyCode::Tab) => return Some(Action::ToggleFocus),
                    (true, KeyCode::Char('t')) => return Some(Action::SwitchAgent),
                    // Ctrl+Space arrives as Char(' ') on most terminals, Null on a few.
                    (true, KeyCode::Char(' ')) | (true, KeyCode::Null) => {
                        return Some(Action::FileComplete);
                    }
                    // Page scroll works from either focus (PgUp/Dn from prompt)
                    (false, KeyCode::PageUp) => return Some(Action::ScrollPageUp),
                    (false, KeyCode::PageDown) => return Some(Action::ScrollPageDown),
                    (false, KeyCode::Home)
                        if focus == FocusPane::Scrollback || focus == FocusPane::Todos =>
                    {
                        return Some(Action::ScrollToTop);
                    }
                    (false, KeyCode::End)
                        if focus == FocusPane::Scrollback || focus == FocusPane::Todos =>
                    {
                        return Some(Action::ScrollToBottom);
                    }
                    _ => {}
                }

                match focus {
                    FocusPane::Prompt => {
                        // Readline / IDE word editing. Prompt-only so Ctrl+W in
                        // `/sessions` still closes a live row (dialog handler).
                        if alt && !ctrl {
                            match key.code {
                                KeyCode::Backspace => return Some(Action::InputKillWordBack),
                                KeyCode::Char('b') => return Some(Action::InputWordLeft),
                                KeyCode::Char('f') => return Some(Action::InputWordRight),
                                KeyCode::Char('d') => return Some(Action::InputKillWordForward),
                                _ => {}
                            }
                        }
                        match (ctrl, shift, key.code) {
                            (false, _, KeyCode::Enter) => Some(Action::SubmitInput),
                            // Arrows edit the draft; Up on empty → history
                            (false, false, KeyCode::Up) => Some(Action::InputHistoryPrev),
                            (false, false, KeyCode::Down) => Some(Action::InputHistoryNext),
                            (false, false, KeyCode::Left) => Some(Action::InputLeft),
                            (false, false, KeyCode::Right) => Some(Action::InputRight),
                            // Shift+arrows: turn jump without leaving prompt
                            (false, true, KeyCode::Left) => Some(Action::JumpPrevTurn),
                            (false, true, KeyCode::Right) => Some(Action::JumpNextTurn),
                            (false, _, KeyCode::Home) => Some(Action::InputHome),
                            (false, _, KeyCode::End) => Some(Action::InputEnd),
                            (false, _, KeyCode::Backspace) => Some(Action::InputBackspace),
                            (false, _, KeyCode::Delete) => Some(Action::InputDelete),
                            (true, _, KeyCode::Char('u')) => Some(Action::InputClear),
                            (true, _, KeyCode::Char('w')) => Some(Action::InputKillWordBack),
                            (true, _, KeyCode::Backspace) => Some(Action::InputKillWordBack),
                            (true, _, KeyCode::Delete) => Some(Action::InputKillWordForward),
                            (true, _, KeyCode::Left) => Some(Action::InputWordLeft),
                            (true, _, KeyCode::Right) => Some(Action::InputWordRight),
                            // Ctrl+Up/Down: pure scroll while typing
                            (true, _, KeyCode::Up) => Some(Action::ScrollDown),
                            (true, _, KeyCode::Down) => Some(Action::ScrollUp),
                            _ => None,
                        }
                    }
                    FocusPane::Scrollback => match (ctrl, shift, key.code) {
                        (false, false, KeyCode::Enter) => Some(Action::FocusPrompt),
                        (false, false, KeyCode::Char(' ')) => Some(Action::FocusPrompt),
                        // Do not bind printable `i`: unbracketed paste and
                        // "any letter returns to the prompt"
                        // would swallow every ASCII i (Turkish `iyi` → `y`).
                        // Tab / Space / Enter / Esc / Backspace still return.
                        (false, false, KeyCode::Up) | (false, false, KeyCode::Char('k')) => {
                            Some(Action::SelectPrev)
                        }
                        (false, false, KeyCode::Down) | (false, false, KeyCode::Char('j')) => {
                            Some(Action::SelectNext)
                        }
                        (true, _, KeyCode::Up) | (true, _, KeyCode::Char('k')) => {
                            Some(Action::ScrollDown)
                        }
                        (true, _, KeyCode::Down) | (true, _, KeyCode::Char('j')) => {
                            Some(Action::ScrollUp)
                        }
                        (false, true, KeyCode::Left) | (false, true, KeyCode::Char('h')) => {
                            Some(Action::JumpPrevTurn)
                        }
                        (false, true, KeyCode::Right) | (false, true, KeyCode::Char('l')) => {
                            Some(Action::JumpNextTurn)
                        }
                        (false, false, KeyCode::Char('y')) => Some(Action::CopySelection),
                        (false, false, KeyCode::Char('t')) => Some(Action::ToggleTodosPanel),
                        (false, false, KeyCode::Char('e')) => Some(Action::ToggleThinking),
                        (false, false, KeyCode::Char('h')) => Some(Action::ToggleThinking),
                        (false, false, KeyCode::Char('l')) => Some(Action::ToggleToolResult),
                        (false, false, KeyCode::Char('g')) => Some(Action::ScrollToTop),
                        (false, true, KeyCode::Char('g')) | (false, true, KeyCode::Char('G')) => {
                            Some(Action::ScrollToBottom)
                        }
                        (false, false, KeyCode::Char('G')) => Some(Action::ScrollToBottom),
                        (false, false, KeyCode::Backspace) => Some(Action::FocusPrompt),
                        _ => None,
                    },
                    FocusPane::Todos => match (ctrl, shift, key.code) {
                        (false, false, KeyCode::Enter)
                        | (false, false, KeyCode::Char(' '))
                        | (false, false, KeyCode::Backspace) => Some(Action::FocusPrompt),
                        (false, false, KeyCode::Up) | (false, false, KeyCode::Char('k')) => {
                            Some(Action::ScrollDown)
                        }
                        (false, false, KeyCode::Down) | (false, false, KeyCode::Char('j')) => {
                            Some(Action::ScrollUp)
                        }
                        (false, false, KeyCode::Char('t')) => Some(Action::ToggleTodosPanel),
                        (false, false, KeyCode::Char('g')) => Some(Action::ScrollToTop),
                        (false, true, KeyCode::Char('g')) | (false, true, KeyCode::Char('G')) => {
                            Some(Action::ScrollToBottom)
                        }
                        (false, false, KeyCode::Char('G')) => Some(Action::ScrollToBottom),
                        _ => None,
                    },
                }
            }
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
                (true, KeyCode::Char('v')) => Some(Action::PasteClipboard),
                // Second Ctrl+C on the quit confirm means "yes, leave".
                (true, KeyCode::Char('c')) => Some(Action::Quit),
                (true, KeyCode::Char('q')) => Some(Action::ForceQuit),
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
                _ => None,
            },
        }
    }
}

// ── Binding lists for help display ─────────────────────────────────────
fn normal_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("/help", "Open help", KeymapContext::Normal),
        KeyBinding::new(
            "Tab",
            "Focus prompt ↔ todos ↔ scrollback",
            KeymapContext::Normal,
        ),
        KeyBinding::new("Ctrl+T", "Cycle primary agent", KeymapContext::Normal),
        KeyBinding::new(
            "Esc",
            "Cancel turn / double-Esc clear draft",
            KeymapContext::Normal,
        ),
        KeyBinding::new("Enter", "Send message (prompt)", KeymapContext::Normal),
        KeyBinding::new(
            "Shift+Enter",
            "While a turn runs: queue for after it (/queue)",
            KeymapContext::Normal,
        ),
        KeyBinding::new(
            "Ctrl+Enter",
            "While a turn runs: interrupt between tool calls (/now)",
            KeymapContext::Normal,
        ),
        KeyBinding::new(
            "j/k · ↑/↓",
            "Select message (scrollback) / scroll todos",
            KeymapContext::Normal,
        ),
        KeyBinding::new("Ctrl+↑/↓", "Scroll transcript", KeymapContext::Normal),
        KeyBinding::new("PgUp/PgDn", "Page scroll", KeymapContext::Normal),
        KeyBinding::new(
            "g / G",
            "Top / bottom (scrollback / todos)",
            KeymapContext::Normal,
        ),
        KeyBinding::new("Shift+←/→", "Prev / next user turn", KeymapContext::Normal),
        KeyBinding::new(
            "Ctrl+W / Ctrl+⌫",
            "Delete previous word (prompt)",
            KeymapContext::Normal,
        ),
        KeyBinding::new(
            "Ctrl+Del / Alt+D",
            "Delete next word (prompt)",
            KeymapContext::Normal,
        ),
        KeyBinding::new("Ctrl+←/→", "Move by word (prompt)", KeymapContext::Normal),
        KeyBinding::new("y", "Copy selected message", KeymapContext::Normal),
        KeyBinding::new(
            "Ctrl+V",
            "Paste image from clipboard (prompt)",
            KeymapContext::Normal,
        ),
        KeyBinding::new("e / h", "Toggle thinking fold", KeymapContext::Normal),
        KeyBinding::new(
            "t",
            "Toggle todo panel (scrollback / todos)",
            KeymapContext::Normal,
        ),
        KeyBinding::new("l", "Toggle tool results", KeymapContext::Normal),
        KeyBinding::new(
            "Space / Enter",
            "Focus prompt (scrollback)",
            KeymapContext::Normal,
        ),
        KeyBinding::new("Ctrl+P", "Provider setup", KeymapContext::Normal),
        KeyBinding::new("Ctrl+M", "Model selection", KeymapContext::Normal),
        KeyBinding::new("Ctrl+B", "Toggle sidebar", KeymapContext::Normal),
        KeyBinding::new("Ctrl+G", "Toggle tasks panel", KeymapContext::Normal),
        KeyBinding::new(
            "Ctrl+. / Ctrl+,",
            "Sidebar next / prev tab",
            KeymapContext::Normal,
        ),
        KeyBinding::new("1–6", "Sidebar tab (scrollback)", KeymapContext::Normal),
        KeyBinding::new("[ / ]", "Sidebar tabs (scrollback)", KeymapContext::Normal),
        KeyBinding::new("Ctrl+A", "Toggle auto scroll", KeymapContext::Normal),
        KeyBinding::new("Ctrl+L", "Clear session", KeymapContext::Normal),
    ]
}

fn dialog_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("Esc/q", "Close dialog", KeymapContext::Dialog),
        KeyBinding::new("Enter/y", "Confirm / Select", KeymapContext::Dialog),
        KeyBinding::new("Tab/Up/Down", "Navigate", KeymapContext::Dialog),
        KeyBinding::new(
            "/ · j/k · ←/→",
            "Search / nav / fold (model picker)",
            KeymapContext::Dialog,
        ),
        KeyBinding::new("Ctrl+S", "Save (provider form)", KeymapContext::Dialog),
        KeyBinding::new("Ctrl+V", "Paste (API key)", KeymapContext::Dialog),
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
        KeyBinding::new("Esc/q", "Close help", KeymapContext::Help),
        KeyBinding::new("Up/Down", "Scroll help", KeymapContext::Help),
    ]
}

#[cfg(test)]
#[path = "keymap_tests.rs"]
mod tests;
