//! Lines added / removed by this session's own file edits.
//!
//! `write`, `edit` and `apply_patch` end a successful result with a
//! `[+N -M]` trailer (`whycodes_core::edit_trailer`); the agent feeds every
//! tool result through [`EditStats::record`] and `/info` shows the sum. The
//! counts are block-level (an `edit` counts the whole replaced text, an
//! overwrite counts the whole old file), so they are labelled approximate
//! as soon as one of those two tools contributed.

use serde::{Deserialize, Serialize};
use whycodes_core::types::{ContentBlock, Message, MessageContent, Role};

/// Cumulative line counts from the session's file-mutating tool results.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditStats {
    pub added: u64,
    pub removed: u64,
    /// True when a block-level count (`edit`, `write`) is included or a
    /// counted result had lost its trailer (truncated, spilled, failed).
    pub approximate: bool,
}

/// Tools whose results carry a `[+N -M]` trailer.
const COUNTED_TOOLS: [&str; 3] = ["write", "edit", "apply_patch"];

impl EditStats {
    pub fn is_empty(&self) -> bool {
        self.added == 0 && self.removed == 0
    }

    /// Fold one tool result in. Tools without a trailer contract are
    /// ignored; a counted tool whose result lost its trailer marks the
    /// total approximate instead of silently undercounting.
    pub fn record(&mut self, tool: &str, content: &str) {
        if !COUNTED_TOOLS.contains(&tool) {
            return;
        }
        match whycodes_core::parse_edit_trailers(content) {
            Some((added, removed)) => {
                self.added += added;
                self.removed += removed;
                if tool != "apply_patch" {
                    self.approximate = true;
                }
            }
            None => self.approximate = true,
        }
    }

    /// Rebuild the counts from a persisted transcript: every assistant
    /// tool call to a counted tool, paired with its result by call id.
    pub fn from_messages(messages: &[Message]) -> Self {
        let mut stats = Self::default();
        for (i, msg) in messages.iter().enumerate() {
            if msg.role != Role::Assistant {
                continue;
            }
            let MessageContent::Blocks(blocks) = &msg.content else {
                continue;
            };
            for block in blocks {
                let ContentBlock::ToolUse { id, name, .. } = block else {
                    continue;
                };
                if !COUNTED_TOOLS.contains(&name.as_str()) {
                    continue;
                }
                let result = messages[i + 1..]
                    .iter()
                    .find(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some(id))
                    .and_then(|m| m.content.as_text())
                    .unwrap_or("");
                stats.record(name, result);
            }
        }
        stats
    }

    /// `+12 -3 lines`, with the approximate label when it applies.
    pub fn label(&self) -> String {
        let mut out = format!("+{} -{} lines", self.added, self.removed);
        if self.approximate {
            out.push_str(" (approximate)");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant_call(id: &str, name: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input: serde_json::json!({}),
            }]),
            tool_call_id: None,
            name: None,
            created_at: None,
        }
    }

    fn tool_result(id: &str, text: &str) -> Message {
        Message {
            role: Role::Tool,
            content: MessageContent::Text(text.into()),
            tool_call_id: Some(id.into()),
            name: None,
            created_at: None,
        }
    }

    #[test]
    fn record_sums_counted_tools_only() {
        let mut s = EditStats::default();
        assert!(s.is_empty());
        s.record("bash", "[+99 -99]");
        assert!(s.is_empty());
        s.record("apply_patch", "Patch applied to `a` (1 hunk).\n[+2 -1]");
        assert_eq!(
            s,
            EditStats {
                added: 2,
                removed: 1,
                approximate: false
            }
        );
        assert_eq!(s.label(), "+2 -1 lines");
        s.record("write", "Wrote a.rs  ·  3 lines\n\n[+3 -0]");
        assert!(s.approximate);
        assert_eq!(s.label(), "+5 -1 lines (approximate)");

        // A counted tool without a trailer (error, truncated) is flagged.
        let mut lost = EditStats::default();
        lost.record("edit", "Error writing file: denied");
        assert!(lost.is_empty());
        assert!(lost.approximate);
    }

    #[test]
    fn from_messages_pairs_calls_with_results_by_id() {
        let messages = vec![
            Message {
                role: Role::User,
                content: MessageContent::Text("hi".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Text("plain".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            assistant_call("c1", "apply_patch"),
            assistant_call("c2", "read"),
            tool_result("c2", "contents"),
            tool_result("c1", "Patch applied to `a` (1 hunk).\n[+4 -2]"),
            assistant_call("c3", "edit"),
        ];
        let stats = EditStats::from_messages(&messages);
        // c3 never got a result: counted as lost, hence approximate.
        assert_eq!(
            stats,
            EditStats {
                added: 4,
                removed: 2,
                approximate: true
            }
        );
        assert_eq!(EditStats::from_messages(&[]), EditStats::default());
    }
}
