//! Split `<think>…</think>` that some open models put at the start of
//! `content` (instead of a `reasoning` field) into thinking events.
//!
//! Only a tag that opens the response counts; `<think>` later in the text is
//! left alone. The tag may arrive split across chunks, so up to one tag's
//! worth of text is held back. A tag that never closes, in a response with no
//! other text, is also emitted as the answer at the end so nothing is lost.

use whycodes_core::types::StreamEvent;

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

#[derive(Debug, Default, PartialEq, Eq)]
enum State {
    /// Nothing decided yet: leading text could still be `<think>`.
    #[default]
    Start,
    Inside,
    After,
}

#[derive(Debug, Default)]
pub(crate) struct ThinkTags {
    state: State,
    held: String,
    thought: String,
    answered: bool,
}

impl ThinkTags {
    /// Route one `content` delta.
    pub(crate) fn push(&mut self, text: &str) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        match self.state {
            State::After => self.answer(text.to_string(), &mut out),
            State::Inside => self.inside(text, &mut out),
            State::Start => {
                self.held.push_str(text);
                let lead = self.held.trim_start();
                if let Some(rest) = lead.strip_prefix(OPEN) {
                    let rest = rest.to_string();
                    self.held.clear();
                    self.state = State::Inside;
                    self.inside(&rest, &mut out);
                } else if !OPEN.starts_with(lead) {
                    self.state = State::After;
                    let held = std::mem::take(&mut self.held);
                    self.answer(held, &mut out);
                }
            }
        }
        out
    }

    /// Flush at the end of the response.
    pub(crate) fn finish(&mut self) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        let held = std::mem::take(&mut self.held);
        match self.state {
            State::Start => self.answer(held, &mut out),
            State::Inside => {
                self.think(held, &mut out);
                let thought = self.thought.trim();
                if !self.answered && !thought.is_empty() {
                    out.push(StreamEvent::TextDelta {
                        text: thought.to_string(),
                    });
                }
            }
            State::After => {}
        }
        self.state = State::After;
        out
    }

    fn inside(&mut self, text: &str, out: &mut Vec<StreamEvent>) {
        self.held.push_str(text);
        if let Some(at) = self.held.find(CLOSE) {
            let rest = self.held[at + CLOSE.len()..].trim_start().to_string();
            self.held.truncate(at);
            let thought = std::mem::take(&mut self.held);
            self.think(thought, out);
            self.state = State::After;
            self.answer(rest, out);
            return;
        }
        // Keep a tail that could be the start of `</think>`.
        let keep = (1..CLOSE.len())
            .rev()
            .find(|&n| self.held.ends_with(&CLOSE[..n]))
            .unwrap_or(0);
        let tail = self.held.split_off(self.held.len() - keep);
        let thought = std::mem::replace(&mut self.held, tail);
        self.think(thought, out);
    }

    fn think(&mut self, text: String, out: &mut Vec<StreamEvent>) {
        if !text.is_empty() {
            self.thought.push_str(&text);
            out.push(StreamEvent::ThinkingDelta { text });
        }
    }

    fn answer(&mut self, text: String, out: &mut Vec<StreamEvent>) {
        if !text.is_empty() {
            self.answered = true;
            out.push(StreamEvent::TextDelta { text });
        }
    }
}

#[cfg(test)]
#[path = "think_tags_tests.rs"]
mod tests;
