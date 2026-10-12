//! Tool call / result pairing for outgoing requests.
//!
//! Strict APIs reject a transcript where an assistant tool call has no
//! result (OpenAI: "tool_calls must be followed by tool messages"; Anthropic:
//! every `tool_use` needs a `tool_result`) or where a tool result answers no
//! call. The agent now answers interrupted calls itself, but sessions saved
//! before that, imported transcripts and trimmed histories can still carry
//! either shape. This repairs the request copy only; the stored transcript
//! is left as the user saw it.

use whycodes_core::types::{ContentBlock, Message, MessageContent, Role};

/// Body of the result supplied for a call the transcript never answered.
pub const MISSING_RESULT: &str = "Tool result missing: this call was interrupted and never ran.";

/// The transcript with every tool call answered and every result owned by a
/// call. `None` when it already pairs up, so the common case copies nothing.
pub fn paired_messages(messages: &[Message]) -> Option<Vec<Message>> {
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    let mut open: Vec<String> = Vec::new();
    // A call with an empty id cannot be matched by id; keep whatever results
    // follow that assistant turn rather than dropping real output.
    let mut lenient = false;
    let mut changed = false;
    for msg in messages {
        match msg.role {
            Role::Tool => {
                let id = msg.tool_call_id.as_deref().unwrap_or("");
                let owned = take_open(&mut open, id) || lenient;
                changed |= !owned;
                push_if(&mut out, msg, owned);
            }
            Role::Assistant => {
                changed |= close_open(&mut out, &mut open);
                lenient = record_calls(msg, &mut open);
                out.push(msg.clone());
            }
            Role::User | Role::System => {
                changed |= close_open(&mut out, &mut open);
                lenient = false;
                out.push(msg.clone());
            }
        }
    }
    changed |= close_open(&mut out, &mut open);
    changed.then_some(out)
}

/// Remove `id` from the open calls; true when it was one of them.
fn take_open(open: &mut Vec<String>, id: &str) -> bool {
    let before = open.len();
    open.retain(|o| o != id);
    open.len() != before
}

fn push_if(out: &mut Vec<Message>, msg: &Message, keep: bool) {
    if keep {
        out.push(msg.clone());
    }
}

/// Note the assistant's tool call ids; true when one had no id.
fn record_calls(msg: &Message, open: &mut Vec<String>) -> bool {
    let MessageContent::Blocks(blocks) = &msg.content else {
        return false;
    };
    let mut unnamed = false;
    for block in blocks {
        if let ContentBlock::ToolUse { id, .. } = block {
            unnamed |= id.is_empty();
            open.push(id.clone());
        }
    }
    open.retain(|id| !id.is_empty());
    unnamed
}

/// Answer every still-open call; true when any needed answering.
fn close_open(out: &mut Vec<Message>, open: &mut Vec<String>) -> bool {
    let any = !open.is_empty();
    for id in open.drain(..) {
        out.push(Message {
            role: Role::Tool,
            content: MessageContent::Text(MISSING_RESULT.to_string()),
            tool_call_id: Some(id),
            name: None,
            created_at: None,
        });
    }
    any
}

#[cfg(test)]
#[path = "pairing_tests.rs"]
mod tests;
