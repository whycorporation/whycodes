use super::*;

#[test]
fn colour_and_cursor_codes_are_removed() {
    let raw = "\x1b[1m\x1b[32m   Compiling\x1b[0m whycodes v0.6.7\n\x1b[2K\x1b[1Gdone";
    assert_eq!(
        clean_terminal_output(raw),
        "   Compiling whycodes v0.6.7\ndone"
    );
}

#[test]
fn osc_titles_and_two_byte_escapes_are_removed() {
    let raw = "\x1b]0;window title\x07a\x1b]8;;https://x\x1b\\link\x1b]8;;\x1b\\b\x1b=c\x1b";
    assert_eq!(clean_terminal_output(raw), "alinkbc");
    // An unterminated sequence at the end eats the rest, never panics.
    assert_eq!(clean_terminal_output("ok\x1b[31"), "ok");
    assert_eq!(clean_terminal_output("ok\x1b]0;title"), "ok");
}

#[test]
fn progress_redraws_keep_only_what_the_terminal_shows() {
    let raw = "Downloading  10%\rDownloading  55%\rDownloading 100%\nInstalled 3 packages";
    assert_eq!(
        clean_terminal_output(raw),
        "Downloading 100%\nInstalled 3 packages"
    );
    // CRLF line endings are not redraws.
    assert_eq!(clean_terminal_output("a\r\nb\r\n"), "a\nb\n");
}

#[test]
fn long_runs_of_one_line_collapse_and_short_runs_stay() {
    let mut raw = String::from("start\n");
    for _ in 0..50 {
        raw.push_str("waiting for lock\n");
    }
    raw.push_str("error[E0425]: cannot find value\nok\nok\nend");
    assert_eq!(
        clean_terminal_output(&raw),
        "start\nwaiting for lock  [same line repeated 49 more times]\n\
         error[E0425]: cannot find value\nok\nok\nend"
    );
    assert_eq!(
        clean_terminal_output("x\nx\nx"),
        "x  [same line repeated 2 more times]"
    );
}

#[test]
fn plain_output_passes_through_unchanged() {
    let raw = "line one\n\nline three\ttabbed\n";
    assert_eq!(clean_terminal_output(raw), raw);
    assert_eq!(clean_terminal_output("a\n\n\n\nb"), "a\n\n\n\nb");
    assert_eq!(clean_terminal_output(""), "");
    // Leading blank lines are layout too.
    assert_eq!(clean_terminal_output("\n\nx"), "\n\nx");
    assert_eq!(clean_terminal_output("\nx\nx"), "\nx\nx");
}

#[test]
fn long_runs_that_differ_only_in_numbers_keep_their_ends() {
    let mut raw = String::from("running 4000 tests\n");
    for n in 1..=76 {
        raw.push_str(&format!("test case_{n} ... ok\n"));
    }
    raw.push_str("test case_77 ... FAILED\n");
    for n in 78..=4000 {
        raw.push_str(&format!("test case_{n} ... ok\n"));
    }
    raw.push_str("failures: case_77");
    assert_eq!(
        clean_terminal_output(&raw),
        "running 4000 tests\n\
         test case_1 ... ok\n[74 similar lines; only the numbers differ]\ntest case_76 ... ok\n\
         test case_77 ... FAILED\n\
         test case_78 ... ok\n[3921 similar lines; only the numbers differ]\ntest case_4000 ... ok\n\
         failures: case_77"
    );
}

#[test]
fn short_similar_runs_and_bare_numbers_stay() {
    let seven: Vec<String> = (1..=7).map(|n| format!("step {n} done")).collect();
    let seven = seven.join("\n");
    assert_eq!(clean_terminal_output(&seven), seven);
    let rows: Vec<String> = (1..=20).map(|n| format!("{n},{}", n * 2)).collect();
    let rows = rows.join("\n");
    assert_eq!(clean_terminal_output(&rows), rows);
}
