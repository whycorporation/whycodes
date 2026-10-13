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
fn quit_works_from_both_focuses_and_question_mark_is_not_help() {
    let k = Keymap::new();
    for focus in [FocusPane::Prompt, FocusPane::Scrollback, FocusPane::Todos] {
        assert_eq!(
            k.resolve(KeymapContext::Normal, focus, &ctrl(KeyCode::Char('c'))),
            Some(Action::Quit)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, focus, &ctrl(KeyCode::Char('q'))),
            Some(Action::ForceQuit)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, focus, &ctrl(KeyCode::Char('v'))),
            Some(Action::PasteClipboard)
        );
        assert_eq!(
            k.resolve(KeymapContext::Normal, focus, &key(KeyCode::Char('?'))),
            None
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
        k.resolve(KeymapContext::Normal, f, &ctrl(KeyCode::Char('w'))),
        Some(Action::InputKillWordBack)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &ctrl(KeyCode::Backspace)),
        Some(Action::InputKillWordBack)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &ctrl(KeyCode::Delete)),
        Some(Action::InputKillWordForward)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &ctrl(KeyCode::Left)),
        Some(Action::InputWordLeft)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &ctrl(KeyCode::Right)),
        Some(Action::InputWordRight)
    );
    let alt_backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT);
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &alt_backspace),
        Some(Action::InputKillWordBack)
    );
    let alt_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT);
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &alt_d),
        Some(Action::InputKillWordForward)
    );
    let alt_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT);
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &alt_b),
        Some(Action::InputWordLeft)
    );
    let alt_f = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT);
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &alt_f),
        Some(Action::InputWordRight)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &ctrl(KeyCode::Char('v'))),
        Some(Action::PasteClipboard)
    );
    // Product chords must not be stolen by word-editing.
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &ctrl(KeyCode::Char('a'))),
        Some(Action::ToggleAutoScroll)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &ctrl(KeyCode::Char('b'))),
        Some(Action::ToggleSidebar)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &shift(KeyCode::Left)),
        Some(Action::JumpPrevTurn)
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
        None,
        "printable i must type, not steal into FocusPrompt"
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
        k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('t'))),
        Some(Action::ToggleTodosPanel)
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
    // Ctrl+W is prompt-only; scrollback must not steal session-picker close.
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &ctrl(KeyCode::Char('w'))),
        None
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &ctrl(KeyCode::Char('j'))),
        Some(Action::ScrollUp)
    );
}

