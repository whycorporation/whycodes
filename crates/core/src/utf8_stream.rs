//! UTF-8 text from a byte stream that splits characters across chunks.
//!
//! An HTTP body arrives in chunks cut at arbitrary bytes. Decoding each
//! chunk on its own turns a character split between two chunks (any
//! non-ASCII text: `ç`, `日`, emoji) into two replacement characters.

/// Decodes chunks as they come, holding back an incomplete trailing
/// character until the next chunk completes it.
#[derive(Debug, Default, Clone)]
pub struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    pub fn new() -> Self {
        Self::default()
    }

    /// The text `chunk` completes. Invalid bytes become U+FFFD right away;
    /// only a character cut at the end of the chunk waits.
    pub fn push(&mut self, chunk: &[u8]) -> String {
        self.pending.extend_from_slice(chunk);
        let complete = match std::str::from_utf8(&self.pending) {
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            _complete_or_invalid => self.pending.len(),
        };
        let text = String::from_utf8_lossy(&self.pending[..complete]).into_owned();
        self.pending.drain(..complete);
        text
    }

    /// Whatever is still held back when the stream ends (a truncated
    /// character), decoded lossily.
    pub fn finish(&mut self) -> String {
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_split_across_chunks_decodes_whole() {
        let bytes = "aç日😀".as_bytes();
        let mut stream = Utf8Stream::new();
        let mut out = String::new();
        for b in bytes {
            out.push_str(&stream.push(&[*b]));
        }
        out.push_str(&stream.finish());
        assert_eq!(out, "aç日😀");
    }

    #[test]
    fn invalid_bytes_and_a_truncated_end_become_replacement_chars() {
        let mut stream = Utf8Stream::new();
        assert_eq!(stream.push(b"a\xffb"), "a\u{FFFD}b");
        assert_eq!(stream.push(&[0xe6, 0x97]), "");
        assert_eq!(stream.finish(), "\u{FFFD}");
        assert_eq!(stream.finish(), "");
    }
}
