use super::*;

fn env_from(pairs: &[(&str, &str)], enhanced: bool, windows: bool) -> TerminalEnv {
    let owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    TerminalEnv::from_lookup(enhanced, windows, |k| {
        owned.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    })
}

const FULL: &str = "set -s extended-keys on\n\
set -as terminal-features 'xterm*:extkeys'\n\
set -g allow-passthrough on\n\
set -g focus-events on\n";

#[test]
fn env_reads_tmux_wezterm_and_windows_terminal() {
    let e = env_from(
        &[
            ("TMUX", "/tmp/tmux-1/default,1,0"),
            ("TERM_PROGRAM", "WezTerm"),
            ("TERM", "xterm-256color"),
            ("WT_SESSION", "abc"),
        ],
        false,
        false,
    );
    assert!(e.tmux && e.windows_terminal && e.wezterm());
    assert_eq!(e.term, "xterm-256color");
    assert!(!e.modifiers_reported());

    let empty = env_from(&[("TMUX", "")], true, false);
    assert!(!empty.tmux, "an empty $TMUX is not tmux");
    assert!(!empty.wezterm());
    assert!(empty.modifiers_reported());
    assert!(env_from(&[], false, true).modifiers_reported());
    assert!(!TerminalEnv::detect(false).keyboard_enhanced);
}

#[test]
fn tmux_missing_reads_options_not_comments() {
    assert_eq!(tmux_missing("").len(), TMUX_SETTINGS.len());
    assert!(tmux_missing(FULL).is_empty());
    let commented = "# set -g focus-events on\nset -g mouse on\n";
    assert_eq!(tmux_missing(commented).len(), TMUX_SETTINGS.len());
    // A deliberate value is the user's choice, not a missing setting.
    let off = "set-option -g focus-events off # keep quiet\n";
    assert!(
        !tmux_missing(off)
            .iter()
            .any(|s| s.line.contains("focus-events"))
    );
    // terminal-features without extkeys does not enable extended keys.
    let rgb = "set -as terminal-features ',xterm*:RGB'\n";
    assert!(
        tmux_missing(rgb)
            .iter()
            .any(|s| s.line.contains("terminal-features"))
    );
}

#[test]
fn conf_edit_appends_missing_and_is_idempotent() {
    let fresh = tmux_conf_with_settings("").expect("empty file gets every setting");
    assert!(fresh.starts_with(TMUX_HEADER), "{fresh}");
    for s in TMUX_SETTINGS {
        assert!(fresh.contains(s.line), "{fresh}");
    }
    assert!(
        tmux_conf_with_settings(&fresh).is_none(),
        "second run is a no-op"
    );

    let partial = "set -g mouse on\nset -g focus-events on";
    let next = tmux_conf_with_settings(partial).expect("three settings missing");
    assert!(next.starts_with("set -g mouse on\nset -g focus-events on\n\n"));
    assert_eq!(next.matches("focus-events").count(), 1, "{next}");
    assert!(next.ends_with("set -g allow-passthrough on\n"), "{next}");
    assert!(tmux_conf_with_settings(FULL).is_none());
}

#[test]
fn report_covers_protocol_tmux_and_wezterm() {
    let plain = report(&env_from(&[], false, false), None, None);
    assert!(plain.contains("terminal: unknown (TERM=unset)"), "{plain}");
    assert!(plain.contains("kitty keyboard protocol: off"), "{plain}");
    assert!(plain.contains("inside tmux: no"), "{plain}");

    let on = report(&env_from(&[], true, false), None, None);
    assert!(on.contains("protocol: on"), "{on}");
    let win = report(&env_from(&[], false, true), None, None);
    assert!(win.contains("Windows console"), "{win}");

    let tmux = env_from(&[("TMUX", "x"), ("TERM_PROGRAM", "tmux")], false, false);
    let missing = report(&tmux, Some(""), Some(Path::new("/h/.tmux.conf")));
    assert!(missing.contains("4 of 4 settings missing"), "{missing}");
    assert!(missing.contains("set -s extended-keys on"), "{missing}");
    assert!(missing.contains(".tmux.conf"), "{missing}");
    let done = report(&tmux, Some(FULL), None);
    assert!(
        done.contains("(~/.tmux.conf): already configured"),
        "{done}"
    );

    let wez = env_from(&[("TERM_PROGRAM", "WezTerm")], false, false);
    assert!(report(&wez, None, None).contains(WEZTERM_LINE));
    let wez_on = env_from(&[("TERM_PROGRAM", "WezTerm")], true, false);
    assert!(!report(&wez_on, None, None).contains(WEZTERM_LINE));
}

#[test]
fn read_tmux_configs_includes_xdg_file() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(read_tmux_configs(home.path()), "");
    let xdg = home.path().join(".config").join("tmux");
    std::fs::create_dir_all(&xdg).unwrap();
    std::fs::write(xdg.join("tmux.conf"), "set -g focus-events on\n").unwrap();
    std::fs::write(tmux_conf_path(home.path()), "set -g allow-passthrough on\n").unwrap();
    let text = read_tmux_configs(home.path());
    assert_eq!(tmux_missing(&text).len(), 2, "{text}");
}

#[test]
fn apply_writes_backup_then_settings_and_second_run_is_noop() {
    let home = tempfile::tempdir().unwrap();
    let conf = tmux_conf_path(home.path());

    // No file yet: create it, nothing to back up.
    let first = apply_tmux_setup(&conf);
    assert!(matches!(
        first,
        Ok(TmuxApply::Written {
            added: 4,
            backup: None
        })
    ));
    assert!(apply_message(&conf, &first).contains("Added 4 setting(s)"));
    let again = apply_tmux_setup(&conf);
    assert!(matches!(again, Ok(TmuxApply::AlreadyConfigured)));
    assert!(apply_message(&conf, &again).contains("already configured"));

    // Existing file: copy to .bak with the original text first.
    std::fs::write(&conf, "set -g mouse on\n").unwrap();
    let result = apply_tmux_setup(&conf);
    let Ok(TmuxApply::Written {
        added: 4,
        backup: Some(bak),
    }) = &result
    else {
        panic!("expected a backup: {result:?}");
    };
    assert_eq!(std::fs::read_to_string(bak).unwrap(), "set -g mouse on\n");
    assert!(apply_message(&conf, &result).contains("backup:"));
    let written = std::fs::read_to_string(&conf).unwrap();
    assert!(written.starts_with("set -g mouse on\n"), "{written}");
    assert!(tmux_missing(&written).is_empty(), "{written}");
}

#[test]
fn apply_reports_unreadable_config() {
    let home = tempfile::tempdir().unwrap();
    // A directory where the file should be: read fails with something other
    // than NotFound and nothing is written.
    let conf = home.path().join("dir.conf");
    std::fs::create_dir(&conf).unwrap();
    let result = apply_tmux_setup(&conf);
    assert!(result.is_err());
    assert!(apply_message(&conf, &result).starts_with("Could not update"));
}