#[test]
fn todos_focus_bindings() {
    let k = Keymap::new();
    let f = FocusPane::Todos;
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &key(KeyCode::Up)),
        Some(Action::ScrollDown)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('k'))),
        Some(Action::ScrollDown)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &key(KeyCode::Down)),
        Some(Action::ScrollUp)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('j'))),
        Some(Action::ScrollUp)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('t'))),
        Some(Action::ToggleTodosPanel)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('g'))),
        Some(Action::ScrollToTop)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('G'))),
        Some(Action::ScrollToBottom)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &key(KeyCode::Home)),
        Some(Action::ScrollToTop)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &key(KeyCode::End)),
        Some(Action::ScrollToBottom)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &key(KeyCode::Esc)),
        Some(Action::EscapeMode)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &key(KeyCode::Enter)),
        Some(Action::FocusPrompt)
    );
    assert_eq!(
        k.resolve(KeymapContext::Normal, f, &key(KeyCode::Char('i'))),
        None,
        "printable i must type, not steal into FocusPrompt"
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
            FocusPane::Prompt,
            &ctrl(KeyCode::Char('g'))
        ),
        Some(Action::ToggleTasksPane)
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
            &ctrl(KeyCode::Char('.'))
        ),
        Some(Action::SidebarNextTab)
    );
    assert_eq!(
        k.resolve(
            KeymapContext::Normal,
            FocusPane::Prompt,
            &ctrl(KeyCode::Char(','))
        ),
        Some(Action::SidebarPrevTab)
    );
    assert_eq!(
        k.resolve(
            KeymapContext::Normal,
            FocusPane::Prompt,
            &key(KeyCode::Char('.'))
        ),
        None,
        "bare `.` still types in the prompt"
    );
    assert_eq!(
        k.resolve(
            KeymapContext::Normal,
            FocusPane::Prompt,
            &key(KeyCode::Char('1'))
        ),
        None,
        "digits still type in the prompt"
    );
    assert_eq!(
        k.resolve(
            KeymapContext::Normal,
            FocusPane::Scrollback,
            &key(KeyCode::Char('1'))
        ),
        Some(Action::SidebarTab1)
    );
    assert_eq!(
        k.resolve(
            KeymapContext::Normal,
            FocusPane::Scrollback,
            &key(KeyCode::Char('6'))
        ),
        Some(Action::SidebarTab6)
    );
    assert_eq!(
        k.resolve(
            KeymapContext::Normal,
            FocusPane::Prompt,
            &ctrl(KeyCode::Tab)
        ),
        None,
        "Ctrl+Tab stays session-MRU (run loop); keymap must not steal it"
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
    // `:` is a printable character in the prompt (URLs, `::`, `std::io`).
    assert_eq!(
        k.resolve(
            KeymapContext::Normal,
            FocusPane::Prompt,
            &key(KeyCode::Char(':'))
        ),
        None
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
            &ctrl(KeyCode::Char('v'))
        ),
        Some(Action::PasteClipboard)
    );
    assert_eq!(
        k.resolve(
            KeymapContext::Dialog,
            FocusPane::Prompt,
            &ctrl(KeyCode::Char('c'))
        ),
        Some(Action::Quit)
    );
    assert_eq!(
        k.resolve(
            KeymapContext::Dialog,
            FocusPane::Prompt,
            &ctrl(KeyCode::Char('q'))
        ),
        Some(Action::ForceQuit)
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
        None
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

fn bindings(pairs: &[(&str, &str)]) -> Keymap {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Keymap::from_bindings(&map)
}

#[test]
fn chords_parse_modifiers_names_and_capitals() {
    let c = |code, mods| Some(Chord::new(code, mods));
    assert_eq!(
        Chord::parse("ctrl+k"),
        c(KeyCode::Char('k'), KeyModifiers::CONTROL)
    );
    assert_eq!(
        Chord::parse("Ctrl-K"),
        c(
            KeyCode::Char('k'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )
    );
    assert_eq!(
        Chord::parse("control+alt+shift+g"),
        c(
            KeyCode::Char('g'),
            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT
        )
    );
    assert_eq!(Chord::parse("G"), Chord::parse("shift+g"));
    assert_eq!(
        Chord::parse("meta+enter"),
        c(KeyCode::Enter, KeyModifiers::ALT)
    );
    assert_eq!(
        Chord::parse("ctrl++"),
        c(KeyCode::Char('+'), KeyModifiers::CONTROL)
    );
    assert_eq!(Chord::parse("-"), c(KeyCode::Char('-'), KeyModifiers::NONE));
    assert_eq!(Chord::parse("shift+?"), Chord::parse("?"));
    assert_eq!(Chord::parse("backtab"), Chord::parse("shift+tab"));
    assert_eq!(Chord::parse(" f12 "), c(KeyCode::F(12), KeyModifiers::NONE));
    for (name, code) in [
        ("return", KeyCode::Enter),
        ("escape", KeyCode::Esc),
        ("space", KeyCode::Char(' ')),
        ("backspace", KeyCode::Backspace),
        ("del", KeyCode::Delete),
        ("ins", KeyCode::Insert),
        ("home", KeyCode::Home),
        ("end", KeyCode::End),
        ("pgup", KeyCode::PageUp),
        ("pgdn", KeyCode::PageDown),
        ("up", KeyCode::Up),
        ("down", KeyCode::Down),
        ("left", KeyCode::Left),
        ("right", KeyCode::Right),
    ] {
        assert_eq!(Chord::parse(name), c(code, KeyModifiers::NONE), "{name}");
    }
    for bad in ["hyper+k", "ctrl+nope", "f0", "f25", "fx", ""] {
        assert_eq!(Chord::parse(bad), None, "{bad}");
    }
}

#[test]
fn user_bindings_override_add_and_unbind_in_normal_context() {
    let k = bindings(&[
        ("ctrl+o", "toggle_sidebar"),
        ("ctrl+l", "none"),
        ("f2", "Model-Dialog"),
        ("alt+k", "scroll_up"),
        ("ctrl+z", "not_an_action"),
        ("hyper+x", "quit"),
    ]);
    let n = KeymapContext::Normal;
    let p = FocusPane::Prompt;
    assert_eq!(
        k.resolve(n, p, &ctrl(KeyCode::Char('o'))),
        Some(Action::ToggleSidebar)
    );
    assert_eq!(k.resolve(n, p, &ctrl(KeyCode::Char('l'))), None, "unbound");
    assert_eq!(
        k.resolve(KeymapContext::Session, p, &key(KeyCode::F(2))),
        Some(Action::OpenModelDialog)
    );
    let alt_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT);
    assert_eq!(k.resolve(n, p, &alt_k), Some(Action::ScrollDown));
    // Bad entries are skipped; defaults still apply.
    assert_eq!(k.resolve(n, p, &ctrl(KeyCode::Char('z'))), None);
    assert_eq!(
        k.resolve(n, p, &ctrl(KeyCode::Char('b'))),
        Some(Action::ToggleSidebar)
    );
    // Dialogs keep their fixed keys.
    assert_eq!(
        k.resolve(KeymapContext::Dialog, p, &ctrl(KeyCode::Char('l'))),
        None
    );
    assert_eq!(
        k.resolve(KeymapContext::Dialog, p, &key(KeyCode::Enter)),
        Some(Action::DialogConfirm)
    );
}

#[test]
fn printable_bindings_never_capture_prompt_typing() {
    let k = bindings(&[("x", "copy_selection"), ("G", "scroll_to_top")]);
    let n = KeymapContext::Normal;
    assert_eq!(
        k.resolve(n, FocusPane::Prompt, &key(KeyCode::Char('x'))),
        None
    );
    assert_eq!(
        k.resolve(n, FocusPane::Scrollback, &key(KeyCode::Char('x'))),
        Some(Action::CopySelection)
    );
    assert_eq!(
        k.resolve(n, FocusPane::Scrollback, &shift(KeyCode::Char('G'))),
        Some(Action::ScrollToTop)
    );
}

#[test]
fn every_bindable_action_name_resolves() {
    for (name, action) in ACTION_NAMES {
        assert_eq!(action_from_name(name), Some(*action), "{name}");
    }
    assert_eq!(action_from_name("dialog_confirm"), None);
}

#[test]
fn app_keymap_comes_from_tui_config() {
    let config = crate::config::TuiAppConfig {
        key_bindings: [("ctrl+o".to_string(), "toggle_sidebar".to_string())].into(),
        ..Default::default()
    };
    let app = crate::app::TuiApp::from_config(config);
    assert_eq!(
        app.keymap.resolve(
            KeymapContext::Normal,
            FocusPane::Prompt,
            &ctrl(KeyCode::Char('o'))
        ),
        Some(Action::ToggleSidebar)
    );
}
