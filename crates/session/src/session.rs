use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use whycode_core::types::{
    ContentBlock, LlmRequest, Message, MessageContent, Role, SessionInfo, ToolDefinition,
};

/// Soft cap on tool result bodies kept in the transcript (Unicode scalars).
///
/// Large dumps (test output, full file reads) dominate context; the agent can
/// re-read files if it needs more. ~32 KiB ≈ 8k tokens under the char/4 heuristic.
pub const TOOL_RESULT_MAX_CHARS: usize = 32_768;

/// Cap for *older* tool results during compact (OpenCode-style prune ~2k chars).
/// Recent tool dumps stay at [`TOOL_RESULT_MAX_CHARS`] so the model can still
/// use the last step's output; older steps shrink hard for TTFT.
pub const TOOL_RESULT_PRUNE_CHARS: usize = 2_048;

/// How many recent tool-role messages keep the large cap when pruning.
const PRUNE_KEEP_RECENT_TOOLS: usize = 4;

/// Minimum number of tail messages retained by [`Session::compact`].
const MIN_KEEP_MESSAGES: usize = 4;

/// Reserved heuristic tokens for the summary line prepended by compact.
const SUMMARY_TOKEN_SLACK: usize = 64;

/// Outcome of [`Session::compact`] for autocompact circuit breakers.
#[derive(Debug, Clone, Copy)]
pub struct CompactOutcome {
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub messages_before: usize,
    pub messages_after: usize,
}

impl CompactOutcome {
    /// True when compact dropped tokens or messages.
    pub fn reduced(self) -> bool {
        self.tokens_after < self.tokens_before || self.messages_after < self.messages_before
    }

    /// Still above the autocompact threshold after the pass.
    pub fn still_over(self, threshold: usize) -> bool {
        threshold > 0 && self.tokens_after > threshold
    }

    /// Failed to relieve pressure: still over threshold and no reduction.
    pub fn failed(self, threshold: usize) -> bool {
        self.still_over(threshold) && !self.reduced()
    }
}

/// ~4 Unicode scalars per token (matches `whycode_llm` fallback family).
///
/// ASCII uses byte length (same as scalar count); non-ASCII still walks chars.
fn estimate_tokens(text: &str) -> usize {
    let n = if text.is_ascii() {
        text.len()
    } else {
        text.chars().count()
    };
    if n == 0 { 0 } else { n.div_ceil(4) }
}

fn message_tokens(msg: &Message) -> usize {
    match &msg.content {
        MessageContent::Text(t) => estimate_tokens(t),
        MessageContent::Blocks(b) => b
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => estimate_tokens(text),
                ContentBlock::ToolResult { content, .. } => estimate_tokens(content),
                ContentBlock::ToolUse { name, input, .. } => {
                    estimate_tokens(name) + estimate_tokens(&input.to_string())
                }
                ContentBlock::Image { .. } => 100,
            })
            .sum(),
    }
}

/// Running token estimate kept index-aligned with `Session::messages`.
///
/// Skipped on serde so export JSON stays stable; rebuilt after load or when
/// `valid` is false (deserialize default).
#[derive(Debug, Clone)]
struct SessionTokenCache {
    system: usize,
    per_msg: Vec<usize>,
    total: usize,
    valid: bool,
}

impl Default for SessionTokenCache {
    fn default() -> Self {
        Self {
            system: 0,
            per_msg: Vec::new(),
            total: 0,
            valid: false,
        }
    }
}

impl SessionTokenCache {
    fn from_parts(system_prompt: &str, messages: &[Message]) -> Self {
        let system = estimate_tokens(system_prompt);
        let per_msg: Vec<usize> = messages.iter().map(message_tokens).collect();
        let total = system + per_msg.iter().sum::<usize>();
        Self {
            system,
            per_msg,
            total,
            valid: true,
        }
    }

    fn ensure(&mut self, system_prompt: &str, messages: &[Message]) {
        if self.valid && self.per_msg.len() == messages.len() {
            return;
        }
        *self = Self::from_parts(system_prompt, messages);
    }

    fn push_msg(&mut self, msg: &Message) {
        if !self.valid {
            return;
        }
        let t = message_tokens(msg);
        self.per_msg.push(t);
        self.total = self.total.saturating_add(t);
    }

    fn set_system(&mut self, system_prompt: &str) {
        if !self.valid {
            return;
        }
        let system = estimate_tokens(system_prompt);
        self.total = self
            .total
            .saturating_sub(self.system)
            .saturating_add(system);
        self.system = system;
    }

    fn rebuild(&mut self, system_prompt: &str, messages: &[Message]) {
        *self = Self::from_parts(system_prompt, messages);
    }

    fn invalidate(&mut self) {
        self.valid = false;
        self.per_msg.clear();
        self.system = 0;
        self.total = 0;
    }
}

/// Parse `WHYCODE_IMAGE_B64:image/png\n<base64>` payloads from the read tool.
fn split_whycode_image_payload(content: &str) -> Option<(String, String, String)> {
    const MARK: &str = "WHYCODE_IMAGE_B64:";
    let idx = content.find(MARK)?;
    let after = &content[idx + MARK.len()..];
    let (media, rest) = after.split_once('\n')?;
    let media = media.trim();
    let b64 = rest.trim();
    if media.is_empty() || b64.is_empty() {
        return None;
    }
    let preface = content[..idx].trim().to_string();
    let preface = if preface.is_empty() {
        format!("[image {media}]")
    } else {
        preface
    };
    Some((media.to_string(), b64.to_string(), preface))
}

