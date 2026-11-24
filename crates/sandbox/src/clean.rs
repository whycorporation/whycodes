//! Terminal noise removal for command output shown to the model.
//!
//! A shell transcript is written for a terminal: colour codes, cursor moves,
//! progress bars redrawn with `\r`, and the same line printed hundreds of
//! times, or hundreds of lines that differ only in a counter. None of it carries meaning for the model, and all of it costs
//! tokens on every later request. This keeps what a terminal would end up
//! showing — every error, warning and summary line — and drops the rest.
//! It runs once, when the tool result is made, so stored history never
//! changes afterwards (the provider prompt cache stays valid).

/// A run of identical lines this long or longer collapses to one line.
const REPEAT_COLLAPSE: usize = 3;

/// Strip ANSI sequences, resolve `\r` redraws, collapse repeated lines.
pub fn clean_terminal_output(raw: &str) -> String {
    let plain = strip_ansi(raw);
    let mut out = String::with_capacity(plain.len());
    let mut last: Option<&str> = None;
    let mut repeats = 0usize;
    for line in plain.split('\n') {
        let shown = final_redraw(line);
        // Blank lines are layout, not noise: never collapsed.
        if last == Some(shown) && !shown.is_empty() {
            repeats += 1;
            continue;
        }
        flush_repeats(&mut out, repeats);
        push_line(&mut out, shown);
        last = Some(shown);
        repeats = 0;
    }
    flush_repeats(&mut out, repeats);
    collapse_similar(&out)
}

/// A run of lines that differ only in their numbers this long or longer
/// keeps its first and last line and notes how many were between.
const SIMILAR_COLLAPSE: usize = 8;

/// Collapse long runs like `test case_1 ... ok` … `test case_4000 ... ok`.
/// A line that breaks the shape (`test case_77 ... FAILED`) ends the run and
/// is kept, so nothing but the repetition is lost.
fn collapse_similar(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut kept: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let shape = digit_shape(lines[i]);
        let mut end = i + 1;
        while shape.is_some() && end < lines.len() && digit_shape(lines[end]) == shape {
            end += 1;
        }
        let run = &lines[i..end];
        if run.len() >= SIMILAR_COLLAPSE {
            kept.push(run[0].to_string());
            kept.push(format!(
                "[{} similar lines; only the numbers differ]",
                run.len() - 2
            ));
            kept.push(run[run.len() - 1].to_string());
        } else {
            kept.extend(run.iter().map(|l| l.to_string()));
        }
        i = end;
    }
    kept.join("\n")
}

/// The line with every run of digits replaced by `#`, or `None` when it has
/// no digits or no letters (a data row of bare numbers is content, not noise).
fn digit_shape(line: &str) -> Option<String> {
    let has_digit = line.bytes().any(|b| b.is_ascii_digit());
    let has_letter = line.bytes().any(|b| b.is_ascii_alphabetic());
    if !has_digit || !has_letter {
        return None;
    }
    let mut shape = String::with_capacity(line.len());
    let mut in_digits = false;
    for c in line.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                shape.push('#');
            }
            in_digits = true;
        } else {
            shape.push(c);
            in_digits = false;
        }
    }
    Some(shape)
}

/// What a terminal shows for one line: the text after its last bare `\r`.
/// A trailing `\r` is a CRLF line ending, not a redraw.
fn final_redraw(line: &str) -> &str {
    let line = line.strip_suffix('\r').unwrap_or(line);
    match line.rfind('\r') {
        Some(i) => &line[i + 1..],
        None => line,
    }
}

fn push_line(out: &mut String, line: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(line);
}

/// Note the copies of the previous line that were skipped. Short runs are
/// written back as they were; only a long run becomes a note.
fn flush_repeats(out: &mut String, repeats: usize) {
    if repeats + 1 >= REPEAT_COLLAPSE {
        out.push_str(&format!("  [same line repeated {repeats} more times]"));
        return;
    }
    let last = out.rsplit('\n').next().unwrap_or("").to_string();
    for _ in 0..repeats {
        push_line(out, &last);
    }
}

/// Remove CSI (`ESC [ … final`), OSC (`ESC ] … BEL|ESC \`) and two-byte
/// escape sequences.
fn strip_ansi(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => skip_csi(&mut chars),
            Some(']') => skip_osc(&mut chars),
            _ => {}
        }
    }
    out
}

fn skip_csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for c in chars.by_ref() {
        if ('@'..='~').contains(&c) {
            return;
        }
    }
}

fn skip_osc(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(c) = chars.next() {
        if c == '\x07' {
            return;
        }
        if c == '\x1b' && chars.peek() == Some(&'\\') {
            chars.next();
            return;
        }
    }
}

#[cfg(test)]
#[path = "clean_tests.rs"]
mod tests;
