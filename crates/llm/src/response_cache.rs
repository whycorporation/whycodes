//! Process-local exact + semantic cache for **text-only** LLM replies.
//!
//! Tool-using agent turns are never stored: they depend on live workspace
//! state. Title, compact, retain, and tools-free chat can replay.
//!
//! Semantic match uses the same hashed n-gram embed as `whycodes-memory`
//! (no ONNX). Same system + tool-name set is required so a similar question
//! in a different project cannot leak.

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rustc_hash::FxHasher;
use whycodes_core::types::{ContentBlock, LlmRequest, LlmResponse, MessageContent, Role, Usage};

const DIM: usize = 64;
const TTL: Duration = Duration::from_secs(600);
const MAX_ENTRIES: usize = 48;
/// High bar: near-paraphrase only ("what's the port" ≈ "what is the port").
const SEMANTIC_THRESHOLD: f32 = 0.88;

/// Cached assistant text (no tool calls).
#[derive(Debug, Clone)]
pub struct CachedText {
    pub text: String,
}

struct Entry {
    exact: u64,
    embed: Vec<f32>,
    tool_sig: u64,
    system_sig: u64,
    model_sig: u64,
    text: String,
    at: Instant,
}

/// In-process LRU of text-only completions.
pub struct ResponseCache {
    inner: Mutex<VecDeque<Entry>>,
}

impl ResponseCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(VecDeque::new()),
        }
    }

    /// Process-wide cache shared by title / compact / agent / idle suggest.
    pub fn global() -> &'static Self {
        static CACHE: OnceLock<ResponseCache> = OnceLock::new();
        CACHE.get_or_init(Self::new)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<Entry>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn clear(&self) {
        self.lock().clear();
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Only tools-free requests are eligible (agent tool turns stay live).
    pub fn eligible(request: &LlmRequest) -> bool {
        request.tools.is_empty()
    }

    pub fn lookup(&self, request: &LlmRequest, model: &str) -> Option<CachedText> {
        if !Self::eligible(request) {
            return None;
        }
        let exact = exact_key(request, model);
        let tool_sig = tool_sig(request);
        let system_sig = fnv1a_64(request.system.as_bytes());
        let model_sig = fnv1a_64(model.as_bytes());
        let query = last_user_text(request);
        let embed = embed(query, DIM);
        let now = Instant::now();

        let mut guard = self.lock();
        evict_expired(&mut guard, now);

        if let Some(pos) = guard.iter().position(|e| e.exact == exact)
            && let Some(e) = guard.remove(pos)
        {
            let text = e.text.clone();
            guard.push_back(e);
            return Some(CachedText { text });
        }

        let mut best: Option<(usize, f32)> = None;
        for (i, e) in guard.iter().enumerate() {
            if e.tool_sig != tool_sig || e.system_sig != system_sig || e.model_sig != model_sig {
                continue;
            }
            let s = cosine(&embed, &e.embed);
            if s >= SEMANTIC_THRESHOLD {
                best = better_semantic(best, i, s);
            }
        }
        take_semantic_hit(&mut guard, best)
    }

    #[cfg(test)]
    pub(crate) fn better_semantic_for_tests(
        best: Option<(usize, f32)>,
        i: usize,
        score: f32,
    ) -> Option<(usize, f32)> {
        better_semantic(best, i, score)
    }

    #[cfg(test)]
    pub(crate) fn take_semantic_hit_none_for_tests() -> Option<CachedText> {
        let mut empty = VecDeque::new();
        take_semantic_hit(&mut empty, Some((0, 1.0)))
    }

    pub fn store(&self, request: &LlmRequest, model: &str, text: &str) {
        self.store_at(request, model, text, Instant::now());
    }

    fn store_at(&self, request: &LlmRequest, model: &str, text: &str, now: Instant) {
        if !Self::eligible(request) {
            return;
        }
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let exact = exact_key(request, model);
        let mut guard = self.lock();
        evict_expired(&mut guard, now);
        if guard.iter().any(|e| e.exact == exact) {
            return;
        }
        guard.push_back(Entry {
            exact,
            embed: embed(last_user_text(request), DIM),
            tool_sig: tool_sig(request),
            system_sig: fnv1a_64(request.system.as_bytes()),
            model_sig: fnv1a_64(model.as_bytes()),
            text: text.to_string(),
            at: now,
        });
        while guard.len() > MAX_ENTRIES {
            guard.pop_front();
        }
    }

    /// Build a synthetic completion from a cache hit.
    pub fn to_response(hit: &CachedText, model: &str) -> LlmResponse {
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: hit.text.clone(),
            }],
            stop_reason: Some("cache".into()),
            usage: Usage::default(),
            model: model.to_string(),
        }
    }
}