/// Cap a tool result string; no-op when already under the limit.
fn cap_tool_text(text: String) -> String {
    cap_tool_text_to(text, TOOL_RESULT_MAX_CHARS)
}

fn cap_tool_text_to(text: String, max_chars: usize) -> String {
    // Single walk: byte length is a cheap upper bound (UTF-8 ≥ 1 byte/scalar).
    // When under the limit in bytes we cannot exceed max_chars; skip counting.
    if text.len() <= max_chars {
        return text;
    }
    let total = text.chars().count();
    if total <= max_chars {
        return text;
    }
    let mut out: String = text.chars().take(max_chars).collect();
    let omitted = total - max_chars;
    out.push_str(&format!(
        "\n\n[... {omitted} characters truncated for context management]"
    ));
    out
}

fn summarize_trimmed(trimmed: &[Message]) -> String {
    let mut users = 0usize;
    let mut assistants = 0usize;
    let mut tools = 0usize;
    let mut other = 0usize;
    for m in trimmed {
        match m.role {
            Role::User => users += 1,
            Role::Assistant => assistants += 1,
            Role::Tool => tools += 1,
            Role::System => other += 1,
        }
    }
    format!(
        "[{n} earlier messages trimmed for context management \
         (user={users}, assistant={assistants}, tool={tools}{other_part})]",
        n = trimmed.len(),
        other_part = if other > 0 {
            format!(", other={other}")
        } else {
            String::new()
        }
    )
}

/// A conversation session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    /// How `title` was chosen. Gates auto-title so manual renames stick.
    #[serde(default)]
    pub title_source: crate::title::TitleSource,
    pub messages: Vec<Message>,
    pub system_prompt: String,
    pub project_path: PathBuf,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Token usage across every turn, as the provider reported it.
    ///
    /// `#[serde(default)]` so sessions exported before this existed still load.
    #[serde(default)]
    pub usage: whycode_core::types::Usage,
    /// Incremental char/4 estimate (system + per-message). Not persisted.
    #[serde(skip)]
    token_cache: SessionTokenCache,
}

impl Session {
    /// Create a new session
    pub fn new(project_path: PathBuf, system_prompt: String) -> Self {
        let now = chrono::Utc::now();
        let id = uuid::Uuid::new_v4().to_string();
        let title = crate::title::default_title(&project_path, &id);
        let token_cache = SessionTokenCache::from_parts(&system_prompt, &[]);
        Self {
            id,
            title,
            title_source: crate::title::TitleSource::Default,
            messages: Vec::new(),
            system_prompt,
            project_path,
            created_at: now,
            updated_at: now,
            usage: Default::default(),
            token_cache,
        }
    }

    /// User-facing rename; locks the title against further auto updates.
    pub fn set_title_manual(&mut self, title: impl Into<String>) {
        let t = crate::title::sanitize_title(&title.into());
        if t.is_empty() {
            return;
        }
        self.title = t;
        self.title_source = crate::title::TitleSource::Manual;
        self.touch();
    }

    /// Instant offline title from the first user prompt (if still default).
    ///
    /// Returns `true` when the title changed.
    pub fn apply_heuristic_title(&mut self, first_user_text: &str) -> bool {
        if !self.title_source.allows_heuristic() {
            return false;
        }
        let t = crate::title::heuristic_title(first_user_text);
        if t.is_empty() || t == self.title {
            return false;
        }
        self.title = t;
        self.title_source = crate::title::TitleSource::Heuristic;
        self.touch();
        true
    }

    /// Upgrade a placeholder title from the transcript's first user message.
    ///
    /// Used on resume and when listing sessions so legacy `New session - …`
    /// rows (and unfinished `project-ab` placeholders) pick up a real name
    /// without waiting for another turn.
    ///
    /// Returns `true` when the title changed.
    pub fn maybe_upgrade_title_from_history(&mut self) -> bool {
        let Some(text) = self.first_user_text() else {
            return false;
        };
        self.apply_heuristic_title(&text)
    }

    /// Apply a small-model title when still auto-titleable.
    ///
    /// Returns `true` when the title changed.
    pub fn apply_generated_title(&mut self, title: impl Into<String>) -> bool {
        if !self.title_source.allows_llm() {
            return false;
        }
        let t = crate::title::sanitize_title(&title.into());
        if t.is_empty() || t == self.title {
            // Mark generated even on no-op so we do not keep re-calling the model.
            if t == self.title && !t.is_empty() {
                self.title_source = crate::title::TitleSource::Generated;
            }
            return false;
        }
        self.title = t;
        self.title_source = crate::title::TitleSource::Generated;
        self.touch();
        true
    }

