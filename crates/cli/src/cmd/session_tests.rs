use super::*;

#[test]
fn missing_database_is_not_a_generic_error() {
    let err = anyhow::Error::from(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"));
    assert!(crate::cmd::helpers::is_missing_database(&err));
}

#[test]
fn session_and_stats_printer_helpers() {
    let listed = session_list_line("abc", "Title", 4);
    assert!(listed.contains("Title (4 messages)"), "{listed}");
    assert!(listed.contains("abc"), "{listed}");
    assert_eq!(session_list_dates("c", "u"), "    Created: c  Updated: u");
    assert_eq!(
        session_list_project("/tmp/p").as_deref(),
        Some("    Project: /tmp/p")
    );
    assert!(session_list_project("").is_none());
    assert!(session_list_project("/").is_none());
    assert_eq!(
        stats_token_line(10, 7, 3),
        "  Tokens:    10 total (7 in + 3 out)"
    );
    assert_eq!(
        stats_cache_line(Some(2), Some(1)).as_deref(),
        Some("  Cache:     2 read, 1 write")
    );
    assert_eq!(
        stats_cache_line(Some(2), None).as_deref(),
        Some("  Cache:     2 read, 0 write")
    );
    assert_eq!(
        stats_cache_line(None, Some(4)).as_deref(),
        Some("  Cache:     4 write")
    );
    assert!(stats_cache_line(None, None).is_none());
    assert_eq!(stats_top_session_title("hello", "abcdefghij"), "hello");
    assert_eq!(stats_top_session_title("", "abcdefghij"), "abcdefgh");
}
