use super::*;

fn cfg(desktop: Option<bool>, on: &[&str]) -> NotifyConfig {
    NotifyConfig {
        on: on.iter().map(|s| s.to_string()).collect(),
        desktop,
        ..NotifyConfig::default()
    }
}

fn written(term: &str, tmux: bool) -> String {
    let mut out = Vec::new();
    write_to(&mut out, term, tmux, "all good");
    String::from_utf8(out).unwrap()
}

#[test]
fn picks_the_sequence_for_the_terminal() {
    assert_eq!(
        written("xterm-256color", false),
        "\x1b]9;WhyCodes: all good\x07"
    );
    assert_eq!(
        written("foot", false),
        "\x1b]777;notify;WhyCodes;all good\x07"
    );
    assert_eq!(
        written("screen", true),
        "\x1bPtmux;\x1b\x1b]9;WhyCodes: all good\x07\x1b\\"
    );
    assert_eq!(
        sequence("rxvt-unicode", false, "a;b", "x;y"),
        "\x1b]777;notify;a,b;x;y\x07"
    );
}

#[test]
fn body_is_one_short_line_without_controls() {
    assert_eq!(clean("two\n  lines\x1b[31m\u{9b}"), "two lines[31m");
    let long = clean(&"x".repeat(500));
    assert_eq!(long.chars().count(), MAX_BODY);
    assert!(long.ends_with('…'));
    assert_eq!(turn_done_body("\n  Fixed it.\nMore"), "Done: Fixed it.");
    assert_eq!(turn_done_body(" \n"), "Turn finished");
}

#[test]
fn config_focus_and_events() {
    // Nothing else in the test process turns `turn_done` on.
    configure(&cfg(Some(true), &[]));
    configure(&cfg(None, &["turn_done", "need_input"]));
    assert!(!wanted(NotifyEvent::TurnDone), "desktop not enabled");
    configure(&cfg(Some(true), &["need_input"]));
    configure(&NotifyConfig::default());
    assert!(wanted(NotifyEvent::NeedInput), "never switched back off");
    assert!(!wanted(NotifyEvent::TurnDone));
    assert!(wants_focus_reports());
    // Focused (the default): nothing reaches stdout.
    notify(NotifyEvent::NeedInput, "x");
    notify(NotifyEvent::TurnDone, "x");

    note_focus_events(&[Event::FocusLost, Event::FocusGained]);
    assert!(FOCUSED.load(Ordering::Relaxed));
    note_focus_events(&[Event::Resize(1, 1)]);
    assert!(FOCUSED.load(Ordering::Relaxed));
}

struct Broken;

impl Write for Broken {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("closed"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn write_errors_are_ignored() {
    write_to(&mut Broken, "xterm", false, "x");
    assert!(Broken.flush().is_ok());
}
