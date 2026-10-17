//! Branches inside one conversation (`/branch`, `/tree`).
//!
//! [`Session::messages`] stays the active path: the transcript shown and sent
//! to the model. Every other path is a [`Branch`], a full root-to-leaf copy,
//! so compaction, spill and undo on the active path never reach it. The tree
//! is rebuilt on demand by merging paths whose turns match.
//!
//! Turns match by a fingerprint that leaves out tool-result bodies, because
//! pruning and spilling shorten those in place on the active path. A path
//! whose prefix was compacted into a summary no longer matches its siblings
//! and shows as its own line from the root.

use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use whycodes_core::types::{ContentBlock, Message, MessageContent, Role};

use crate::session::Session;

/// Longest label shown for one turn (first line of its user message).
const LABEL_MAX_CHARS: usize = 72;

/// A stored path other than the active one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    /// Set by `/branch <name>`; `None` for a path kept by a switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Every message from the root to this branch's leaf.
    pub messages: Vec<Message>,
}

/// A node to switch to: the first `end` messages of one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeTarget {
    /// 0 is the active path; `n` is `branches[n - 1]`.
    path: usize,
    end: usize,
}

/// One user turn in the tree, in display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    /// Indent level; it grows only where the conversation forks.
    pub depth: usize,
    /// This turn is one of several alternatives after the same point.
    pub fork_start: bool,
    /// First line of the turn's user message.
    pub label: String,
    /// Names of branches marked at this turn.
    pub names: Vec<String>,
    /// This turn is on the active path.
    pub on_active_path: bool,
    /// This turn is the active path's last turn.
    pub active_leaf: bool,
    pub target: TreeTarget,
}

impl TreeRow {
    /// One-line rendering shared by the TUI picker and the plain REPL.
    pub fn line(&self) -> String {
        let marker = if self.active_leaf {
            '●'
        } else if self.on_active_path {
            '•'
        } else {
            '○'
        };
        let indent = if self.fork_start {
            format!("{}↳ ", "  ".repeat(self.depth.saturating_sub(1)))
        } else {
            "  ".repeat(self.depth)
        };
        let mut out = format!("{indent}{marker} {}", self.label);
        if !self.names.is_empty() {
            out.push_str(&format!("  [{}]", self.names.join(", ")));
        }
        out
    }
}

/// Why a branch command could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchError {
    /// `/branch` on a conversation with no messages yet.
    Empty,
    /// The tree changed since the picker was built.
    Stale,
}

impl std::fmt::Display for BranchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Empty => "nothing to branch from yet",
            Self::Stale => "the conversation changed; open /tree again",
        })
    }
}

impl std::error::Error for BranchError {}

impl Session {
    /// Mark the current point under `name` (or `branch N`). Returns the name.
    pub fn mark_branch(&mut self, name: &str) -> Result<String, BranchError> {
        if self.messages.is_empty() {
            return Err(BranchError::Empty);
        }
        let name = match name.trim() {
            "" => format!("branch {}", self.branches.len() + 1),
            given => given.to_string(),
        };
        self.branches.push(Branch {
            name: Some(name.clone()),
            messages: self.messages.clone(),
        });
        Ok(name)
    }

    /// The conversation as a tree of user turns, active path first.
    pub fn tree_rows(&self) -> Vec<TreeRow> {
        let mut paths: Vec<(&[Message], Option<&str>)> = vec![(&self.messages, None)];
        for b in &self.branches {
            paths.push((&b.messages, b.name.as_deref()));
        }
        let mut trie = Trie::default();
        for (path, (messages, name)) in paths.into_iter().enumerate() {
            trie.insert(path, messages, name);
        }
        trie.rows()
    }

    /// Make `target` the active leaf. The old active path is kept as a
    /// branch unless another path already holds all of it. Returns `false`
    /// when `target` already is the active path.
    pub fn switch_branch(&mut self, target: TreeTarget) -> Result<bool, BranchError> {
        let source = match target.path {
            0 => &self.messages,
            n => match self.branches.get(n - 1) {
                Some(b) => &b.messages,
                None => return Err(BranchError::Stale),
            },
        };
        let Some(next) = source.get(..target.end) else {
            return Err(BranchError::Stale);
        };
        let next = next.to_vec();
        if same_path(&next, &self.messages) {
            return Ok(false);
        }
        // An unnamed branch reached at its leaf moves into the active slot.
        let moves = target.path.checked_sub(1).filter(|&i| {
            let b = &self.branches[i];
            b.name.is_none() && b.messages.len() == target.end
        });
        if let Some(i) = moves {
            self.branches.remove(i);
        }
        let old = std::mem::take(&mut self.messages);
        let kept =
            is_prefix(&old, &next) || self.branches.iter().any(|b| is_prefix(&old, &b.messages));
        if !kept {
            self.branches.push(Branch {
                name: None,
                messages: old,
            });
        }
        // Checkpoint indexes point into the old path.
        self.checkpoint = None;
        self.set_messages(next);
        Ok(true)
    }
}