impl Default for ResponseCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract assistant text only when the response has no tool-use blocks.
pub fn text_only_response(resp: &LlmResponse) -> Option<String> {
    if resp
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
    {
        return None;
    }
    let text = resp
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn better_semantic(best: Option<(usize, f32)>, i: usize, score: f32) -> Option<(usize, f32)> {
    match best {
        None => Some((i, score)),
        Some((_, b)) if score > b => Some((i, score)),
        other => other,
    }
}

fn take_semantic_hit(
    guard: &mut VecDeque<Entry>,
    best: Option<(usize, f32)>,
) -> Option<CachedText> {
    let (i, score) = best?;
    let e = guard.remove(i)?;
    let text = e.text.clone();
    guard.push_back(e);
    tracing::debug!("response_cache.semantic_hit score={score}");
    Some(CachedText { text })
}

fn evict_expired(entries: &mut VecDeque<Entry>, now: Instant) {
    while let Some(front) = entries.front() {
        if now.saturating_duration_since(front.at) > TTL {
            entries.pop_front();
        } else {
            break;
        }
    }
}

fn last_user_text(request: &LlmRequest) -> &str {
    request
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .and_then(|m| m.content.as_text())
        .unwrap_or("")
}

fn tool_sig(request: &LlmRequest) -> u64 {
    let mut names: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
    names.sort_unstable();
    let mut h = FxHasher::default();
    for n in names {
        n.hash(&mut h);
    }
    h.finish()
}

fn exact_key(request: &LlmRequest, model: &str) -> u64 {
    let mut h = FxHasher::default();
    model.hash(&mut h);
    request.system.hash(&mut h);
    for msg in request.messages.iter() {
        match msg.role {
            Role::System => 0u8.hash(&mut h),
            Role::User => 1u8.hash(&mut h),
            Role::Assistant => 2u8.hash(&mut h),
            Role::Tool => 3u8.hash(&mut h),
        }
        hash_content(&mut h, &msg.content);
        if let Some(id) = &msg.tool_call_id {
            id.hash(&mut h);
        }
    }
    for t in request.tools.iter() {
        t.name.hash(&mut h);
    }
    h.finish()
}

fn hash_content(h: &mut FxHasher, content: &MessageContent) {
    match content {
        MessageContent::Text(s) => s.hash(h),
        MessageContent::Blocks(blocks) => {
            for b in blocks {
                match b {
                    ContentBlock::Text { text } => text.hash(h),
                    ContentBlock::ToolUse { id, name, input } => {
                        id.hash(h);
                        name.hash(h);
                        input.to_string().hash(h);
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        tool_use_id.hash(h);
                        content.hash(h);
                        is_error.hash(h);
                    }
                    ContentBlock::Image { .. } => {
                        0x49u8.hash(h); // 'I' — images differ by presence only
                    }
                    ContentBlock::Thinking { text, signature } => {
                        0x54u8.hash(h);
                        text.hash(h);
                        signature.hash(h);
                    }
                    ContentBlock::RedactedThinking { data } => {
                        0x52u8.hash(h);
                        data.hash(h);
                    }
                }
            }
        }
    }
}

fn embed(text: &str, dim: usize) -> Vec<f32> {
    let dim = dim.max(8);
    let mut v = vec![0.0f32; dim];
    let lower = text.to_lowercase();
    for tok in lower.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-') {
        if tok.len() < 2 {
            continue;
        }
        accumulate(&mut v, tok, 1.0);
        if tok.chars().count() > 4 {
            let prefix: String = tok.chars().take(4).collect();
            accumulate(&mut v, &prefix, 0.35);
        }
    }
    let chars: Vec<char> = lower.chars().filter(|c| !c.is_control()).collect();
    if chars.len() >= 3 {
        for w in chars.windows(3) {
            let tri: String = w.iter().collect();
            if tri.chars().all(|c| c.is_whitespace()) {
                continue;
            }
            accumulate(&mut v, &tri, 0.5);
        }
    }
    l2_normalize(&mut v);
    v
}

fn accumulate(v: &mut [f32], feature: &str, weight: f32) {
    let h = fnv1a_64(feature.as_bytes());
    let idx = (h as usize) % v.len();
    let sign = if h & 1 == 0 { 1.0 } else { -1.0 };
    v[idx] += weight * sign;
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn l2_normalize(v: &mut [f32]) {
    let mut sum = 0.0f32;
    for x in v.iter() {
        sum += *x * *x;
    }
    if sum <= f32::EPSILON {
        return;
    }
    let inv = sum.sqrt().recip();
    for x in v.iter_mut() {
        *x *= inv;
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
#[path = "response_cache_tests.rs"]
mod tests;
