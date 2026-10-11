//! Byte line buffer for newline-delimited streams (SSE, NDJSON).
//!
//! Network chunks split anywhere, including inside a multi-byte UTF-8
//! character. Decoding each chunk on its own turned such a split into
//! replacement characters (`ş` → `��`). `\n` is one byte that never occurs
//! inside a multi-byte sequence, so splitting raw bytes on it and decoding
//! only complete lines is always safe. Consumed bytes are dropped once per
//! chunk instead of re-copying the whole tail after every line.

#[derive(Debug, Default)]
pub(crate) struct LineBuffer {
    buf: Vec<u8>,
    /// Start of the unread part of `buf`.
    start: usize,
}

impl LineBuffer {
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.buf.drain(..self.start);
        self.start = 0;
        self.buf.extend_from_slice(bytes);
    }

    /// Next complete line without its `\n`, trimmed.
    pub(crate) fn next_line(&mut self) -> Option<String> {
        let rest = &self.buf[self.start..];
        let pos = rest.iter().position(|&b| b == b'\n')?;
        let line = String::from_utf8_lossy(&rest[..pos]).trim().to_string();
        self.start += pos + 1;
        Some(line)
    }

    /// The unterminated tail, trimmed. Empties the buffer.
    pub(crate) fn take_rest(&mut self) -> String {
        let line = String::from_utf8_lossy(&self.buf[self.start..])
            .trim()
            .to_string();
        self.buf.clear();
        self.start = 0;
        line
    }
}

#[cfg(test)]
#[path = "line_buffer_tests.rs"]
mod tests;
