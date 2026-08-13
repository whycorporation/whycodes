//! Process-local exact + semantic cache for **text-only** LLM replies.
//!
//! Tool-using agent turns are never stored: they depend on live workspace
//! state. Title, compact, retain, and tools-free chat can replay.
//!
//! Semantic match uses the same hashed n-gram embed as `whycode-memory`
//! (no ONNX). Same system + tool-name set is required so a similar question
//! in a different project cannot leak.

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rustc_hash::FxHasher;
use whycode_core::types::{ContentBlock, LlmRequest, LlmResponse, MessageContent, Role, Usage};

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

    pub fn clear(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.clear();
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
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

        let mut guard = self.inner.lock().ok()?;
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
            if s >= SEMANTIC_THRESHOLD && best.is_none_or(|(_, b)| s > b) {
                best = Some((i, s));
            }
        }
        let (i, score) = best?;
        let e = guard.remove(i)?;
        let text = e.text.clone();
        guard.push_back(e);
        tracing::debug!(score, "response_cache.semantic_hit");
        Some(CachedText { text })
    }

    pub fn store(&self, request: &LlmRequest, model: &str, text: &str) {
        if !Self::eligible(request) {
            return;
        }
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let exact = exact_key(request, model);
        let now = Instant::now();
        let Ok(mut guard) = self.inner.lock() else {
            tracing::warn!("response_cache.store: lock poisoned");
            return;
        };
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
    for t in &request.tools {
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
mod tests {
    use super::*;
    use whycode_core::types::{Message, ToolDefinition};

    fn req(system: &str, user: &str) -> LlmRequest {
        LlmRequest {
            system: system.into(),
            messages: std::sync::Arc::from(vec![Message {
                role: Role::User,
                content: MessageContent::Text(user.into()),
                tool_call_id: None,
                name: None,
            }]),
            tools: vec![],
            max_tokens: Some(64),
            temperature: Some(0.2),
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: false,
        }
    }

    #[test]
    fn exact_hit_replays() {
        let cache = ResponseCache::new();
        let r = req("sys", "what is the default port");
        assert!(cache.lookup(&r, "haiku").is_none());
        cache.store(&r, "haiku", "8080");
        let hit = cache.lookup(&r, "haiku").expect("exact");
        assert_eq!(hit.text, "8080");
    }

    #[test]
    fn different_model_misses() {
        let cache = ResponseCache::new();
        let r = req("sys", "default port");
        cache.store(&r, "haiku", "8080");
        assert!(cache.lookup(&r, "sonnet").is_none());
    }

    #[test]
    fn semantic_paraphrase_hits_same_system() {
        let cache = ResponseCache::new();
        let a = req("sys", "what is the default port");
        cache.store(&a, "haiku", "8080");
        let b = req("sys", "what's the default port");
        let hit = cache.lookup(&b, "haiku").expect("semantic");
        assert_eq!(hit.text, "8080");
    }

    #[test]
    fn different_system_does_not_semantic_hit() {
        let cache = ResponseCache::new();
        cache.store(
            &req("project-a", "what is the default port"),
            "haiku",
            "8080",
        );
        assert!(
            cache
                .lookup(&req("project-b", "what is the default port"), "haiku")
                .is_none()
        );
    }

    #[test]
    fn tools_in_request_never_cache() {
        let cache = ResponseCache::new();
        let mut r = req("sys", "read src/main.rs");
        r.tools.push(ToolDefinition {
            name: "read".into(),
            description: "read a file".into(),
            parameters: serde_json::json!({"type": "object"}),
        });
        cache.store(&r, "haiku", "fn main() {}");
        assert!(cache.lookup(&r, "haiku").is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn text_only_skips_tool_use_responses() {
        let resp = LlmResponse {
            content: vec![
                ContentBlock::Text {
                    text: "calling".into(),
                },
                ContentBlock::ToolUse {
                    id: "1".into(),
                    name: "read".into(),
                    input: serde_json::json!({}),
                },
            ],
            stop_reason: None,
            usage: Usage::default(),
            model: "x".into(),
        };
        assert!(text_only_response(&resp).is_none());
    }
}