/// Stable identity of a message for matching turns across paths. Tool
/// bodies are left out: prune / shake / spill rewrite them in place.
fn fingerprint(m: &Message) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::mem::discriminant(&m.role).hash(&mut h);
    m.tool_call_id.hash(&mut h);
    m.name.hash(&mut h);
    if m.role != Role::Tool {
        match &m.content {
            MessageContent::Text(t) => t.hash(&mut h),
            MessageContent::Blocks(blocks) => {
                for b in blocks {
                    hash_block(b, &mut h);
                }
            }
        }
    }
    h.finish()
}

fn hash_block(b: &ContentBlock, h: &mut impl Hasher) {
    std::mem::discriminant(b).hash(h);
    match b {
        ContentBlock::Text { text } | ContentBlock::Thinking { text, .. } => text.hash(h),
        ContentBlock::ToolUse { id, name, .. } => (id, name).hash(h),
        ContentBlock::ToolResult { tool_use_id, .. } => tool_use_id.hash(h),
        ContentBlock::RedactedThinking { data } => data.hash(h),
        ContentBlock::Image { .. } => {}
        ContentBlock::Compaction { content, .. } => content.hash(h),
    }
}

/// `a` is `b` or a leading part of it.
fn is_prefix(a: &[Message], b: &[Message]) -> bool {
    a.len() <= b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| fingerprint(x) == fingerprint(y))
}

fn same_path(a: &[Message], b: &[Message]) -> bool {
    a.len() == b.len() && is_prefix(a, b)
}

/// `(start, end)` of each user turn. Messages before the first user message
/// belong to the first turn.
fn turns(messages: &[Message]) -> Vec<(usize, usize)> {
    let mut starts: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == Role::User)
        .map(|(i, _)| i)
        .collect();
    match starts.first_mut() {
        Some(first) => *first = 0,
        None if !messages.is_empty() => starts.push(0),
        None => {}
    }
    let ends = starts.iter().skip(1).copied().chain([messages.len()]);
    starts.iter().copied().zip(ends).collect()
}

fn turn_label(turn: &[Message]) -> String {
    let text = turn
        .iter()
        .find(|m| m.role == Role::User)
        .and_then(|m| m.content.as_text())
        .and_then(|t| t.lines().map(str::trim).find(|l| !l.is_empty()));
    match text {
        Some(line) if line.chars().count() > LABEL_MAX_CHARS => {
            let cut: String = line.chars().take(LABEL_MAX_CHARS - 1).collect();
            format!("{cut}…")
        }
        Some(line) => line.to_string(),
        None => "(no text)".to_string(),
    }
}

struct Node {
    key: Vec<u64>,
    row: TreeRow,
    children: Vec<usize>,
}

#[derive(Default)]
struct Trie {
    nodes: Vec<Node>,
    roots: Vec<usize>,
}

impl Trie {
    fn insert(&mut self, path: usize, messages: &[Message], name: Option<&str>) {
        let mut parent: Option<usize> = None;
        for (start, end) in turns(messages) {
            let key: Vec<u64> = messages[start..end].iter().map(fingerprint).collect();
            let siblings = match parent {
                Some(p) => &self.nodes[p].children,
                None => &self.roots,
            };
            let found = siblings.iter().copied().find(|&n| self.nodes[n].key == key);
            let id = match found {
                Some(id) => id,
                None => self.push(parent, key, &messages[start..end], path, end),
            };
            self.nodes[id].row.on_active_path |= path == 0;
            parent = Some(id);
        }
        if let Some(leaf) = parent {
            let row = &mut self.nodes[leaf].row;
            row.active_leaf |= path == 0;
            row.names.extend(name.map(str::to_string));
        }
    }

    fn push(
        &mut self,
        parent: Option<usize>,
        key: Vec<u64>,
        turn: &[Message],
        path: usize,
        end: usize,
    ) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node {
            key,
            row: TreeRow {
                depth: 0,
                fork_start: false,
                label: turn_label(turn),
                names: Vec::new(),
                on_active_path: false,
                active_leaf: false,
                target: TreeTarget { path, end },
            },
            children: Vec::new(),
        });
        match parent {
            Some(p) => self.nodes[p].children.push(id),
            None => self.roots.push(id),
        }
        id
    }

    /// Depth-first rows. A lone child stays at its parent's depth; only a
    /// fork indents, so a long linear conversation stays flat.
    fn rows(mut self) -> Vec<TreeRow> {
        let fork = self.roots.len() > 1;
        let mut stack: Vec<(usize, usize, bool)> = self
            .roots
            .iter()
            .rev()
            .map(|&r| (r, usize::from(fork), fork))
            .collect();
        let mut out = Vec::with_capacity(self.nodes.len());
        while let Some((id, depth, fork_start)) = stack.pop() {
            let node = &mut self.nodes[id];
            let fork = node.children.len() > 1;
            let child_depth = depth + usize::from(fork);
            stack.extend(node.children.iter().rev().map(|&c| (c, child_depth, fork)));
            let mut row = std::mem::replace(&mut node.row, empty_row());
            row.depth = depth;
            row.fork_start = fork_start;
            out.push(row);
        }
        out
    }
}

fn empty_row() -> TreeRow {
    TreeRow {
        depth: 0,
        fork_start: false,
        label: String::new(),
        names: Vec::new(),
        on_active_path: false,
        active_leaf: false,
        target: TreeTarget { path: 0, end: 0 },
    }
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
