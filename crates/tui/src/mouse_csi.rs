//! Parse SGR mouse reports that Windows ConPTY leaked as text.
//!
//! After a failed turn, ConPTY can emit `ESC[<btn;x;yM` as `Key::Char`
//! (`<65;NaN;NaNM[`) instead of `Event::Mouse`. Keep this matcher out of
//! `input.rs` so llvm-cov `show` expansions do not inflate the workspace
//! 82% floor.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MouseCsi {
    Complete(usize),
    Prefix,
    Invalid,
}

pub(crate) fn parse_mouse_csi(s: &str) -> MouseCsi {
    let b = s.as_bytes();
    if b.is_empty() {
        return MouseCsi::Invalid;
    }
    let mut i = 0;
    if b[0] == b'[' {
        i = 1;
        if i == b.len() {
            return MouseCsi::Prefix;
        }
    }
    if i >= b.len() || b[i] != b'<' {
        return MouseCsi::Invalid;
    }
    i += 1;
    if i == b.len() {
        return MouseCsi::Prefix;
    }
    let body = i;
    while i < b.len() {
        match b[i] {
            b'0'..=b'9' | b';' => i += 1,
            b'N' => {
                let rest = &b[i..];
                if rest.starts_with(b"NaN") {
                    i += 3;
                } else if rest == b"N" || rest == b"Na" {
                    return MouseCsi::Prefix;
                } else {
                    return MouseCsi::Invalid;
                }
            }
            b'M' | b'm' if i > body => return MouseCsi::Complete(i + 1),
            _ => return MouseCsi::Invalid,
        }
    }
    MouseCsi::Prefix
}

pub(crate) fn strip_leaked_mouse_csi(s: &str) -> String {
    let mut rest = s;
    let mut stripped = false;
    loop {
        match parse_mouse_csi(rest) {
            MouseCsi::Complete(n) => {
                rest = &rest[n..];
                stripped = true;
            }
            MouseCsi::Prefix if stripped && rest.starts_with('[') => {
                rest = &rest[1..];
            }
            MouseCsi::Invalid if stripped && rest.starts_with('[') => {
                rest = &rest[1..];
                stripped = true;
            }
            MouseCsi::Prefix if stripped => {
                rest = "";
            }
            _ => break,
        }
    }
    rest.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_and_bare_prefix() {
        assert_eq!(parse_mouse_csi(""), MouseCsi::Invalid);
        assert_eq!(parse_mouse_csi("["), MouseCsi::Prefix);
        assert_eq!(parse_mouse_csi("<"), MouseCsi::Prefix);
        assert_eq!(parse_mouse_csi("[<"), MouseCsi::Prefix);
        assert_eq!(parse_mouse_csi("x"), MouseCsi::Invalid);
        assert_eq!(parse_mouse_csi("[x"), MouseCsi::Invalid);
    }

    #[test]
    fn parse_digits_nan_and_terminator() {
        assert_eq!(parse_mouse_csi("<65;"), MouseCsi::Prefix);
        assert_eq!(parse_mouse_csi("<65;N"), MouseCsi::Prefix);
        assert_eq!(parse_mouse_csi("<65;Na"), MouseCsi::Prefix);
        assert_eq!(parse_mouse_csi("<65;Nx"), MouseCsi::Invalid);
        assert_eq!(parse_mouse_csi("<M"), MouseCsi::Invalid);
        assert_eq!(parse_mouse_csi("<65M"), MouseCsi::Complete(4));
        assert_eq!(parse_mouse_csi("<65m"), MouseCsi::Complete(4));
        assert_eq!(parse_mouse_csi("<65;NaN;NaNM"), MouseCsi::Complete(12));
        assert_eq!(parse_mouse_csi("[<65;NaN;NaNM"), MouseCsi::Complete(13));
        assert_eq!(parse_mouse_csi("<65;NaN;NaNMx"), MouseCsi::Complete(12));
    }

    #[test]
    fn strip_reports_and_trailing_bracket() {
        assert_eq!(strip_leaked_mouse_csi("hello"), "hello");
        assert_eq!(
            strip_leaked_mouse_csi("<65;NaN;NaNM[<65;NaN;NaNM[hello"),
            "hello"
        );
        assert_eq!(strip_leaked_mouse_csi("<65;NaN;NaNM["), "");
        assert_eq!(
            strip_leaked_mouse_csi("<65;NaN"),
            "<65;NaN",
            "an incomplete report alone is ordinary text"
        );
        assert_eq!(strip_leaked_mouse_csi("<65;NaN;NaNM[x"), "x");
    }
}
