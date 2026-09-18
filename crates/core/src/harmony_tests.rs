use super::*;

#[test]
fn fenced_example_is_not_a_hit() {
    let text = "See:\n```\nanalysis to=functions.foo code junk\n```\n";
    assert!(scan_text(text).is_none());
}

#[test]
fn quoted_example_is_not_a_hit() {
    let text = r#"docs say "analysis to=functions.read code""#;
    assert!(scan_text(text).is_none());
}

#[test]
fn bare_marker_without_cosignal_is_not_a_hit() {
    assert!(scan_text("please call to=functions.read next").is_none());
}

#[test]
fn channel_adjacent_is_a_hit() {
    let hit = scan_text("ok\nanalysis to=functions.edit code leftover").unwrap();
    assert_eq!(hit.co_signal, CoSignal::ChannelAdjacency);
}

#[test]
fn cascade_two_markers_is_a_hit() {
    let text = "to=functions.edit x to=functions.apply_patch y";
    let hit = scan_text(text).unwrap();
    assert_eq!(hit.co_signal, CoSignal::Cascade);
}

#[test]
fn cjk_junk_after_marker_is_a_hit() {
    let text =
        "to=functions.edit code \u{8d4c}\u{535a}\u{7f51}\u{7ad9}\u{63a8}\u{8350}\u{4ee3}\u{7406}";
    let hit = scan_text(text).unwrap();
    assert_eq!(hit.co_signal, CoSignal::NonLatinJunk);
}

#[test]
fn fake_code_output_framing_is_a_hit() {
    let hit = scan_text("to=functions.edit code_output Cell 0: spam").unwrap();
    assert_eq!(hit.co_signal, CoSignal::FakeResultFraming);
}

#[test]
fn glitch_surface_before_marker_is_a_hit() {
    let hit = scan_text("SolidGoldMagikarp to=functions.edit code").unwrap();
    assert_eq!(hit.co_signal, CoSignal::GlitchToken);
}

#[test]
fn trusted_boundary_is_inert_when_omitted() {
    assert!(scan_text("diff talks about to=functions.apply_patch").is_none());
}

#[test]
fn trusted_boundary_fires_when_supplied() {
    let text = "clean prefix to=functions.edit leftover";
    let end = text.find(MARKER).unwrap();
    let hit = scan_text_with_boundary(text, Some(end)).unwrap();
    assert_eq!(hit.co_signal, CoSignal::TrustedBoundary);
}

#[test]
fn escape_replaces_special_tokens() {
    let raw = "see <|channel|> then <|call|>";
    let out = escape_replay(raw);
    assert!(!out.contains("<|channel|>"));
    assert!(!out.contains("<|call|>"));
    assert!(out.contains("< channel >"));
}

#[test]
fn scan_json_walks_string_leaves() {
    let v = serde_json::json!({
        "path": "a.rs",
        "new_string": "fn x() {}\nanalysis to=functions.edit code 赌博网站推荐代理"
    });
    assert!(scan_json(&v).is_some());
}

#[test]
fn leak_after_last_hunk_vs_mid_hunk() {
    let clean = "@@ -1 +1 @@\n-a\n+b\n";
    let after = format!("{clean}analysis to=functions.edit code junk\n");
    let at = after.find(MARKER).unwrap();
    assert!(leak_is_after_last_hunk(&after, at));

    let mid = "@@ -1 +1 @@\n-a\n+b analysis to=functions.edit code\n";
    let at = mid.find(MARKER).unwrap();
    assert!(!leak_is_after_last_hunk(mid, at));
}

#[test]
fn last_hunk_start_finds_header() {
    let patch = "--- a\n+++ b\n@@ -1 +1 @@\n a\n";
    assert_eq!(last_hunk_start(patch), Some(patch.find("@@").unwrap()));
}

#[test]
fn truncate_at_line_drops_contaminated_suffix() {
    let text = "keep\nanalysis to=functions.edit code\n";
    let at = text.find(MARKER).unwrap();
    assert_eq!(truncate_at_line(text, at), "keep\n");
}

#[test]
fn hit_summary_names_cosignal() {
    let hit = Hit {
        at: 4,
        co_signal: CoSignal::Cascade,
    };
    assert!(hit.summary().contains("cascade"));
}

#[test]
fn scan_json_skips_non_strings_and_walks_arrays() {
    assert!(scan_json(&serde_json::json!(1)).is_none());
    assert!(scan_json(&serde_json::json!([null, {"x": "to=functions.read"}])).is_none());
    let hit = scan_json(&serde_json::json!([
        "ok",
        "analysis to=functions.edit code"
    ]));
    assert!(hit.is_some());
}

#[test]
fn last_hunk_start_on_leading_header_or_none() {
    assert_eq!(last_hunk_start("@@ -1 +1 @@\n a\n"), Some(0));
    assert_eq!(last_hunk_start("no hunks"), None);
}

