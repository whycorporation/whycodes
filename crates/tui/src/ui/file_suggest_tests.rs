use super::*;
use ratatui::buffer::Buffer;
use ratatui::style::Color;

#[test]
fn at_token_finds_mentions() {
    assert_eq!(at_token("@main", 5), Some((0, 5)));
    assert_eq!(at_token("fix @src/ now", 9), Some((4, 9)));
    assert_eq!(at_token("fix @src/ now", 11), None); // cursor past token
    assert_eq!(at_token("mail bob@corp.io", 12), None); // email, not a mention
    assert_eq!(at_token("no mention", 5), None);
    assert_eq!(at_token("@", 1), Some((0, 1)));
    assert_eq!(at_token("a@", 2), None); // glued to a word → not a mention
    assert_eq!(at_token(" @", 2), Some((1, 2))); // space-separated → mention
    assert_eq!(at_token("user@host path", 9), None);
    assert_eq!(at_token("see @file.rs extra", 16), None);
}

#[test]
fn frecency_lifts_picked_files() {
    let mut st = FileSuggestState {
        frecency: Some(Frecency::ephemeral()),
        ..Default::default()
    };
    let mut matches = vec![
        FileMatch {
            rel: "src/main.rs".into(),
            score: 100,
            ..Default::default()
        },
        FileMatch {
            rel: "src/mail.rs".into(),
            score: 110,
            ..Default::default()
        },
    ];
    st.frecency.as_mut().unwrap().record("src/main.rs");
    FileSuggestState::apply_boosts(st.frecency.as_ref(), &mut matches);
    assert_eq!(matches[0].rel, "src/main.rs"); // boosted past the raw winner
    assert!(matches[0].score > 110);
}

#[test]
fn at_token_stops_at_comma_and_semicolon() {
    assert_eq!(at_token("see @src/lib.rs, please", 6), Some((4, 15)));
    assert_eq!(at_token("see @src/lib.rs; please", 6), Some((4, 15)));
    assert_eq!(at_token("@foo", 0), Some((0, 4)));
    assert_eq!(at_token("", 0), None);
    assert_eq!(at_token("just text", 0), None);
}

#[test]
fn activate_inserts_at_and_spaces_after_a_word() {
    let mut st = FileSuggestState::default();
    let mut buf = String::new();
    let mut cur = 0usize;
    st.activate(&mut buf, &mut cur);
    assert_eq!(buf, "@");
    assert_eq!(cur, 1);
    assert!(st.active);

    let mut buf = String::from("fix");
    let mut cur = 3;
    st.activate(&mut buf, &mut cur);
    assert_eq!(buf, "fix @");
    assert_eq!(cur, 5);
    assert!(st.active);

    // Already inside a token: no second '@'.
    let mut buf = String::from("see @src");
    let mut cur = 8;
    st.activate(&mut buf, &mut cur);
    assert_eq!(buf, "see @src");
    assert!(st.active);
    assert_eq!(st.query, "src");
}

