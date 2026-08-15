// ── keymap.rs: Centralized keybinding registry ────────────────────────
// Context-aware bindings (normal / session / dialog modes).
// Focus-aware: Prompt vs Scrollback (Grok Build model).

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
    SidebarNextTab,
    SidebarPrevTab,
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
    /// Cycle primary agents (Ctrl+T; Tab is focus toggle — Grok)
    SwitchAgent,
    /// Tab: Prompt ↔ Scrollback
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

    pub fn resolve(&self, ctx: KeymapContext, focus: FocusPane, key: &KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

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
                    (true, KeyCode::Char('q')) => return Some(Action::Quit),
                    (false, KeyCode::Char('?')) => return Some(Action::ToggleHelp),
                    (false, KeyCode::Char(':')) if focus == FocusPane::Prompt => {
                        return Some(Action::EnterCommand);
                    }
                    (false, KeyCode::Esc) => return Some(Action::EscapeMode),
                    (true, KeyCode::Char('b')) => return Some(Action::ToggleSidebar),
                    // Sidebar tabs: [ / ] in scrollback (prompt still types them).
                    (false, KeyCode::Char(']')) if focus == FocusPane::Scrollback => {
                        return Some(Action::SidebarNextTab);
                    }
                    (false, KeyCode::Char('[')) if focus == FocusPane::Scrollback => {
                        return Some(Action::SidebarPrevTab);
                    }
                    (true, KeyCode::Char('p')) => return Some(Action::OpenProviderDialog),
                    (true, KeyCode::Char('m')) => return Some(Action::OpenModelDialog),
                    (true, KeyCode::Char('a')) => return Some(Action::ToggleAutoScroll),
                    (true, KeyCode::Char('l')) => return Some(Action::ClearSession),
                    // Grok: Tab toggles focus; Ctrl+T cycles agent (was bare Tab)
                    (false, KeyCode::Tab) => return Some(Action::ToggleFocus),
                    (true, KeyCode::Char('t')) => return Some(Action::SwitchAgent),
                    // Ctrl+Space arrives as Char(' ') on most terminals, Null on a few.
                    (true, KeyCode::Char(' ')) | (true, KeyCode::Null) => {
                        return Some(Action::FileComplete);
                    }
                    // Page scroll works from either focus (Grok: PgUp/Dn from prompt)
                    (false, KeyCode::PageUp) => return Some(Action::ScrollPageUp),
                    (false, KeyCode::PageDown) => return Some(Action::ScrollPageDown),
                    (false, KeyCode::Home) if focus == FocusPane::Scrollback => {
                        return Some(Action::ScrollToTop);
                    }
                    (false, KeyCode::End) if focus == FocusPane::Scrollback => {
                        return Some(Action::ScrollToBottom);
                    }
                    _ => {}
                }

                match focus {
                    FocusPane::Prompt => match (ctrl, shift, key.code) {
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
                        // Ctrl+Up/Down: pure scroll while typing
                        (true, _, KeyCode::Up) => Some(Action::ScrollDown),
                        (true, _, KeyCode::Down) => Some(Action::ScrollUp),
                        _ => None,
                    },
                    FocusPane::Scrollback => match (ctrl, shift, key.code) {
                        (false, false, KeyCode::Enter) => Some(Action::FocusPrompt),
                        (false, false, KeyCode::Char(' ')) => Some(Action::FocusPrompt),
                        (false, false, KeyCode::Char('i')) => Some(Action::FocusPrompt),
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
        KeyBinding::new("Tab", "Focus prompt ↔ scrollback", KeymapContext::Normal),
        KeyBinding::new("Ctrl+T", "Cycle primary agent", KeymapContext::Normal),
        KeyBinding::new(":", "Enter command mode (prompt)", KeymapContext::Normal),
        KeyBinding::new(
            "Esc",
            "Cancel turn / double-Esc clear draft",
            KeymapContext::Normal,
        ),
        KeyBinding::new("Enter", "Send message (prompt)", KeymapContext::Normal),
        KeyBinding::new(
            "j/k · ↑/↓",
            "Select message (scrollback)",
            KeymapContext::Normal,
        ),
        KeyBinding::new("Ctrl+↑/↓", "Scroll transcript", KeymapContext::Normal),
        KeyBinding::new("PgUp/PgDn", "Page scroll", KeymapContext::Normal),
        KeyBinding::new("g / G", "Top / bottom (scrollback)", KeymapContext::Normal),
        KeyBinding::new("Shift+←/→", "Prev / next user turn", KeymapContext::Normal),
        KeyBinding::new("y", "Copy selected message", KeymapContext::Normal),
        KeyBinding::new("e / h", "Toggle thinking fold", KeymapContext::Normal),
        KeyBinding::new("l", "Toggle tool results", KeymapContext::Normal),
        KeyBinding::new(
            "Space / i",
            "Focus prompt (scrollback)",
            KeymapContext::Normal,
        ),
        KeyBinding::new("Ctrl+P", "Provider setup", KeymapContext::Normal),
        KeyBinding::new("Ctrl+M", "Model selection", KeymapContext::Normal),
        KeyBinding::new("Ctrl+B", "Toggle sidebar", KeymapContext::Normal),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    #[test]
    fn quit_and_help_work_from_both_focuses() {
        let k = Keymap::new();
        for focus in [FocusPane::Prompt, FocusPane::Scrollback] {
            assert_eq!(
                k.resolve(KeymapContext::Normal, focus, &ctrl(KeyCode::Char('c'))),
                Some(Action::Quit)
            );
            assert_eq!(
                k.resolve(KeymapContext::Normal, focus, &ctrl(KeyCode::Char('q'))),
                Some(Action::Quit)
            );
            assert_eq!(
                k.resolve(KeymapContext::Normal, focus, &key(KeyCode::Char('?'))),
                Some(Action::ToggleHelp)
            );
        }
    }

    #[test]
    fn prompt_focus_bindings() {
        let k = Keymap::new();
        let f = FocusPane::Prompt;
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Enter)),
            Some(Action::SubmitInput)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Up)),
            Some(Action::InputHistoryPrev)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Down)),
            Some(Action::InputHistoryNext)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Left)),
            Some(Action::InputLeft)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Right)),
            Some(Action::InputRight)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &shift(KeyCode::Left)),
            Some(Action::JumpPrevTurn)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &shift(KeyCode::Right)),
            Some(Action::JumpNextTurn)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Home)),
            Some(Action::InputHome)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::End)),
            Some(Action::InputEnd)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Backspace)),
            Some(Action::InputBackspace)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Delete)),
            Some(Action::InputDelete)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &ctrl(KeyCode::Char('u'))),
            Some(Action::InputClear)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &ctrl(KeyCode::Up)),
            Some(Action::ScrollDown)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &ctrl(KeyCode::Down)),
            Some(Action::ScrollUp)
        );
        // plain char in prompt is a draft edit → no action
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('x'))),
            None
        );
    }

    #[test]
    fn scrollback_focus_bindings() {
        let k = Keymap::new();
        let f = FocusPane::Scrollback;
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Enter)),
            Some(Action::FocusPrompt)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char(' '))),
            Some(Action::FocusPrompt)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('i'))),
            Some(Action::FocusPrompt)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Up)),
            Some(Action::SelectPrev)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('k'))),
            Some(Action::SelectPrev)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Down)),
            Some(Action::SelectNext)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('j'))),
            Some(Action::SelectNext)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &shift(KeyCode::Left)),
            Some(Action::JumpPrevTurn)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &shift(KeyCode::Char('h'))),
            Some(Action::JumpPrevTurn)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &shift(KeyCode::Right)),
            Some(Action::JumpNextTurn)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('y'))),
            Some(Action::CopySelection)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('e'))),
            Some(Action::ToggleThinking)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('l'))),
            Some(Action::ToggleToolResult)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('g'))),
            Some(Action::ScrollToTop)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &shift(KeyCode::Char('g'))),
            Some(Action::ScrollToBottom)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('G'))),
            Some(Action::ScrollToBottom)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &key(KeyCode::Backspace)),
            Some(Action::FocusPrompt)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &ctrl(KeyCode::Char('k'))),
            Some(Action::ScrollDown)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, f, &ctrl(KeyCode::Char('j'))),
            Some(Action::ScrollUp)
        );
    }

    #[test]
    fn global_chords() {
        let k = Keymap::new();
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Prompt,
                &ctrl(KeyCode::Char('b'))
            ),
            Some(Action::ToggleSidebar)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Scrollback,
                &key(KeyCode::Char(']'))
            ),
            Some(Action::SidebarNextTab)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Scrollback,
                &key(KeyCode::Char('['))
            ),
            Some(Action::SidebarPrevTab)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Prompt,
                &ctrl(KeyCode::Char('p'))
            ),
            Some(Action::OpenProviderDialog)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Prompt,
                &ctrl(KeyCode::Char('m'))
            ),
            Some(Action::OpenModelDialog)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Prompt,
                &ctrl(KeyCode::Char('a'))
            ),
            Some(Action::ToggleAutoScroll)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Prompt,
                &ctrl(KeyCode::Char('l'))
            ),
            Some(Action::ClearSession)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, FocusPane::Prompt, &key(KeyCode::Tab)),
            Some(Action::ToggleFocus)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Prompt,
                &ctrl(KeyCode::Char('t'))
            ),
            Some(Action::SwitchAgent)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Prompt,
                &ctrl(KeyCode::Char(' '))
            ),
            Some(Action::FileComplete)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Prompt,
                &ctrl(KeyCode::Null)
            ),
            Some(Action::FileComplete)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Prompt,
                &key(KeyCode::PageUp)
            ),
            Some(Action::ScrollPageUp)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Prompt,
                &key(KeyCode::PageDown)
            ),
            Some(Action::ScrollPageDown)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Scrollback,
                &key(KeyCode::Home)
            ),
            Some(Action::ScrollToTop)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Scrollback,
                &key(KeyCode::End)
            ),
            Some(Action::ScrollToBottom)
        );
        // `:` only enters command mode from prompt
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Prompt,
                &key(KeyCode::Char(':'))
            ),
            Some(Action::EnterCommand)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Scrollback,
                &key(KeyCode::Char(':'))
            ),
            None
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, FocusPane::Prompt, &key(KeyCode::Esc)),
            Some(Action::EscapeMode)
        );
        // plain chars in scrollback do nothing
        assert_eq!(
            k.resolve(
                KeymapContext::Normal,
                FocusPane::Scrollback,
                &key(KeyCode::Char('z'))
            ),
            None
        );
    }

    #[test]
    fn shift_alt_enter_inserts_newline() {
        let k = Keymap::new();
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(
            k.resolve(KeymapContext::Normal, FocusPane::Prompt, &enter),
            Some(Action::InputNewline)
        );
        let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
        assert_eq!(
            k.resolve(KeymapContext::Normal, FocusPane::Scrollback, &alt_enter),
            Some(Action::InputNewline)
        );
    }

    #[test]
    fn dialog_context() {
        let k = Keymap::new();
        assert_eq!(
            k.resolve(KeymapContext::Dialog, FocusPane::Prompt, &key(KeyCode::Esc)),
            Some(Action::DialogCancel)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Dialog,
                FocusPane::Prompt,
                &key(KeyCode::Char('q'))
            ),
            Some(Action::DialogCancel)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Dialog,
                FocusPane::Prompt,
                &key(KeyCode::Enter)
            ),
            Some(Action::DialogConfirm)
        );
        assert_eq!(
            k.resolve(KeymapContext::Dialog, FocusPane::Prompt, &key(KeyCode::Tab)),
            Some(Action::DialogNextField)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Dialog,
                FocusPane::Prompt,
                &key(KeyCode::BackTab)
            ),
            Some(Action::DialogPrevField)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Dialog,
                FocusPane::Prompt,
                &key(KeyCode::Char('y'))
            ),
            Some(Action::DialogConfirm)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Dialog,
                FocusPane::Prompt,
                &key(KeyCode::Char('n'))
            ),
            Some(Action::DialogCancel)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Dialog,
                FocusPane::Prompt,
                &key(KeyCode::Char('a'))
            ),
            Some(Action::DialogConfirm)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Dialog,
                FocusPane::Prompt,
                &key(KeyCode::Char('d'))
            ),
            Some(Action::DialogCancel)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Dialog,
                FocusPane::Prompt,
                &ctrl(KeyCode::Char('s'))
            ),
            Some(Action::DialogConfirm)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Dialog,
                FocusPane::Prompt,
                &ctrl(KeyCode::Char('c'))
            ),
            Some(Action::DialogCancel)
        );
        // typing characters in a dialog field → no action
        assert_eq!(
            k.resolve(
                KeymapContext::Dialog,
                FocusPane::Prompt,
                &key(KeyCode::Char('x'))
            ),
            None
        );
    }

    #[test]
    fn command_and_help_contexts() {
        let k = Keymap::new();
        assert_eq!(
            k.resolve(
                KeymapContext::Command,
                FocusPane::Prompt,
                &key(KeyCode::Enter)
            ),
            Some(Action::SubmitInput)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Command,
                FocusPane::Prompt,
                &key(KeyCode::Esc)
            ),
            Some(Action::EscapeMode)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Command,
                FocusPane::Prompt,
                &key(KeyCode::Backspace)
            ),
            Some(Action::InputBackspace)
        );
        assert_eq!(
            k.resolve(KeymapContext::Help, FocusPane::Prompt, &key(KeyCode::Esc)),
            Some(Action::EscapeMode)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Help,
                FocusPane::Prompt,
                &key(KeyCode::Char('q'))
            ),
            Some(Action::EscapeMode)
        );
        assert_eq!(
            k.resolve(
                KeymapContext::Help,
                FocusPane::Prompt,
                &key(KeyCode::Char('?'))
            ),
            Some(Action::ToggleHelp)
        );
    }

    #[test]
    fn bindings_for_context_are_nonempty_and_attributed() {
        for ctx in [
            KeymapContext::Normal,
            KeymapContext::Dialog,
            KeymapContext::Command,
            KeymapContext::Session,
            KeymapContext::Help,
        ] {
            let bindings = bindings_for_context(ctx);
            assert!(!bindings.is_empty(), "{ctx:?}");
            for b in &bindings {
                assert!(!b.key.is_empty());
                assert!(!b.description.is_empty());
            }
        }
        // Session reuses normal bindings; Help has its own set
        assert_eq!(
            bindings_for_context(KeymapContext::Normal).len(),
            bindings_for_context(KeymapContext::Session).len()
        );
        assert!(
            bindings_for_context(KeymapContext::Help).len()
                < bindings_for_context(KeymapContext::Normal).len()
        );
    }
}
