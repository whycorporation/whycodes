//! Text normalisation that lets `edit` / `apply_patch` find what the model
//! meant: CRLF files matched with `\n` text, and typographic punctuation
//! (curly quotes, dashes, non-breaking spaces) matched with its ASCII form.

/// True when most line breaks in `text` are `\r\n`.
pub(crate) fn is_crlf(text: &str) -> bool {
    let crlf = text.matches("\r\n").count();
    crlf > 0 && crlf * 2 >= text.matches('\n').count()
}

/// `text` with every line break written as `\r\n`.
pub(crate) fn to_crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// The ASCII stand-in for a typographic character, or `c` itself.
fn fold_char(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{00A0}' | '\u{2007}' | '\u{202F}' => ' ',
        _ => c,
    }
}

/// `text` with typographic punctuation folded to ASCII, and for every byte of
/// the folded text the byte offset in `text` it came from. The map has one
/// extra entry, `text.len()`, so a span end at the folded length maps back.
pub(crate) fn fold(text: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut map = Vec::with_capacity(text.len() + 1);
    for (offset, c) in text.char_indices() {
        let folded = fold_char(c);
        out.push(folded);
        map.extend(std::iter::repeat_n(offset, folded.len_utf8()));
    }
    map.push(text.len());
    (out, map)
}

#[cfg(test)]
#[path = "tolerant_tests.rs"]
mod tests;
