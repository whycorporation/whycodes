//! Minimal Server-Sent Events (SSE) parser for MCP transports.

use std::collections::VecDeque;

/// A single SSE event (WHATWG event-stream interpretation, simplified).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
}

/// Incremental SSE parser. Feed raw bytes / text; drain complete events.
#[derive(Debug, Default)]
pub struct SseParser {
    buf: String,
    event_name: Option<String>,
    data_lines: Vec<String>,
    id: Option<String>,
    pending: VecDeque<SseEvent>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &str) {
        self.buf.push_str(chunk);
        self.drain_lines();
    }

    pub fn take_events(&mut self) -> Vec<SseEvent> {
        self.pending.drain(..).collect()
    }

    fn drain_lines(&mut self) {
        loop {
            let Some(newline_at) = self.buf.find('\n') else {
                return;
            };
            let mut line = self.buf.drain(..=newline_at).collect::<String>();
            if line.ends_with('\n') {
                line.pop();
            }
            if line.ends_with('\r') {
                line.pop();
            }

            if line.is_empty() {
                self.dispatch_event();
                continue;
            }

            if line.starts_with(':') {
                continue;
            }

            let (field, value) = match line.split_once(':') {
                Some((f, v)) => {
                    let v = v.strip_prefix(' ').unwrap_or(v);
                    (f, v)
                }
                None => (line.as_str(), ""),
            };

            match field {
                "event" => self.event_name = Some(value.to_string()),
                "data" => self.data_lines.push(value.to_string()),
                "id" => {
                    if value.is_empty() {
                        self.id = None;
                    } else {
                        self.id = Some(value.to_string());
                    }
                }
                "retry" => {}
                _ => {}
            }
        }
    }

    fn dispatch_event(&mut self) {
        if self.data_lines.is_empty() && self.event_name.is_none() && self.id.is_none() {
            return;
        }
        let data = self.data_lines.join("\n");
        let event = SseEvent {
            event: self.event_name.take(),
            data,
            id: self.id.clone(),
        };
        self.data_lines.clear();
        self.pending.push_back(event);
    }
}

/// Parse a complete SSE body (one-shot response) into events.
pub fn parse_sse_body(body: &str) -> Vec<SseEvent> {
    let mut parser = SseParser::new();
    parser.push(body);
    if !parser.data_lines.is_empty() || parser.event_name.is_some() {
        parser.dispatch_event();
    }
    parser.take_events()
}

#[cfg(test)]
#[path = "sse_tests.rs"]
mod tests;
