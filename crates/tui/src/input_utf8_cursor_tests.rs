use super::*;

#[test]
fn insert_multibyte_advances_by_utf8_len() {
    let mut buf = String::new();
    let mut cursor = 0usize;
    for c in ['ş', 'a', 'ğ'] {
        let pos = clamp_cursor(&buf, cursor);
        buf.insert(pos, c);
        cursor = pos + c.len_utf8();
    }
    assert_eq!(buf, "şağ");
    assert_eq!(cursor, buf.len());
    assert!(buf.is_char_boundary(cursor));
}

#[test]
fn backspace_deletes_whole_grapheme_bytes() {
    let mut buf = String::from("şa");
    let mut cursor = buf.len();
    let end = clamp_cursor(&buf, cursor);
    let start = prev_boundary(&buf, end);
    buf.replace_range(start..end, "");
    cursor = start;
    assert_eq!(buf, "ş");
    assert_eq!(cursor, "ş".len());

    let end = clamp_cursor(&buf, cursor);
    let start = prev_boundary(&buf, end);
    buf.replace_range(start..end, "");
    cursor = start;
    assert_eq!(buf, "");
    assert_eq!(cursor, 0);
}

#[test]
fn left_right_stay_on_char_boundaries() {
    let s = "şxğ";
    let mut c = s.len();
    c = prev_boundary(s, c);
    assert!(s.is_char_boundary(c));
    assert_eq!(&s[..c], "şx");
    c = prev_boundary(s, c);
    assert_eq!(&s[..c], "ş");
    c = next_boundary(s, c);
    assert_eq!(&s[..c], "şx");
}

#[test]
fn clamp_and_boundaries_cover_edges() {
    let s = "şa";
    assert_eq!(clamp_cursor(s, 0), 0);
    assert_eq!(clamp_cursor(s, s.len()), s.len());
    assert_eq!(clamp_cursor(s, 1), 0, "mid-codepoint snaps back");
    assert_eq!(clamp_cursor(s, 99), s.len());
    assert_eq!(prev_boundary("", 0), 0);
    assert_eq!(prev_boundary(s, 0), 0);
    assert_eq!(next_boundary(s, s.len()), s.len());
    assert_eq!(next_boundary("", 0), 0);
    assert!(is_bare_slash_draft("/"));
    assert!(is_bare_slash_draft("//"));
    assert!(!is_bare_slash_draft(""));
    assert!(!is_bare_slash_draft("/h"));
}

#[test]
fn word_boundaries_skip_whitespace_then_the_token() {
    let s = "hello world";
    assert_eq!(prev_word_boundary(s, s.len()), 6);
    assert_eq!(prev_word_boundary(s, 6), 0);
    assert_eq!(prev_word_boundary(s, 0), 0);
    assert_eq!(next_word_boundary(s, 0), 5);
    assert_eq!(next_word_boundary(s, 5), 11);
    assert_eq!(next_word_boundary(s, s.len()), s.len());

    let trailing = "hello  ";
    assert_eq!(prev_word_boundary(trailing, trailing.len()), 0);
    let leading = "  hello";
    assert_eq!(next_word_boundary(leading, 0), leading.len());
    assert_eq!(prev_word_boundary("", 0), 0);
    assert_eq!(next_word_boundary("", 0), 0);
    assert_eq!(prev_word_boundary("şa ğ", "şa ğ".len()), 4);
}

#[test]
fn sidebar_tab_actions_map_to_tabs() {
    use crate::app::SidebarTab;
    use crate::keymap::Action;
    assert_eq!(
        sidebar_tab_from_action(Action::SidebarTab1),
        Some(SidebarTab::Files)
    );
    assert_eq!(
        sidebar_tab_from_action(Action::SidebarTab2),
        Some(SidebarTab::Diagnostics)
    );
    assert_eq!(
        sidebar_tab_from_action(Action::SidebarTab3),
        Some(SidebarTab::Mcp)
    );
    assert_eq!(
        sidebar_tab_from_action(Action::SidebarTab4),
        Some(SidebarTab::Todos)
    );
    assert_eq!(
        sidebar_tab_from_action(Action::SidebarTab5),
        Some(SidebarTab::Preview)
    );
    assert_eq!(
        sidebar_tab_from_action(Action::SidebarTab6),
        Some(SidebarTab::Agents)
    );
    assert_eq!(sidebar_tab_from_action(Action::Quit), None);
}