#[test]
fn refresh_dismisses_outside_a_token_and_stays_open_without_index() {
    let mut st = FileSuggestState {
        active: true,
        query: "old".into(),
        matches: vec![FileMatch {
            rel: "x.rs".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    st.refresh("plain text", 4);
    assert!(!st.active);
    assert!(st.matches.is_empty());
    assert!(st.query.is_empty());

    st.refresh("@lib", 4);
    assert!(st.active, "no index still opens the popup");
    assert_eq!(st.query, "lib");
    assert!(st.matches.is_empty());

    // Cursor moved inside the same token: nothing to do.
    st.selected = 3;
    st.refresh("@lib", 2);
    assert_eq!(st.selected, 3);
    assert_eq!(st.query, "lib");
}

#[test]
fn step_wraps_and_accept_replaces_the_token() {
    let mut st = FileSuggestState {
        frecency: Some(Frecency::ephemeral()),
        ..Default::default()
    };
    st.token_start = 0;
    st.matches = vec![
        FileMatch {
            rel: "src".into(),
            is_dir: true,
            ..Default::default()
        },
        FileMatch {
            rel: "src/main.rs".into(),
            ..Default::default()
        },
    ];
    st.selected = 0;
    st.step(0); // stay
    assert_eq!(st.selected, 0);
    st.step(-1);
    assert_eq!(st.selected, 1, "wrap backward");
    st.step(1);
    assert_eq!(st.selected, 0, "wrap forward");
    st.step(1);
    assert_eq!(st.current().map(|m| m.rel.as_str()), Some("src/main.rs"));

    let mut empty = FileSuggestState::default();
    empty.step(1);
    assert!(empty.current().is_none());
    let mut buf = String::from("@x");
    let mut cur = 2;
    assert!(!empty.accept(&mut buf, &mut cur));
    assert_eq!(buf, "@x");

    // Dir accept keeps the picker open and drills down (no index → empty).
    st.selected = 0;
    let mut buf = String::from("@s");
    let mut cur = 2;
    assert!(st.accept(&mut buf, &mut cur));
    assert_eq!(buf, "@src/");

    // File accept records frecency and closes.
    st.matches = vec![FileMatch {
        rel: "README.md".into(),
        ..Default::default()
    }];
    st.selected = 0;
    st.active = true;
    st.token_start = 0;
    let mut buf = String::from("@R");
    let mut cur = 2;
    assert!(!st.accept(&mut buf, &mut cur));
    assert_eq!(buf, "@README.md ");
    assert!(!st.active);
    assert!(st.frecency.as_ref().unwrap().boost("README.md") > 0);
}

#[test]
fn poll_and_await_are_quiet_without_an_index() {
    let mut st = FileSuggestState::default();
    assert!(!st.poll_matches());
    assert!(!st.awaiting_matches());
    st.active = true;
    assert!(!st.poll_matches());
    assert!(!st.awaiting_matches());
    st.results_pending = true;
    assert!(!st.awaiting_matches(), "no index → not awaiting");
    st.dismiss();
    assert!(!st.active);
    assert!(!st.results_pending);
    assert!(st.list_hit.is_none());
}

#[test]
fn row_index_at_uses_last_paint_hit_rect() {
    let mut st = FileSuggestState {
        matches: vec![
            FileMatch {
                rel: "a.rs".into(),
                ..Default::default()
            },
            FileMatch {
                rel: "b.rs".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert!(st.row_index_at(0, 0).is_none());
    st.list_hit = Some(Rect {
        x: 2,
        y: 10,
        width: 20,
        height: 2,
    });
    st.list_scroll_start = 0;
    assert_eq!(st.row_index_at(5, 10), Some(0));
    assert_eq!(st.row_index_at(5, 11), Some(1));
    assert!(st.row_index_at(5, 12).is_none());
    assert!(st.row_index_at(1, 10).is_none());
    assert!(st.row_index_at(22, 10).is_none());
    st.list_scroll_start = 1;
    assert_eq!(st.row_index_at(5, 10), Some(1));
    assert!(st.row_index_at(5, 11).is_none(), "scrolled past last row");
}

#[test]
fn apply_boosts_is_a_no_op_without_frecency() {
    let mut matches = vec![FileMatch {
        rel: "a.rs".into(),
        score: 10,
        ..Default::default()
    }];
    FileSuggestState::apply_boosts(None, &mut matches);
    assert_eq!(matches[0].score, 10);
}

fn paint(width: u16, height: u16, app: &mut TuiApp) -> String {
    use crate::theme::ThemeName;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let palette = ThemeName::DefaultDark.palette();
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| {
            let prompt = Rect {
                x: 0,
                y: height.saturating_sub(2),
                width,
                height: 2,
            };
            render(frame, prompt, app, &palette);
        })
        .expect("draw");
    let buf = terminal.backend().buffer().clone();
    let area = buf.area();
    let mut out = String::new();
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

#[test]
fn render_inactive_clears_hit_and_paints_nothing() {
    let mut app = TuiApp::new(crate::config::TuiAppConfig::default());
    app.file_suggest.list_hit = Some(Rect {
        x: 0,
        y: 0,
        width: 4,
        height: 1,
    });
    app.file_suggest.hovered = Some(0);
    let text = paint(40, 12, &mut app);
    assert!(app.file_suggest.list_hit.is_none());
    assert!(app.file_suggest.hovered.is_none());
    assert!(!text.contains("files"), "{text}");
}

#[test]
fn render_too_short_prompt_skips_the_popup() {
    let mut app = TuiApp::new(crate::config::TuiAppConfig::default());
    app.file_suggest.active = true;
    // prompt_area.y == 1 cannot fit height 3.
    use crate::theme::ThemeName;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let palette = ThemeName::DefaultDark.palette();
    let backend = TestBackend::new(40, 4);
    let mut terminal = Terminal::new(backend).expect("term");
    terminal
        .draw(|frame| {
            render(
                frame,
                Rect {
                    x: 0,
                    y: 1,
                    width: 40,
                    height: 2,
                },
                &mut app,
                &palette,
            );
        })
        .expect("draw");
    assert!(app.file_suggest.list_hit.is_none());
}

#[test]
fn render_empty_shows_unavailable_and_matches_paint_rows() {
    let mut app = TuiApp::new(crate::config::TuiAppConfig::default());
    app.file_suggest.active = true;
    let empty = paint(50, 16, &mut app);
    assert!(empty.contains("files"), "{empty}");
    assert!(
        empty.contains("unavailable") || empty.contains("index"),
        "empty + no index: {empty}"
    );

    app.file_suggest.matches = vec![
        FileMatch {
            rel: "src/main.rs".into(),
            indices: vec![0, 4],
            ..Default::default()
        },
        FileMatch {
            rel: "src".into(),
            is_dir: true,
            ..Default::default()
        },
    ];
    app.file_suggest.selected = 0;
    app.file_suggest.hovered = Some(1);
    let text = paint(50, 16, &mut app);
    assert!(text.contains("files"), "{text}");
    assert!(text.contains("src/main.rs"), "{text}");
    assert!(text.contains("src/"), "dir gets a trailing slash: {text}");
    assert!(text.contains('❯'), "selected row caret: {text}");
    assert!(app.file_suggest.list_hit.is_some());
}

#[test]
fn render_scrollbar_when_matches_overflow() {
    let mut app = TuiApp::new(crate::config::TuiAppConfig::default());
    app.file_suggest.active = true;
    app.file_suggest.matches = (0..20)
        .map(|i| FileMatch {
            rel: format!("file{i:02}.rs"),
            ..Default::default()
        })
        .collect();
    app.file_suggest.selected = 12;
    let text = paint(40, 20, &mut app);
    assert!(text.contains("files"), "{text}");
    assert!(text.contains("20"), "count in the hairline: {text}");
    assert!(
        text.contains("file12.rs") || text.contains("file11.rs"),
        "scrolled window should include the selection: {text}"
    );
    assert!(app.file_suggest.list_scroll_start > 0);
}

#[test]
fn scan_status_root_label_and_scanning_empty_paint() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn a() {}").unwrap();
    let extra_dir = tempfile::tempdir().unwrap();
    std::fs::write(extra_dir.path().join("b.rs"), "fn b() {}").unwrap();
    let extra_name = extra_dir
        .path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "other".into());
    let index = WorkspaceIndex::start(vec![
        dir.path().to_path_buf(),
        extra_dir.path().to_path_buf(),
    ]);
    let mut st = FileSuggestState::default();
    st.set_index(index.clone());
    assert!(st.scan_status().is_some());
    assert!(st.root_label(0).is_none());
    let tag = st.root_label(1);
    assert!(
        tag.as_deref() == Some(extra_name.as_str()) || tag.is_some(),
        "{tag:?} extra={extra_name}"
    );
    assert!(st.root_label(99).is_none());

    let mut app = TuiApp::new(crate::config::TuiAppConfig::default());
    app.file_suggest.set_index(index);
    app.file_suggest.active = true;
    app.file_suggest.matches.clear();
    let text = paint(50, 16, &mut app);
    assert!(
        text.contains("scanning") || text.contains("files") || text.contains("no matches"),
        "{text}"
    );

    app.file_suggest.matches = vec![FileMatch {
        rel: "b.rs".into(),
        root: 1,
        ..Default::default()
    }];
    let tagged = paint(50, 16, &mut app);
    assert!(
        tagged.contains(&extra_name) || tagged.contains("b.rs"),
        "{tagged}"
    );

    let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
    paint_row(
        &mut buf,
        0,
        0,
        0,
        &FileMatch {
            rel: "x.rs".into(),
            ..Default::default()
        },
        None,
        false,
        Color::Black,
        &DropdownColors::from_palette(&crate::theme::ThemeName::DefaultDark.palette()),
        &crate::theme::ThemeName::DefaultDark.palette(),
    );
}