#[test]
fn leak_is_after_last_hunk_edge_cases() {
    assert!(!leak_is_after_last_hunk("no hunks", 0));
    let patch = "@@ -1 +1 @@\n a\n@@ -2 +2 @@\n b\nanalysis to=functions.edit\n";
    let at = patch.find(MARKER).unwrap();
    assert!(leak_is_after_last_hunk(patch, at));
    assert!(!leak_is_after_last_hunk(patch, 0));
}

#[test]
fn backtick_span_is_ignored() {
    assert!(scan_text("`analysis to=functions.edit code`").is_none());
}

#[test]
fn escape_is_noop_without_tokens() {
    assert_eq!(escape_replay("plain"), "plain");
}

#[test]
fn cyrillic_run_is_non_latin() {
    let text = "to=functions.edit \u{0410}\u{0411}\u{0412}\u{0413}\u{0414}\u{0415}\u{0416}\u{0417}";
    assert_eq!(scan_text(text).unwrap().co_signal, CoSignal::NonLatinJunk);
}

#[test]
fn channel_word_at_start_and_glued_identifier() {
    let hit = scan_text("analysis to=functions.edit code leftover").unwrap();
    assert_eq!(hit.co_signal, CoSignal::ChannelAdjacency);
    assert!(scan_text("xanalysis to=functions.edit").is_none());
}

#[test]
fn pua_glitch_and_thai_georgian_hangul() {
    let hit = scan_text("\u{e000} to=functions.edit code").unwrap();
    assert_eq!(hit.co_signal, CoSignal::GlitchToken);
    let thai = "to=functions.edit \u{0e01}\u{0e02}\u{0e03}\u{0e04}\u{0e05}\u{0e06}\u{0e07}\u{0e08}";
    assert_eq!(scan_text(thai).unwrap().co_signal, CoSignal::NonLatinJunk);
    let georgian =
        "to=functions.edit \u{10a0}\u{10a1}\u{10a2}\u{10a3}\u{10a4}\u{10a5}\u{10a6}\u{10a7}";
    assert_eq!(
        scan_text(georgian).unwrap().co_signal,
        CoSignal::NonLatinJunk
    );
    let hangul =
        "to=functions.edit \u{ac00}\u{ac01}\u{ac02}\u{ac03}\u{ac04}\u{ac05}\u{ac06}\u{ac07}";
    assert_eq!(scan_text(hangul).unwrap().co_signal, CoSignal::NonLatinJunk);
}

#[test]
fn non_latin_run_survives_spaces_and_resets_on_ascii() {
    let spaced =
        "to=functions.edit \u{8d4c} \u{535a} \u{7f51} \u{7ad9} \u{63a8} \u{8350} \u{4ee3} \u{7406}";
    assert_eq!(scan_text(spaced).unwrap().co_signal, CoSignal::NonLatinJunk);
    assert!(scan_text("to=functions.edit \u{8d4c}\u{535a} abcdefgh").is_none());
}

#[test]
fn escaped_quotes_and_trailing_backslash() {
    assert!(scan_text("foo \\\" analysis to=functions.edit code").is_some());
    assert!(scan_text("x\\to=functions.read").is_none());
}

#[test]
fn truncate_first_line_and_compat_ideograph() {
    let text = "analysis to=functions.edit code leftover";
    let at = text.find(MARKER).unwrap();
    assert_eq!(truncate_at_line(text, at), "");
    let cjk_compat =
        "to=functions.edit \u{f900}\u{f901}\u{f902}\u{f903}\u{f904}\u{f905}\u{f906}\u{f907}";
    assert_eq!(
        scan_text(cjk_compat).unwrap().co_signal,
        CoSignal::NonLatinJunk
    );
}

#[test]
fn leak_between_hunks_is_not_after_last() {
    let patch = "@@ -1 +1 @@\n a\nanalysis to=functions.edit\n@@ -2 +2 @@\n b\n";
    let at = patch.find(MARKER).unwrap();
    assert!(!leak_is_after_last_hunk(patch, at));
}

#[test]
fn escaped_backslash_at_eof_and_odd_backticks() {
    assert!(scan_text("to=functions.read\\").is_none());
    assert!(scan_text("see \\` analysis to=functions.edit code").is_some());
}

#[test]
fn underscore_glue_is_not_channel_adjacent() {
    assert!(scan_text("x_analysis to=functions.edit").is_none());
}

#[test]
fn latin1_supplement_resets_non_latin_run() {
    // é is ASCII-adjacent Latin-1, not junk — run must reset.
    assert!(scan_text("to=functions.edit \u{8d4c}\u{535a}\u{00e9}").is_none());
}

#[test]
fn closed_then_open_fence_is_a_hit() {
    let text = "```\nignore\n```\nanalysis to=functions.edit code leftover";
    assert!(scan_text(text).is_some());
}

#[test]
fn backslash_at_very_end_of_prefix() {
    // Trailing `\` in the quoted-prefix walk (`i += 2` past EOF).
    assert!(scan_text("\\to=functions.read next").is_none());
}

#[test]
fn hunk_line_starting_with_plus_is_mid_hunk() {
    let patch = "@@ -1 +1 @@\n+analysis to=functions.edit code\n";
    let at = patch.find(MARKER).unwrap();
    assert!(!leak_is_after_last_hunk(patch, at));
}