    /// Count of user-role messages (used to gate first-turn auto-title).
    pub fn user_message_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| m.role == Role::User)
            .count()
    }

    /// Text of the first user message, if any.
    pub fn first_user_text(&self) -> Option<String> {
        self.messages
            .iter()
            .find(|m| m.role == Role::User)
            .and_then(|m| m.content.as_text().map(|s| s.to_string()))
            .or_else(|| {
                // Blocks-only user messages: join text blocks.
                self.messages
                    .iter()
                    .find(|m| m.role == Role::User)
                    .map(|m| match &m.content {
                        MessageContent::Text(t) => t.clone(),
                        MessageContent::Blocks(blocks) => blocks
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    })
            })
            .filter(|s| !s.trim().is_empty())
    }

    /// Short assistant excerpt for title refine (first text block, capped).
    pub fn first_assistant_snippet(&self, max_chars: usize) -> Option<String> {
        for m in &self.messages {
            if m.role != Role::Assistant {
                continue;
            }
            let text = match &m.content {
                MessageContent::Text(t) => t.clone(),
                MessageContent::Blocks(blocks) => blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            let snippet: String = trimmed.chars().take(max_chars).collect();
            return Some(snippet);
        }
        None
    }

    /// Fold a turn''s reported usage into the session total.
    pub fn add_usage(&mut self, usage: &whycode_core::types::Usage) {
        self.usage.add(usage);
        self.touch();
    }

    /// Add a user message
    pub fn add_user_message(&mut self, content: &str) {
        let msg = Message {
            role: Role::User,
            content: MessageContent::Text(content.to_string()),
            tool_call_id: None,
            name: None,
        };
        self.token_cache.push_msg(&msg);
        self.messages.push(msg);
        self.touch();
    }

    /// Add a user message with structured content blocks (text + images, etc.).
    pub fn add_user_message_blocks(&mut self, blocks: Vec<ContentBlock>) {
        let msg = Message {
            role: Role::User,
            content: MessageContent::Blocks(blocks),
            tool_call_id: None,
            name: None,
        };
        self.token_cache.push_msg(&msg);
        self.messages.push(msg);
        self.touch();
    }

    /// Add an assistant message with content blocks
    pub fn add_assistant_message(&mut self, blocks: Vec<ContentBlock>) {
        let msg = Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(blocks),
            tool_call_id: None,
            name: None,
        };
        self.token_cache.push_msg(&msg);
        self.messages.push(msg);
        self.touch();
    }

    /// Add tool results (oversized bodies are capped for context economy).
    pub fn add_tool_results(&mut self, results: Vec<whycode_core::types::ToolResult>) {
        for result in results {
            let content =
                if let Some((media, b64, preface)) = split_whycode_image_payload(&result.content) {
                    // Vision path: short text + image block (A6).
                    MessageContent::Blocks(vec![
                        ContentBlock::Text {
                            text: cap_tool_text(preface),
                        },
                        ContentBlock::Image {
                            source: whycode_core::types::ImageSource::Base64 {
                                media_type: media,
                                data: b64,
                            },
                        },
                    ])
                } else {
                    MessageContent::Text(cap_tool_text(result.content))
                };
            let msg = Message {
                role: Role::Tool,
                content,
                tool_call_id: Some(result.tool_call_id),
                name: None,
            };
            self.token_cache.push_msg(&msg);
            self.messages.push(msg);
        }
        self.touch();
    }

    /// Build an LLM request from the current conversation
    pub fn build_request(
        &self,
        tools: &[ToolDefinition],
        max_tokens: Option<u32>,
        temperature: Option<f32>,
        _thinking: Option<bool>,
    ) -> LlmRequest {
        LlmRequest {
            system: self.system_prompt.clone(),
            messages: std::sync::Arc::from(self.messages.as_slice()),
            tools: tools.to_vec(),
            max_tokens,
            temperature,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: true,
        }
    }

    /// Get session info
    pub fn info(&self) -> SessionInfo {
        SessionInfo {
            id: self.id.clone(),
            title: self.title.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            message_count: self.messages.len(),
            project_path: self.project_path.clone(),
        }
    }

    /// Cline-style exit summary printed after the terminal is restored.
    ///
    /// `model_label` is typically `provider/model`. `binary` is the CLI name
    /// used in the resume hint (e.g. `"whycode"`).
    pub fn format_exit_summary(
        &self,
        duration: std::time::Duration,
        model_label: &str,
        binary: &str,
    ) -> String {
        // Labels padded to 8 so values line up (matches Cline's "Session Summary").
        let mut out = String::from("\nSession Summary\n");
        out.push_str(&format!("  {:8}  {}\n", "ID", self.id));
        out.push_str(&format!("  {:8}  {}s\n", "Duration", duration.as_secs()));
        out.push_str(&format!("  {:8}  {}\n", "Model", model_label));
        out.push_str(&format!("  {:8}  {}\n", "CWD", self.project_path.display()));
        out.push_str(&format!("  {:8}  {}\n", "Messages", self.messages.len()));
        out.push_str(&format!(
            "  {:8}  {} --resume {}\n",
            "Continue", binary, self.id
        ));
        out
    }

    /// Get conversation as a readable string (for display)
    pub fn conversation_text(&self) -> String {
        let mut out = String::new();
        for msg in &self.messages {
            let role = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::System => "System",
                Role::Tool => "Tool",
            };
            if let Some(text) = msg.content.as_text() {
                out.push_str(&format!("{}: {}\n", role, text));
            }
        }
        out
    }

    /// Update the system prompt
    pub fn set_system_prompt(&mut self, prompt: &str) {
        self.system_prompt = prompt.to_string();
        self.token_cache.set_system(&self.system_prompt);
        self.touch();
    }

    /// Estimate token count (Unicode chars / 4, same family as the LLM fallback).
    ///
    /// Not provider BPE — good enough for compaction thresholds and the context
    /// meter when usage has not been reported yet. O(1) when the running cache
    /// is valid; rebuilds after load or bulk mutation.
    pub fn token_count(&self) -> usize {
        if self.token_cache.valid && self.token_cache.per_msg.len() == self.messages.len() {
            return self.token_cache.total;
        }
        // `token_count` is `&self` on the hot path; rebuild without interior
        // mutability by recomputing. Callers that mutate should keep the cache
        // warm via push/rebuild helpers.
        let system = estimate_tokens(&self.system_prompt);
        system + self.messages.iter().map(message_tokens).sum::<usize>()
    }

    /// Like [`token_count`] but refreshes the running cache when stale.
    fn token_count_cached(&mut self) -> usize {
        self.token_cache
            .ensure(&self.system_prompt, &self.messages);
        self.token_cache.total
    }

    /// Cap oversized tool result bodies already in the transcript.
    ///
    /// Returns how many messages were modified.
    pub fn truncate_large_tool_results(&mut self) -> usize {
        let mut n = 0;
        for msg in &mut self.messages {
            if msg.role != Role::Tool {
                // Also shrink ToolResult blocks inside assistant content.
                if let MessageContent::Blocks(blocks) = &mut msg.content {
                    for block in blocks.iter_mut() {
                        if let ContentBlock::ToolResult { content, .. } = block {
                            let before = content.len();
                            *content = cap_tool_text(std::mem::take(content));
                            if content.len() != before {
                                n += 1;
                            }
                        }
                    }
                }
                continue;
            }
            match &mut msg.content {
                MessageContent::Text(t) => {
                    let before = t.len();
                    *t = cap_tool_text(std::mem::take(t));
                    if t.len() != before {
                        n += 1;
                    }
                }
                MessageContent::Blocks(blocks) => {
                    for block in blocks.iter_mut() {
                        if let ContentBlock::Text { text }
                        | ContentBlock::ToolResult { content: text, .. } = block
                        {
                            let before = text.len();
                            *text = cap_tool_text(std::mem::take(text));
                            if text.len() != before {
                                n += 1;
                            }
                        }
                    }
                }
            }
        }
        if n > 0 {
            self.token_cache.invalidate();
            self.touch();
        }
        n
    }

    /// Compact the conversation toward a token budget.
    ///
    /// 1. Cap oversized tool results in place.
    /// 2. If still over `¾ · max_tokens`, drop oldest messages until under
    ///    budget or only [`MIN_KEEP_MESSAGES`] remain in the tail.
    /// 3. Prepend a short stub summary of what was dropped.
    ///
    /// When already under budget after tool caps, the message list is not
    /// reshuffled (avoids churn on `/compact` for small sessions).
    /// Shrink older tool outputs more aggressively (OpenCode prune spirit).
    ///
    /// Keeps the last [`PRUNE_KEEP_RECENT_TOOLS`] tool-role messages at the
    /// normal cap; everything older is cut to [`TOOL_RESULT_PRUNE_CHARS`].
    pub fn prune_old_tool_results(&mut self) -> usize {
        let tool_indices: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == Role::Tool)
            .map(|(i, _)| i)
            .collect();
        if tool_indices.len() <= PRUNE_KEEP_RECENT_TOOLS {
            return 0;
        }
        let keep_from = tool_indices.len() - PRUNE_KEEP_RECENT_TOOLS;
        let prune_set: std::collections::HashSet<usize> =
            tool_indices[..keep_from].iter().copied().collect();
        let mut n = 0;
        for (i, msg) in self.messages.iter_mut().enumerate() {
            if !prune_set.contains(&i) {
                continue;
            }
            match &mut msg.content {
                MessageContent::Text(t) => {
                    let before = t.len();
                    *t = cap_tool_text_to(std::mem::take(t), TOOL_RESULT_PRUNE_CHARS);
                    if t.len() != before {
                        n += 1;
                    }
                }
                MessageContent::Blocks(blocks) => {
                    for block in blocks.iter_mut() {
                        if let ContentBlock::Text { text }
                        | ContentBlock::ToolResult { content: text, .. } = block
                        {
                            let before = text.len();
                            *text = cap_tool_text_to(std::mem::take(text), TOOL_RESULT_PRUNE_CHARS);
                            if text.len() != before {
                                n += 1;
                            }
                        }
                    }
                }
            }
        }
        if n > 0 {
            self.token_cache.invalidate();
            self.touch();
        }
        n
    }

    /// Insert or replace the leading compact summary user message (LLM path).
    pub fn prepend_compact_summary(&mut self, summary: &str) {
        let text = summary.trim();
        if text.is_empty() {
            return;
        }
        let body = if text.starts_with('[') {
            text.to_string()
        } else {
            format!("[Compacted earlier conversation]\n{text}")
        };
        if let Some(first) = self.messages.first_mut()
            && first.role == Role::User
            && let MessageContent::Text(t) = &first.content
            && t.starts_with("[Compacted")
        {
            first.content = MessageContent::Text(body);
            self.token_cache.invalidate();
            self.touch();
            return;
        }
        self.messages.insert(
            0,
            Message {
                role: Role::User,
                content: MessageContent::Text(body),
                tool_call_id: None,
                name: None,
            },
        );
        self.token_cache.invalidate();
        self.touch();
    }

    /// Concatenate older messages for LLM summarization (capped).
    pub fn transcript_for_compact_summary(&self, max_chars: usize) -> String {
        let mut out = String::new();
        let keep_tail = MIN_KEEP_MESSAGES.min(self.messages.len());
        let end = self.messages.len().saturating_sub(keep_tail);
        for m in &self.messages[..end] {
            let role = match m.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::Tool => "Tool",
                Role::System => "System",
            };
            let text = match &m.content {
                MessageContent::Text(t) => t.clone(),
                MessageContent::Blocks(b) => b
                    .iter()
                    .filter_map(|bl| match bl {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                        ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            let line = format!("{role}: {}\n", text.chars().take(400).collect::<String>());
            if out.len() + line.len() > max_chars {
                break;
            }
            out.push_str(&line);
        }
        out
    }

    pub fn compact(&mut self, max_tokens: usize) -> CompactOutcome {
        let tokens_before = self.token_count_cached();
        let messages_before = self.messages.len();

        self.truncate_large_tool_results();
        self.prune_old_tool_results();

        let target = max_tokens.saturating_mul(3) / 4;
        let target = target.max(1);

        if self.token_count_cached() <= target {
            self.touch();
            return CompactOutcome {
                tokens_before,
                tokens_after: self.token_count_cached(),
                messages_before,
                messages_after: self.messages.len(),
            };
        }

        let min_keep = MIN_KEEP_MESSAGES.min(self.messages.len());
        // Find the smallest `start` such that messages[start..] fit the budget
        // (plus a small allowance for the summary line), never dropping below
        // `min_keep` tail messages.
        let mut start = 0usize;
        while self.messages.len() - start > min_keep {
            let tail = &self.messages[start..];
            let tail_tokens: usize = tail.iter().map(message_tokens).sum();
            let total = estimate_tokens(&self.system_prompt) + tail_tokens + SUMMARY_TOKEN_SLACK;
            if total <= target {
                break;
            }
            start += 1;
        }

        if start == 0 {
            // Tail alone still over budget (or nothing to drop). Leave as-is;
            // tool caps already ran.
            self.touch();
            return CompactOutcome {
                tokens_before,
                tokens_after: self.token_count_cached(),
                messages_before,
                messages_after: self.messages.len(),
            };
        }

        let trimmed = &self.messages[..start];
        let summary = summarize_trimmed(trimmed);
        let mut new_messages = Vec::with_capacity(1 + self.messages.len() - start);
        new_messages.push(Message {
            role: Role::User,
            content: MessageContent::Text(summary),
            tool_call_id: None,
            name: None,
        });
        new_messages.extend(self.messages[start..].iter().cloned());
        self.messages = new_messages;
        self.token_cache
            .rebuild(&self.system_prompt, &self.messages);
        self.touch();
        CompactOutcome {
            tokens_before,
            tokens_after: self.token_count_cached(),
            messages_before,
            messages_after: self.messages.len(),
        }
    }

    /// Persist this session and all messages to the SQLite database.
    ///
    /// Session metadata (including provider-reported token usage) is upserted.
    /// Messages are replaced as a set so repeated saves do not duplicate rows.
    pub fn save_to_db(&self, db: &whycode_storage::db::Database) -> anyhow::Result<()> {
        db.upsert_session(
            &self.id,
            &self.title,
            &self.project_path.to_string_lossy(),
            &self.created_at.to_rfc3339(),
            &self.updated_at.to_rfc3339(),
            &self.usage,
        )?;

        // Full replace in one transaction (honest counts; no half-written rows).
        let mut rows = Vec::with_capacity(self.messages.len());
        for msg in &self.messages {
            let msg_json = serde_json::to_string(msg)?;
            let role_str = serde_json::to_string(&msg.role)?
                .trim_matches('"')
                .to_string();
            let msg_id = uuid::Uuid::new_v4().to_string();
            rows.push((
                msg_id,
                role_str,
                msg_json,
                msg.tool_call_id.clone(),
                msg.name.clone(),
            ));
        }
        db.replace_messages(&self.id, &rows)?;

        Ok(())
    }

    /// Load a session and its messages from the SQLite database by session id.
    pub fn load_from_db(
        db: &whycode_storage::db::Database,
        id: &str,
    ) -> anyhow::Result<Option<Self>> {
        let Some(row) = db.get_session(id)? else {
            return Ok(None);
        };

        let created_at =
            chrono::DateTime::parse_from_rfc3339(&row.created_at)?.with_timezone(&chrono::Utc);
        let updated_at =
            chrono::DateTime::parse_from_rfc3339(&row.updated_at)?.with_timezone(&chrono::Utc);

        let message_rows = db.get_messages(id)?;
        let messages: Vec<whycode_core::types::Message> = message_rows
            .iter()
            .map(|mr| serde_json::from_str(&mr.content))
            .collect::<Result<_, _>>()?;

        let project_path = std::path::PathBuf::from(row.project_path);
        let title_source = crate::title::infer_source_from_title(&row.title, &project_path);
        let token_cache = SessionTokenCache::from_parts("", &messages);
        Ok(Some(Self {
            id: row.id,
            title: row.title,
            title_source,
            messages,
            system_prompt: String::new(), // system_prompt is not yet persisted
            project_path,
            created_at,
            updated_at,
            usage: row.usage,
            token_cache,
        }))
    }
}

#[cfg(test)]
mod persist_tests {
    use super::*;
    use whycode_core::types::Usage;

    #[test]
    fn save_and_load_restores_usage_and_messages() {
        let db = whycode_storage::db::Database::open_in_memory().unwrap();
        let mut session = Session::new(std::path::PathBuf::from("/proj"), "sys".into());
        session.add_user_message("hello");
        session.add_usage(&Usage {
            input_tokens: 42,
            output_tokens: 7,
            cache_creation_input_tokens: Some(1),
            cache_read_input_tokens: Some(2),
        });
        session.save_to_db(&db).unwrap();

        // Second save must not duplicate messages.
        session.add_assistant_message(vec![ContentBlock::Text { text: "hi".into() }]);
        session.save_to_db(&db).unwrap();

        let loaded = Session::load_from_db(&db, &session.id)
            .unwrap()
            .expect("session row");
        assert_eq!(loaded.usage.input_tokens, 42);
        assert_eq!(loaded.usage.output_tokens, 7);
        assert_eq!(loaded.usage.cache_creation_input_tokens, Some(1));
        assert_eq!(loaded.usage.cache_read_input_tokens, Some(2));
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(db.message_count(&session.id).unwrap(), 2);
    }

    #[test]
    fn usage_totals_reflect_saved_sessions() {
        let db = whycode_storage::db::Database::open_in_memory().unwrap();
        let mut a = Session::new(std::path::PathBuf::from("/a"), "s".into());
        a.add_usage(&Usage {
            input_tokens: 10,
            output_tokens: 1,
            ..Default::default()
        });
        a.save_to_db(&db).unwrap();
        let mut b = Session::new(std::path::PathBuf::from("/b"), "s".into());
        b.add_usage(&Usage {
            input_tokens: 20,
            output_tokens: 2,
            ..Default::default()
        });
        b.save_to_db(&db).unwrap();

        let totals = db.usage_totals().unwrap();
        assert_eq!(totals.session_count, 2);
        assert_eq!(totals.usage.input_tokens, 30);
        assert_eq!(totals.usage.output_tokens, 3);
    }
}

// Keep export methods on Session.
impl Session {
    /// Export the session as a shareable JSON file.
    /// Writes to .whycode/shares/{session_id}.json and returns the file path.
    pub fn export_share(&self) -> anyhow::Result<String> {
        let shares_dir = self.project_path.join(".whycode").join("shares");
        std::fs::create_dir_all(&shares_dir)?;

        let filename = format!("{}.json", self.id);
        let share_path = shares_dir.join(&filename);

        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&share_path, json)?;

        // Also write a human-readable Markdown share (OpenCode /export style)
        let md_path = shares_dir.join(format!("{}.md", self.id));
        let _ = std::fs::write(&md_path, self.export_markdown());

        Ok(share_path.to_string_lossy().to_string())
    }

    /// Export conversation as Markdown for sharing / `/export`.
    pub fn export_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# {}\n\n", self.title));
        out.push_str(&format!("- **Session:** `{}`\n", self.id));
        out.push_str(&format!(
            "- **Project:** `{}`\n",
            self.project_path.display()
        ));
        out.push_str(&format!(
            "- **Created:** {}\n\n---\n\n",
            self.created_at.to_rfc3339()
        ));

        for msg in &self.messages {
            let role = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::System => "System",
                Role::Tool => "Tool",
            };
            out.push_str(&format!("### {role}\n\n"));
            match &msg.content {
                MessageContent::Text(t) => {
                    out.push_str(t);
                    out.push_str("\n\n");
                }
                MessageContent::Blocks(blocks) => {
                    for b in blocks {
                        match b {
                            ContentBlock::Text { text } => {
                                out.push_str(text);
                                out.push_str("\n\n");
                            }
                            ContentBlock::ToolUse { name, input, .. } => {
                                out.push_str(&format!(
                                    "```tool\n{name}\n{}\n```\n\n",
                                    serde_json::to_string_pretty(input).unwrap_or_default()
                                ));
                            }
                            ContentBlock::ToolResult {
                                content, is_error, ..
                            } => {
                                let tag = if is_error.unwrap_or(false) {
                                    "error"
                                } else {
                                    "result"
                                };
                                out.push_str(&format!("```{tag}\n{content}\n```\n\n"));
                            }
                            ContentBlock::Image { .. } => {
                                out.push_str("*[image]*\n\n");
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// Revert the session to a previous state by removing all messages after
    /// the given index. Returns the number of messages removed.
    pub fn revert_to(&mut self, message_index: usize) -> usize {
        if message_index >= self.messages.len() {
            return 0;
        }

        let removed = self.messages.len() - message_index - 1;
        self.messages.truncate(message_index + 1);
        self.token_cache
            .rebuild(&self.system_prompt, &self.messages);
        self.touch();
        removed
    }

    /// Replace the entire message list (used by undo/redo).
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.token_cache
            .rebuild(&self.system_prompt, &self.messages);
        self.touch();
    }

    /// Undo the last user turn: remove from the last user message to the end.
    /// Returns the number of messages removed, or 0 if nothing to undo.
    pub fn undo_last_turn(&mut self) -> usize {
        let last_user = self.messages.iter().rposition(|m| m.role == Role::User);
        match last_user {
            Some(idx) => {
                let removed = self.messages.len() - idx;
                self.messages.truncate(idx);
                self.token_cache
                    .rebuild(&self.system_prompt, &self.messages);
                self.touch();
                removed
            }
            None => 0,
        }
    }

    fn touch(&mut self) {
        self.updated_at = chrono::Utc::now();
    }
}

/// Default system prompt for the main agent
pub fn default_system_prompt() -> String {
    include_str!("../../agent/prompt.txt").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_project_path() -> PathBuf {
        PathBuf::from("/tmp/test-project")
    }

    fn test_system_prompt() -> String {
        "You are a helpful assistant.".to_string()
    }

    #[test]
    fn test_format_exit_summary() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("hello");
        let text = session.format_exit_summary(
            std::time::Duration::from_secs(42),
            "openai/gpt-4o",
            "whycode",
        );
        assert!(text.contains("Session Summary"), "{text}");
        assert!(text.contains(&session.id), "{text}");
        assert!(text.contains("Duration  42s"), "{text}");
        assert!(text.contains("Model     openai/gpt-4o"), "{text}");
        assert!(text.contains("CWD       /tmp/test-project"), "{text}");
        assert!(text.contains("Messages  1"), "{text}");
        assert!(
            text.contains(&format!("whycode --resume {}", session.id)),
            "{text}"
        );
    }

    #[test]
    fn test_new_session() {
        let session = Session::new(test_project_path(), test_system_prompt());

        assert!(!session.id.is_empty(), "session id should not be empty");
        assert!(
            session.title.starts_with("test-project-"),
            "title should be project basename + short id, got {:?}",
            session.title
        );
        assert_eq!(session.title_source, crate::title::TitleSource::Default);
        assert!(
            session.messages.is_empty(),
            "new session should have no messages"
        );
        assert_eq!(session.system_prompt, test_system_prompt());
        assert_eq!(session.project_path, test_project_path());
        assert_eq!(session.created_at, session.updated_at);
    }

    #[test]
    fn test_heuristic_and_manual_title() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        assert!(session.apply_heuristic_title("Please fix the auth middleware bug"));
        assert_eq!(session.title_source, crate::title::TitleSource::Heuristic);
        assert!(session.title.to_ascii_lowercase().contains("auth"));

        session.set_title_manual("My locked name");
        assert_eq!(session.title, "My locked name");
        assert_eq!(session.title_source, crate::title::TitleSource::Manual);
        assert!(!session.apply_heuristic_title("something else"));
        assert!(!session.apply_generated_title("model title"));
        assert_eq!(session.title, "My locked name");
    }

    #[test]
    fn upgrade_from_history_uses_first_user_message() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        // Simulate a legacy placeholder still marked Default.
        session.title = "New session - 2026-01-01".into();
        session.title_source = crate::title::TitleSource::Default;
        session.add_user_message("Please fix the stripe webhook timeout");
        session.add_assistant_message(vec![ContentBlock::Text {
            text: "Looking into it.".into(),
        }]);
        session.add_user_message("also check retries");

        assert!(session.maybe_upgrade_title_from_history());
        assert_eq!(session.title_source, crate::title::TitleSource::Heuristic);
        assert!(session.title.to_ascii_lowercase().contains("stripe"));
        // Second call is a no-op (no longer Default).
        assert!(!session.maybe_upgrade_title_from_history());
    }

    #[test]
    fn test_add_messages() {
        let mut session = Session::new(test_project_path(), test_system_prompt());

        session.add_user_message("Hello");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].role, Role::User);
        assert_eq!(session.messages[0].content.as_text(), Some("Hello"));

        session.add_assistant_message(vec![ContentBlock::Text {
            text: "Hi there!".to_string(),
        }]);
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[1].role, Role::Assistant);
        assert_eq!(session.messages[1].content.as_text(), Some("Hi there!"));

        // updated_at should change after adding messages
        assert!(session.updated_at >= session.created_at);
    }

    #[test]
    fn test_add_tool_results() {
        let mut session = Session::new(test_project_path(), test_system_prompt());

        let results = vec![
            whycode_core::types::ToolResult {
                tool_call_id: "call-1".to_string(),
                content: "result 1".to_string(),
                is_error: false,
            },
            whycode_core::types::ToolResult {
                tool_call_id: "call-2".to_string(),
                content: "error result".to_string(),
                is_error: true,
            },
        ];

        session.add_user_message("use tools");
        session.add_tool_results(results);

        assert_eq!(session.messages.len(), 3);
        assert_eq!(session.messages[1].role, Role::Tool);
        assert_eq!(session.messages[1].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(session.messages[1].content.as_text(), Some("result 1"));
        assert_eq!(session.messages[2].role, Role::Tool);
        assert_eq!(session.messages[2].tool_call_id.as_deref(), Some("call-2"));
    }

    #[test]
    fn test_build_request() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("test message");

        let tools = vec![ToolDefinition {
            name: "search".to_string(),
            description: "search tool".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }];

        let req = session.build_request(&tools, Some(1024), Some(0.7), None);

        assert_eq!(req.system, test_system_prompt());
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.tools[0].name, "search");
        assert_eq!(req.max_tokens, Some(1024));
        assert_eq!(req.temperature, Some(0.7));
        assert!(req.top_p.is_none());
        assert!(req.top_k.is_none());
    }

    #[test]
    fn test_build_request_no_tools() {
        let session = Session::new(test_project_path(), test_system_prompt());
        let req = session.build_request(&[], None, None, None);

        assert!(req.tools.is_empty());
        assert_eq!(req.system, test_system_prompt());
        assert!(req.messages.is_empty());
    }

    #[test]
    fn test_token_count() {
        let mut session = Session::new(test_project_path(), "short prompt".to_string());

        // Empty session should just have system prompt tokens
        let base_tokens = session.token_count();
        assert_eq!(base_tokens, estimate_tokens("short prompt"));

        // Add a text message
        session.add_user_message("hello world, this is a test message");
        let with_msg = session.token_count();
        assert!(with_msg > base_tokens);

        // Add assistant with blocks
        session.add_assistant_message(vec![
            ContentBlock::Text {
                text: "response text here".to_string(),
            },
            ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "tool".to_string(),
                input: serde_json::json!({}),
            },
        ]);
        let with_blocks = session.token_count();
        assert!(with_blocks > with_msg);
    }

    #[test]
    fn test_compact_under_budget_is_noop() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        for i in 0..10 {
            session.add_user_message(&format!("message {i}"));
        }
        let before = session.messages.len();
        session.compact(1_000_000);
        assert_eq!(session.messages.len(), before);
        assert_eq!(session.messages[0].content.as_text(), Some("message 0"));
    }

    #[test]
    fn test_compact_drops_to_token_budget() {
        let mut session = Session::new(test_project_path(), test_system_prompt());

        for i in 0..10 {
            session.add_user_message(&format!("message {i} {}", "x".repeat(200)));
        }
        assert_eq!(session.messages.len(), 10);

        session.compact(200);

        let summary_text = session.messages[0].content.as_text().unwrap();
        assert!(
            summary_text.contains("earlier messages trimmed"),
            "summary should mention trimmed messages: {summary_text}"
        );
        assert!(
            session.messages.len() >= 5,
            "summary + min keep, got {}",
            session.messages.len()
        );
        assert!(
            session.messages.len() < 11,
            "should have dropped something, got {}",
            session.messages.len()
        );
        let last = session.messages.last().unwrap().content.as_text().unwrap();
        assert!(last.starts_with("message 9"), "last was {last}");
    }

    #[test]
    fn test_compact_few_messages() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("only message");

        let before = session.messages.len();
        session.compact(1000);

        assert_eq!(session.messages.len(), before);
        assert_eq!(session.messages[0].content.as_text(), Some("only message"));
    }

    #[test]
    fn test_add_tool_results_caps_huge_body() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        let huge = "a".repeat(TOOL_RESULT_MAX_CHARS + 5000);
        session.add_tool_results(vec![whycode_core::types::ToolResult {
            tool_call_id: "t1".into(),
            content: huge,
            is_error: false,
        }]);
        let text = session.messages[0].content.as_text().unwrap();
        assert!(text.contains("characters truncated for context management"));
        assert!(text.chars().count() < TOOL_RESULT_MAX_CHARS + 200);
    }

    #[test]
    fn test_info() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("hello");
        session.add_assistant_message(vec![ContentBlock::Text {
            text: "hi".to_string(),
        }]);

        let info = session.info();
        assert_eq!(info.id, session.id);
        assert_eq!(info.title, session.title);
        assert_eq!(info.message_count, 2);
        assert_eq!(info.project_path, test_project_path());
        assert_eq!(info.created_at, session.created_at);
        assert_eq!(info.updated_at, session.updated_at);
    }

    #[test]
    fn test_set_system_prompt() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        let old_updated = session.updated_at;

        // Small delay so we can assert the timestamp changes
        std::thread::sleep(std::time::Duration::from_millis(1));

        session.set_system_prompt("new prompt");

        assert_eq!(session.system_prompt, "new prompt");
        assert!(session.updated_at > old_updated);
    }

    #[test]
    fn test_revert_to() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        for i in 0..5 {
            session.add_user_message(&format!("msg {}", i));
        }

        assert_eq!(session.messages.len(), 5);

        // Keep messages up to and including index 2 (first 3 messages)
        let removed = session.revert_to(2);
        assert_eq!(removed, 2); // removed messages at indices 3 and 4
        assert_eq!(session.messages.len(), 3);
        assert_eq!(session.messages[0].content.as_text(), Some("msg 0"));
        assert_eq!(session.messages[1].content.as_text(), Some("msg 1"));
        assert_eq!(session.messages[2].content.as_text(), Some("msg 2"));
    }

    #[test]
    fn test_revert_to_out_of_bounds() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("hello");

        let removed = session.revert_to(5); // beyond length
        assert_eq!(removed, 0);
        assert_eq!(session.messages.len(), 1);
    }

    #[test]
    fn test_conversation_text() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("hello");
        session.add_assistant_message(vec![ContentBlock::Text {
            text: "hi there".to_string(),
        }]);

        let text = session.conversation_text();
        assert!(text.contains("User: hello"));
        assert!(text.contains("Assistant: hi there"));
    }
}
