use super::*;

fn legacy() -> TerminalEnv {
    TerminalEnv::default()
}

fn kitty() -> TerminalEnv {
    TerminalEnv {
        keyboard_enhanced: true,
        ..TerminalEnv::default()
    }
}

fn binds(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn chord(spec: &str) -> Chord {
    Chord::parse(spec).expect(spec)
}

fn has(issues: &[KeyIssue], key: &str, needle: &str) -> bool {
    issues
        .iter()
        .any(|i| i.key == key && i.problem.contains(needle))
}

#[test]
fn chord_label_event_and_action_name_round_trip() {
    for spec in [
        "ctrl+shift+t",
        "alt+enter",
        "pageup",
        "f2",
        "ctrl+space",
        "shift+tab",
    ] {
        assert_eq!(chord(spec).label(), spec);
    }
    assert_eq!(chord("G").label(), "shift+g");
    assert_eq!(
        chord("shift+g").to_event().code,
        KeyCode::Char('G'),
        "Shift+letter arrives as a capital"
    );
    assert_eq!(action_name(Action::ToggleSidebar), "toggle_sidebar");
    assert_eq!(action_name(Action::ScrollDown), "scroll_up");
    assert_eq!(action_name(Action::InputBackspace), "input_backspace");
}

#[test]
fn legacy_terminals_lose_modified_enter_tab_and_punctuation() {
    for spec in [
        "ctrl+enter",
        "shift+enter",
        "ctrl+tab",
        "ctrl+m",
        "ctrl+i",
        "ctrl+shift+k",
        "ctrl+.",
        "ctrl+backspace",
    ] {
        assert!(!legacy_deliverable(chord(spec)), "{spec}");
    }
    for spec in [
        "ctrl+k",
        "alt+enter",
        "shift+tab",
        "ctrl+space",
        "ctrl+up",
        "f5",
        "g",
    ] {
        assert!(legacy_deliverable(chord(spec)), "{spec}");
    }
}

#[test]
fn configured_issues_flag_duplicates_unknowns_and_os_keys() {
    let b = binds(&[
        ("ctrl+k", "toggle_sidebar"),
        ("ctrl-k", "model_dialog"),
        ("alt+j", "scroll_up"),
        ("Alt-J", "scroll_up"),
        ("cmd+k", "toggle_sidebar"),
        ("hyper+x", "toggle_sidebar"),
        ("f2", "fly_away"),
    ]);
    let issues = configured_issues(&b, &kitty());
    assert!(has(&issues, "ctrl+k", "bound twice"), "{issues:?}");
    assert!(
        !has(&issues, "alt+j", "bound twice"),
        "same action twice is fine"
    );
    assert!(has(&issues, "cmd+k", "never reach"), "{issues:?}");
    assert!(has(&issues, "hyper+x", "unknown key"), "{issues:?}");
    assert!(
        has(&issues, "f2", "unknown action `fly_away`"),
        "{issues:?}"
    );
    assert_eq!(issues.len(), 4, "{issues:?}");
}

#[test]
fn configured_issues_depend_on_terminal() {
    let b = binds(&[("ctrl+enter", "newline"), ("shift+enter", "none")]);
    let legacy_issues = configured_issues(&b, &legacy());
    assert!(has(&legacy_issues, "ctrl+enter", "kitty keyboard protocol"));
    assert!(
        !legacy_issues.iter().any(|i| i.key == "shift+enter"),
        "unbinding an undeliverable chord is not a problem"
    );
    assert!(configured_issues(&b, &kitty()).is_empty());
    let windows = TerminalEnv {
        windows: true,
        ..TerminalEnv::default()
    };
    assert!(configured_issues(&b, &windows).is_empty());

    let wt = TerminalEnv {
        keyboard_enhanced: true,
        windows_terminal: true,
        tmux: true,
        ..TerminalEnv::default()
    };
    let b = binds(&[
        ("ctrl+shift+t", "toggle_tasks"),
        ("ctrl+b", "toggle_tasks"),
        ("ctrl+o", "toggle_sidebar"),
    ]);
    let issues = configured_issues(&b, &wt);
    assert!(
        has(&issues, "ctrl+shift+t", "Windows Terminal"),
        "{issues:?}"
    );
    assert!(has(&issues, "ctrl+b", "tmux prefix"), "{issues:?}");
    assert!(has(&issues, "ctrl+o", "session keys"), "{issues:?}");
    let elsewhere = configured_issues(&b, &kitty());
    assert_eq!(
        elsewhere.len(),
        2,
        "only the session keys remain: {elsewhere:?}"
    );
}

#[test]
fn keys_report_lists_defaults_custom_and_conflicts() {
    let b = binds(&[("f2", "model_dialog"), ("ctrl+l", "none")]);
    let km = Keymap::from_bindings(&b);
    let report = keys_report(&km, &b, &legacy());
    assert!(report.contains("toggle_sidebar"), "{report}");
    assert!(report.contains("f2*"), "{report}");
    let row = |key: &str| report.lines().any(|l| l.starts_with(&format!("  {key} ")));
    assert!(!row("ctrl+l"), "unbound chord drops out:\n{report}");
    assert!(!row("alt+1"), "Alt+1 is the same as 1:\n{report}");
    assert!(!row("shift+end"), "Shift+End is the same as End:\n{report}");
    assert!(row("alt+b"), "Alt+B moves by word:\n{report}");
    assert!(row("shift+left"), "Shift+Left jumps turns:\n{report}");
    assert!(report.contains("Session keys"), "{report}");
    assert!(report.contains("ctrl+o"), "{report}");
    assert!(report.contains("Conflicts:"), "{report}");
    assert!(
        report
            .lines()
            .any(|l| l.contains("ctrl+m") && l.contains("kitty")),
        "Ctrl+M arrives as Enter without the protocol:\n{report}"
    );
    assert!(report.contains("/terminal-setup"), "{report}");

    let clean = keys_report(&Keymap::new(), &HashMap::new(), &kitty());
    assert!(clean.ends_with("No conflicts."), "{clean}");
    let tmux = TerminalEnv {
        keyboard_enhanced: true,
        tmux: true,
        ..TerminalEnv::default()
    };
    let in_tmux = keys_report(&Keymap::new(), &HashMap::new(), &tmux);
    assert!(
        in_tmux
            .lines()
            .any(|l| l.contains("ctrl+b") && l.contains("tmux prefix")),
        "{in_tmux}"
    );
    assert!(!in_tmux.contains("/terminal-setup shows"), "{in_tmux}");
}

#[test]
fn startup_notice_fires_once_per_conflict_set() {
    let dir = tempfile::tempdir().unwrap();
    let one = vec![KeyIssue::new("ctrl+enter", NEEDS_PROTOCOL)];
    let two = vec![
        KeyIssue::new("ctrl+enter", NEEDS_PROTOCOL),
        KeyIssue::new("ctrl+b", "tmux prefix; tmux keeps it"),
    ];
    assert_eq!(issues_hash(&one), issues_hash(&one.clone()));
    assert_ne!(issues_hash(&one), issues_hash(&two));

    let first = startup_notice(&one, dir.path()).expect("first sight");
    assert!(first.starts_with("Key binding ctrl+enter:"), "{first}");
    assert!(first.ends_with("/keys for details"), "{first}");
    assert!(
        startup_notice(&one, dir.path()).is_none(),
        "same set: quiet"
    );
    let more = startup_notice(&two, dir.path()).expect("set changed");
    assert!(more.contains("(+1 more)"), "{more}");
    assert!(
        startup_notice(&[], dir.path()).is_none(),
        "cleared: nothing to say"
    );
    assert!(
        startup_notice(&one, dir.path()).is_some(),
        "returning conflict is new again"
    );

    // Unwritable dir: still reports, just cannot remember.
    let missing = dir.path().join("nope");
    assert!(startup_notice(&one, &missing).is_some());
}
