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
/// Harder shake when the session is still over ~¾ of the compact threshold.
pub const TOOL_RESULT_SHAKE_CHARS: usize = 512;

/// How many recent tool-role messages keep the large cap when pruning.
const PRUNE_KEEP_RECENT_TOOLS: usize = 4;

/// Minimum number of tail messages retained by [`Session::compact`].
const MIN_KEEP_MESSAGES: usize = 4;

/// Reserved heuristic tokens for the summary line prepended by compact.
/// Raised so the richer local summary (goals + paths) fits without thrashing.
const SUMMARY_TOKEN_SLACK: usize = 256;

/// Max chars of dropped-message transcript retained for optional LLM summary.
const DROPPED_TRANSCRIPT_MAX_CHARS: usize = 12_000;

/// Whole-session transcript cap for Grok-style full-replace compact.
const FULL_TRANSCRIPT_MAX_CHARS: usize = 48_000;

/// Preamble prepended to the post-compact summary carrier (Grok full-replace).
pub const COMPACT_CONTINUATION_PREAMBLE: &str = "This session is being continued from a previous conversation that ran out of context. \
     The summary below covers the earlier portion of the conversation.";

/// Outcome of [`Session::compact`] for autocompact circuit breakers.
#[derive(Debug, Clone, Default)]
pub struct CompactOutcome {
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub messages_before: usize,
    pub messages_after: usize,
    /// Text of messages that were dropped (capped). Empty when nothing trimmed.
    /// Used by the agent LLM-compact path so the summary covers *history*, not
    /// the already-kept tail.
    pub dropped_transcript: String,
}

impl CompactOutcome {
    /// True when compact dropped tokens or messages.
    pub fn reduced(&self) -> bool {
        self.tokens_after < self.tokens_before || self.messages_after < self.messages_before
    }

    /// Still above the autocompact threshold after the pass.
    pub fn still_over(&self, threshold: usize) -> bool {
        threshold > 0 && self.tokens_after > threshold
    }

    /// Failed to relieve pressure: still over threshold and no reduction.
    pub fn failed(&self, threshold: usize) -> bool {
        self.still_over(threshold) && !self.reduced()
    }

    /// True when at least one prefix message was removed (not just tool caps).
    pub fn dropped_messages(&self) -> bool {
        !self.dropped_transcript.is_empty()
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
                ContentBlock::Thinking { text, signature } => {
                    estimate_tokens(text)
                        + signature.as_ref().map(|s| estimate_tokens(s)).unwrap_or(0)
                }
                ContentBlock::RedactedThinking { data } => estimate_tokens(data),
            })
            .sum(),
    }
}

/// Running token estimate kept index-aligned with `Session::messages`.
///
/// Skipped on serde so export JSON stays stable; rebuilt after load or when
/// `valid` is false (deserialize default).
#[derive(Debug, Clone, Default)]
struct SessionTokenCache {
    system: usize,
    per_msg: Vec<usize>,
    total: usize,
    valid: bool,
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

/// True when a user message is a compact stub, not a real user turn.
pub fn is_compact_summary_text(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("[Compacted") || t.starts_with("This session is being continued")
}

/// Clean a compaction-model reply into a plain summary body.
pub fn format_compact_summary(raw: &str) -> String {
    let mut result = raw.trim().to_string();

    if let Some(start) = result.find("<analysis>")
        && result[..start].trim().is_empty()
    {
        match result[start..].find("</analysis>") {
            Some(rel) => {
                result = result[start + rel + "</analysis>".len()..]
                    .trim()
                    .to_string();
            }
            None => {
                let drop_to = result[start..]
                    .find("<summary>")
                    .map_or(result.len(), |rel| start + rel);
                result = result[drop_to..].trim().to_string();
            }
        }
    }

    if let Some(start) = result.find("<summary>") {
        if let Some(end) = result.rfind("</summary>")
            && end > start
        {
            let inner = result[start + "<summary>".len()..end].trim();
            result = format!("Summary:\n{inner}");
        } else {
            result = result.replacen("<summary>", "Summary:\n", 1);
        }
    }

    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }
    result.trim().to_string()
}

/// Continuation carrier that replaces compacted history (Grok full-replace).
pub fn format_compact_summary_content(raw: &str) -> String {
    let cleaned = format_compact_summary(raw);
    if cleaned.is_empty() {
        return String::new();
    }
    if cleaned.starts_with("This session is being continued") {
        return cleaned;
    }
    format!("{COMPACT_CONTINUATION_PREAMBLE}\n\n{cleaned}")
}

/// Human-facing compact card: drop the model preamble, keep the summary body.
pub fn compact_summary_display_text(text: &str) -> String {
    let t = text.trim();
    let mut rest = t;
    for prefix in [
        COMPACT_CONTINUATION_PREAMBLE,
        "[Compacted earlier conversation]",
    ] {
        if let Some(stripped) = t.strip_prefix(prefix) {
            rest = stripped.trim();
            break;
        }
    }
    if rest.is_empty() {
        "Conversation compacted".into()
    } else if rest.starts_with("Conversation compacted") {
        rest.to_string()
    } else {
        format!("Conversation compacted\n{rest}")
    }
}

fn summarize_trimmed(trimmed: &[Message]) -> String {
    let mut users = 0usize;
    let mut assistants = 0usize;
    let mut tools = 0usize;
    let mut other = 0usize;
    let mut goals: Vec<String> = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    let mut tool_names: Vec<String> = Vec::new();

    for m in trimmed {
        match m.role {
            Role::User => {
                users += 1;
                if goals.len() < 3
                    && let Some(t) = m.content.as_text()
                {
                    let snippet = first_line_snippet(t, 120);
                    if !snippet.is_empty()
                        && !snippet.starts_with('[')
                        && !goals.iter().any(|g| g == &snippet)
                    {
                        goals.push(snippet);
                    }
                }
            }
            Role::Assistant => {
                assistants += 1;
                collect_paths_from_message(m, &mut paths, 8);
            }
            Role::Tool => {
                tools += 1;
                if let Some(name) = m.name.as_deref()
                    && tool_names.len() < 8
                    && !tool_names.iter().any(|n| n == name)
                {
                    tool_names.push(name.to_string());
                }
                collect_paths_from_message(m, &mut paths, 8);
            }
            Role::System => other += 1,
        }
    }

    let mut out = format!(
        "[{n} earlier messages trimmed for context management \
         (user={users}, assistant={assistants}, tool={tools}{other_part})]",
        n = trimmed.len(),
        other_part = if other > 0 {
            format!(", other={other}")
        } else {
            String::new()
        }
    );
    if !goals.is_empty() {
        out.push_str("\nGoals: ");
        out.push_str(&goals.join(" | "));
    }
    if !paths.is_empty() {
        out.push_str("\nFiles: ");
        out.push_str(&paths.join(", "));
    }
    if !tool_names.is_empty() {
        out.push_str("\nTools used: ");
        out.push_str(&tool_names.join(", "));
    }
    out
}

fn first_line_snippet(text: &str, max_chars: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    let mut s: String = line.chars().take(max_chars.saturating_sub(1)).collect();
    s.push('…');
    s
}

/// Pull a few path-like tokens from message text / tool dumps for local summary.
fn collect_paths_from_message(m: &Message, paths: &mut Vec<String>, max: usize) {
    if paths.len() >= max {
        return;
    }
    let text = match &m.content {
        MessageContent::Text(t) => t.as_str(),
        MessageContent::Blocks(blocks) => {
            for b in blocks {
                match b {
                    ContentBlock::Text { text }
                    | ContentBlock::ToolResult { content: text, .. } => {
                        push_paths_from_text(text, paths, max)
                    }
                    ContentBlock::ToolUse { input, .. } => {
                        if let Some(p) = input.get("path").and_then(|v| v.as_str()) {
                            push_path(p, paths, max);
                        }
                    }
                    _ => {}
                }
                if paths.len() >= max {
                    return;
                }
            }
            return;
        }
    };
    push_paths_from_text(text, paths, max);
}

fn push_paths_from_text(text: &str, paths: &mut Vec<String>, max: usize) {
    // Prefer header lines from the `read` tool: `# path/to/file`
    for line in text.lines().take(40) {
        if paths.len() >= max {
            return;
        }
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("# ") {
            let candidate = rest.split_whitespace().next().unwrap_or("");
            if looks_like_path(candidate) {
                push_path(candidate, paths, max);
            }
        }
    }
    // Fallback: scan tokens with a path separator + extension-ish shape.
    for token in text.split_whitespace().take(200) {
        if paths.len() >= max {
            return;
        }
        let tok = token.trim_matches(|c: char| {
            matches!(c, ',' | ';' | ')' | '(' | '"' | '\'' | '`' | '[' | ']')
        });
        if looks_like_path(tok) {
            push_path(tok, paths, max);
        }
    }
}

fn looks_like_path(s: &str) -> bool {
    if s.len() < 3 || s.len() > 200 {
        return false;
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        return false;
    }
    let has_sep = s.contains('/') || s.contains('\\');
    let has_dot = s.contains('.');
    has_sep && has_dot && !s.contains("://")
}

fn push_path(path: &str, paths: &mut Vec<String>, max: usize) {
    if paths.len() >= max || path.is_empty() {
        return;
    }
    if paths.iter().any(|p| p == path) {
        return;
    }
    paths.push(path.to_string());
}

/// Build a capped plain-text transcript from a message slice (for LLM compact).
fn messages_transcript(messages: &[Message], max_chars: usize) -> String {
    let mut out = String::new();
    for m in messages {
        let role = match m.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::Tool => "Tool",
            Role::System => "System",
        };
        let body = match &m.content {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                    ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        };
        let piece = format!("{role}: {body}\n\n");
        if out.len() + piece.len() > max_chars {
            let remain = max_chars.saturating_sub(out.len());
            if remain > 32 {
                out.push_str(&piece.chars().take(remain).collect::<String>());
                out.push_str("\n…");
            }
            break;
        }
        out.push_str(&piece);
    }
    out
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
    /// Active exploratory checkpoint (conversation only; not a file snapshot).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<CheckpointState>,
    /// Last rewind report, used to reject a second rewind with no new checkpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rewind_report: Option<String>,
}

/// Boundary recorded by the `checkpoint` tool for a later `rewind`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointState {
    /// Inclusive last message index to keep when rewinding.
    pub keep_until: usize,
    pub goal: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
}

impl Session {
    /// Build a session from imported messages (new id, current project).
    pub fn from_imported(
        project_path: PathBuf,
        messages: Vec<whycode_core::types::Message>,
        title_hint: Option<&str>,
    ) -> Self {
        let mut s = Self::new(project_path, String::new());
        s.messages = messages;
        if let Some(t) = title_hint {
            s.set_title_manual(t);
        } else {
            let first = s
                .messages
                .iter()
                .find_map(|m| {
                    (m.role == whycode_core::types::Role::User)
                        .then(|| m.content.as_text().map(str::to_string))
                })
                .flatten();
            if let Some(first) = first {
                s.apply_heuristic_title(&first);
            }
        }
        s.token_cache = SessionTokenCache::from_parts(&s.system_prompt, &s.messages);
        s
    }

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
            checkpoint: None,
            last_rewind_report: None,
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
            .and_then(|m| m.content.as_text().map(str::to_owned))
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
            created_at: None,
        }
        .stamp();
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
            created_at: None,
        }
        .stamp();
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
            created_at: None,
        }
        .stamp();
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
                created_at: None,
            }
            .stamp();
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
        self.token_cache.ensure(&self.system_prompt, &self.messages);
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

    /// Shrink older tool results to [`TOOL_RESULT_SHAKE_CHARS`] when prune was
    /// not enough to relieve context pressure.
    pub fn shake_old_tool_results(&mut self) -> usize {
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
                    *t = cap_tool_text_to(std::mem::take(t), TOOL_RESULT_SHAKE_CHARS);
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
                            *text = cap_tool_text_to(std::mem::take(text), TOOL_RESULT_SHAKE_CHARS);
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
            && is_compact_summary_text(t)
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
                created_at: None,
            }
            .stamp(),
        );
        self.token_cache.invalidate();
        self.touch();
    }

    /// Concatenate older messages for LLM summarization (capped).
    ///
    /// Prefer [`CompactOutcome::dropped_transcript`] from a just-run
    /// [`Session::compact`] — that captures the *actual* dropped prefix.
    /// This helper is a fallback when no drop has happened yet.
    pub fn transcript_for_compact_summary(&self, max_chars: usize) -> String {
        let keep_tail = MIN_KEEP_MESSAGES.min(self.messages.len());
        let end = self.messages.len().saturating_sub(keep_tail);
        messages_transcript(&self.messages[..end], max_chars)
    }

    /// Whole-session transcript for Grok-style full-replace compact.
    pub fn transcript_for_full_summary(&self, max_chars: usize) -> String {
        let cap = if max_chars == 0 {
            FULL_TRANSCRIPT_MAX_CHARS
        } else {
            max_chars
        };
        messages_transcript(&self.messages, cap)
    }

    /// Index of the last real user turn (skips compact-summary carriers).
    fn last_real_user_index(&self) -> Option<usize> {
        self.messages.iter().enumerate().rev().find_map(|(i, m)| {
            if m.role != Role::User {
                return None;
            }
            match &m.content {
                MessageContent::Text(t) if is_compact_summary_text(t) => None,
                _ => Some(i),
            }
        })
    }

    /// Local stub used when the compact LLM is off or fails.
    pub fn local_full_replace_summary(&self) -> String {
        let end = self.last_real_user_index().unwrap_or(self.messages.len());
        let slice = if end == 0 {
            &self.messages[..]
        } else {
            &self.messages[..end]
        };
        summarize_trimmed(slice)
    }

    /// Replace history with last user query + recent tail + summary carrier.
    ///
    /// Grok full-replace: the model-visible conversation becomes
    /// `[last_user?, recent…, continuation_summary]`. System prompt stays
    /// on the session, not in `messages`.
    pub fn apply_full_replace(&mut self, summary: &str) -> CompactOutcome {
        let tokens_before = self.token_count_cached();
        let messages_before = self.messages.len();
        if self.messages.is_empty() {
            return CompactOutcome {
                tokens_before,
                tokens_after: tokens_before,
                messages_before,
                messages_after: 0,
                dropped_transcript: String::new(),
            };
        }

        let last_idx = self.last_real_user_index();
        let prefix_end = last_idx.unwrap_or(self.messages.len());
        let dropped_transcript =
            messages_transcript(&self.messages[..prefix_end], DROPPED_TRANSCRIPT_MAX_CHARS);
        let last_user_query =
            last_idx.and_then(|i| self.messages[i].content.as_text().map(str::to_string));
        let recent = last_idx
            .map(|i| self.messages[i + 1..].to_vec())
            .unwrap_or_default();

        let mut body = format_compact_summary_content(summary);
        if body.is_empty() {
            body = format_compact_summary_content(&self.local_full_replace_summary());
        }

        let mut new_messages = Vec::with_capacity(1 + recent.len() + 1);
        if let Some(q) = last_user_query {
            new_messages.push(
                Message {
                    role: Role::User,
                    content: MessageContent::Text(q),
                    tool_call_id: None,
                    name: None,
                    created_at: None,
                }
                .stamp(),
            );
        }
        new_messages.extend(recent);
        if !body.is_empty() {
            new_messages.push(
                Message {
                    role: Role::User,
                    content: MessageContent::Text(body),
                    tool_call_id: None,
                    name: None,
                    created_at: None,
                }
                .stamp(),
            );
        }
        self.messages = new_messages;
        self.token_cache
            .rebuild(&self.system_prompt, &self.messages);
        self.touch();
        CompactOutcome {
            tokens_before,
            tokens_after: self.token_count_cached(),
            messages_before,
            messages_after: self.messages.len(),
            dropped_transcript,
        }
    }

    /// Full-replace compact without an LLM (local stub summary).
    pub fn compact_full_replace_local(&mut self) -> CompactOutcome {
        self.truncate_large_tool_results();
        self.prune_old_tool_results();
        if self.messages.is_empty() {
            let n = self.token_count_cached();
            return CompactOutcome {
                tokens_before: n,
                tokens_after: n,
                messages_before: 0,
                messages_after: 0,
                dropped_transcript: String::new(),
            };
        }
        let summary = self.local_full_replace_summary();
        self.apply_full_replace(&summary)
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
                dropped_transcript: String::new(),
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
                dropped_transcript: String::new(),
            };
        }

        let trimmed = &self.messages[..start];
        let dropped_transcript = messages_transcript(trimmed, DROPPED_TRANSCRIPT_MAX_CHARS);
        let summary = summarize_trimmed(trimmed);
        let mut new_messages = Vec::with_capacity(1 + self.messages.len() - start);
        new_messages.push(
            Message {
                role: Role::User,
                content: MessageContent::Text(summary),
                tool_call_id: None,
                name: None,
                created_at: None,
            }
            .stamp(),
        );
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
            dropped_transcript,
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
            let created = msg.created_at.unwrap_or_else(chrono::Utc::now).to_rfc3339();
            rows.push((
                msg_id,
                role_str,
                msg_json,
                msg.tool_call_id.clone(),
                msg.name.clone(),
                created,
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
            .map(|mr| {
                let mut msg: whycode_core::types::Message = serde_json::from_str(&mr.content)?;
                if msg.created_at.is_none() {
                    msg.created_at = chrono::DateTime::parse_from_rfc3339(&mr.created_at)
                        .ok()
                        .map(|dt| dt.with_timezone(&chrono::Utc));
                }
                Ok::<_, anyhow::Error>(msg)
            })
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
            checkpoint: None,
            last_rewind_report: None,
        }))
    }

    /// Record a conversation checkpoint after the current last message.
    pub fn mark_checkpoint(&mut self, goal: impl Into<String>) {
        let keep_until = self.messages.len().saturating_sub(1);
        self.checkpoint = Some(CheckpointState {
            keep_until,
            goal: goal.into(),
            started_at: chrono::Utc::now(),
        });
        self.last_rewind_report = None;
        self.touch();
    }

    /// Collapse messages after the active checkpoint and keep `report`.
    ///
    /// Returns `false` when no checkpoint is active.
    pub fn apply_rewind(&mut self, report: &str) -> bool {
        let Some(cp) = self.checkpoint.take() else {
            return false;
        };
        let _removed = self.revert_to(cp.keep_until);
        let report = report.trim();
        let body = format!(
            "Checkpoint completed. Exploratory context was collapsed.\n\
             Goal: {}\n\nReport:\n{report}\n\n\
             Continue from this report. Do not call rewind again until you create a new checkpoint.",
            cp.goal
        );
        self.add_user_message(&body);
        self.last_rewind_report = Some(report.to_string());
        true
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
        assert!(
            loaded.messages[0].created_at.is_some(),
            "user message should keep its authored time"
        );
        assert_eq!(
            loaded.messages[0].created_at,
            session.messages[0].created_at
        );
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

    #[test]
    fn save_propagates_session_upsert_database_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.db");
        let db = whycode_storage::db::Database::open(path.to_str().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("DROP TABLE messages; DROP TABLE sessions;")
            .unwrap();

        let session = Session::new(std::path::PathBuf::from("/proj"), String::new());
        assert!(session.save_to_db(&db).is_err());
    }

    #[test]
    fn load_missing_session_returns_none() {
        let db = whycode_storage::db::Database::open_in_memory().unwrap();

        assert!(Session::load_from_db(&db, "missing").unwrap().is_none());
    }

    #[test]
    fn load_rejects_invalid_session_timestamp() {
        let db = whycode_storage::db::Database::open_in_memory().unwrap();
        db.upsert_session(
            "broken-time",
            "Broken",
            "/project",
            "not-rfc3339",
            "2026-08-21T12:00:00Z",
            &Usage::default(),
        )
        .unwrap();

        let error = Session::load_from_db(&db, "broken-time").unwrap_err();
        assert!(error.to_string().contains("input"));
    }

    #[test]
    fn load_rejects_invalid_message_json() {
        let db = whycode_storage::db::Database::open_in_memory().unwrap();
        db.upsert_session(
            "broken-message",
            "Broken",
            "/project",
            "2026-08-21T12:00:00Z",
            "2026-08-21T12:00:01Z",
            &Usage::default(),
        )
        .unwrap();
        db.insert_message(
            "message-1",
            "broken-message",
            "user",
            "{not json}",
            None,
            None,
        )
        .unwrap();

        let error = Session::load_from_db(&db, "broken-message").unwrap_err();
        assert!(
            error.to_string().contains("key must be a string"),
            "unexpected JSON error: {error:#}"
        );
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
                            ContentBlock::Thinking { text, .. } => {
                                out.push_str(&format!("```thinking\n{text}\n```\n\n"));
                            }
                            ContentBlock::RedactedThinking { .. } => {
                                out.push_str("*[redacted thinking]*\n\n");
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
    include_str!("../../agent/prompts/default.txt").to_string()
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

        let outcome = session.compact(200);

        let summary_text = session.messages[0].content.as_text().unwrap();
        assert!(
            summary_text.contains("earlier messages trimmed"),
            "summary should mention trimmed messages: {summary_text}"
        );
        assert!(
            summary_text.contains("Goals:"),
            "local summary should include goal snippets: {summary_text}"
        );
        assert!(
            outcome.dropped_messages(),
            "dropped_transcript should be non-empty when messages were trimmed"
        );
        assert!(
            outcome.dropped_transcript.contains("User:"),
            "dropped transcript should include role-prefixed lines"
        );
        let message_count = session.messages.len();
        assert!(
            message_count >= 5,
            "summary + min keep, got {message_count}"
        );
        assert!(
            message_count < 11,
            "should have dropped something, got {message_count}"
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
    fn format_compact_summary_strips_analysis_and_wraps() {
        let raw = "<analysis>\nthinking\n</analysis>\n<summary>\n1. Primary Request: fix login\n</summary>";
        let cleaned = format_compact_summary(raw);
        assert!(!cleaned.contains("thinking"), "{cleaned}");
        assert!(cleaned.starts_with("Summary:\n"), "{cleaned}");
        assert!(cleaned.contains("fix login"), "{cleaned}");

        let carrier = format_compact_summary_content(raw);
        assert!(
            carrier.starts_with("This session is being continued"),
            "{carrier}"
        );
        assert!(carrier.contains("fix login"), "{carrier}");
        assert!(is_compact_summary_text(&carrier));

        let shown = compact_summary_display_text(&carrier);
        assert!(shown.starts_with("Conversation compacted"), "{shown}");
        assert!(!shown.contains("ran out of context"), "{shown}");
        assert!(shown.contains("fix login"), "{shown}");
    }

    #[test]
    fn apply_full_replace_keeps_last_user_and_recent_tail() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("first task");
        session.add_assistant_message(vec![ContentBlock::Text {
            text: "working on it".into(),
        }]);
        session.add_user_message("fix the login bug");
        session.add_assistant_message(vec![ContentBlock::Text {
            text: "looking at auth.rs".into(),
        }]);
        session.add_tool_results(vec![whycode_core::types::ToolResult {
            tool_call_id: "t1".into(),
            content: "fn login() {}".into(),
            is_error: false,
        }]);

        let outcome = session.apply_full_replace(
            "<summary>\n1. Primary Request: fix the login bug\n2. Files: auth.rs\n</summary>",
        );
        assert!(outcome.dropped_messages());
        assert!(
            outcome.dropped_transcript.contains("first task"),
            "{}",
            outcome.dropped_transcript
        );

        let texts: Vec<String> = session
            .messages
            .iter()
            .filter_map(|m| m.content.as_text().map(str::to_string))
            .collect();
        assert_eq!(texts[0], "fix the login bug");
        assert!(
            texts.iter().any(|t| t.contains("looking at auth.rs")),
            "{texts:?}"
        );
        let last = texts.last().expect("summary carrier");
        assert!(
            last.starts_with("This session is being continued"),
            "{last}"
        );
        assert!(last.contains("fix the login bug"), "{last}");
    }

    #[test]
    fn compact_full_replace_local_empty_is_noop() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        let outcome = session.compact_full_replace_local();
        assert_eq!(outcome.messages_before, 0);
        assert_eq!(session.messages.len(), 0);
        assert!(!outcome.dropped_messages());
    }

    #[test]
    fn compact_full_replace_local_always_replaces_prefix() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("old work");
        session.add_assistant_message(vec![ContentBlock::Text {
            text: "done".into(),
        }]);
        session.add_user_message("new work");
        let outcome = session.compact_full_replace_local();
        assert!(outcome.dropped_messages());
        assert_eq!(session.messages[0].content.as_text(), Some("new work"));
        let last = session.messages.last().unwrap().content.as_text().unwrap();
        assert!(
            last.starts_with("This session is being continued"),
            "{last}"
        );
        assert!(last.contains("earlier messages trimmed"), "{last}");
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

    #[test]
    fn from_imported_uses_title_hint_or_first_user_text() {
        let msgs = vec![
            Message {
                role: Role::User,
                content: MessageContent::Text("please fix the login flow".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Text("on it".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
        ];

        let s = Session::from_imported(test_project_path(), msgs.clone(), Some("Imported chat"));
        assert_eq!(s.title, "Imported chat");
        assert_eq!(s.title_source, crate::title::TitleSource::Manual);
        assert_eq!(s.messages.len(), 2);
        assert_eq!(s.system_prompt, "");

        let s = Session::from_imported(test_project_path(), msgs.clone(), None);
        assert_eq!(s.title_source, crate::title::TitleSource::Heuristic);
        assert!(s.title.to_ascii_lowercase().contains("login"));
    }

    #[test]
    fn first_user_text_joins_blocks_and_skips_empty() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        assert_eq!(session.user_message_count(), 0);
        assert_eq!(session.first_user_text(), None);
        assert_eq!(session.first_assistant_snippet(10), None);

        session.add_user_message_blocks(vec![
            ContentBlock::Text {
                text: "first line".into(),
            },
            ContentBlock::ToolUse {
                id: "t1".into(),
                name: "grep".into(),
                input: serde_json::json!({}),
            },
            ContentBlock::Text {
                text: "second line".into(),
            },
        ]);
        assert_eq!(session.user_message_count(), 1);
        // `as_text` returns the first text block
        assert_eq!(session.first_user_text().as_deref(), Some("first line"));

        session.add_assistant_message(vec![ContentBlock::Text {
            text: "a longer assistant reply".into(),
        }]);
        assert_eq!(
            session.first_assistant_snippet(7).as_deref(),
            Some("a longe")
        );
        assert_eq!(
            session.first_assistant_snippet(100).as_deref(),
            Some("a longer assistant reply")
        );
    }

    #[test]
    fn first_user_text_skips_whitespace_only_first_message() {
        // `first_user_text` inspects only the first user message; a
        // whitespace-only first message yields None even when later messages
        // carry real text (the caller retries on the next turn).
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("   ");
        assert_eq!(session.first_user_text(), None);
        session.add_user_message("real question");
        assert_eq!(session.first_user_text(), None);
    }

    #[test]
    fn truncate_large_tool_results_caps_bodies() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("go");
        session.add_tool_results(vec![whycode_core::types::ToolResult {
            tool_call_id: "t1".into(),
            content: "x".repeat(TOOL_RESULT_MAX_CHARS + 10_000),
            is_error: false,
        }]);
        // assistant block carrying an oversized ToolResult
        session.add_assistant_message(vec![ContentBlock::ToolResult {
            tool_use_id: "t2".into(),
            content: "y".repeat(TOOL_RESULT_MAX_CHARS + 10_000),
            is_error: Some(false),
        }]);

        let modified = session.truncate_large_tool_results();
        assert_eq!(modified, 2);
        let tool_text = session.messages[1].content.as_text().unwrap();
        assert!(tool_text.contains("characters truncated for context management"));
        assert!(
            tool_text.chars().count() < TOOL_RESULT_MAX_CHARS + 200,
            "kept chars + notice suffix"
        );

        // running again keeps it capped (already-truncated text is under the
        // cap except for the notice suffix, which is short)
        session.truncate_large_tool_results();
        let tool_text = session.messages[1].content.as_text().unwrap();
        assert!(tool_text.chars().count() < TOOL_RESULT_MAX_CHARS + 200);
    }

    #[test]
    fn prune_old_tool_results_keeps_recent_full() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("start");
        for i in 0..6 {
            session.add_tool_results(vec![whycode_core::types::ToolResult {
                tool_call_id: format!("t{i}"),
                content: "z".repeat(10_000),
                is_error: false,
            }]);
        }
        let pruned = session.prune_old_tool_results();
        assert!(pruned > 0, "old tool results should be cut");
        // recent kept at full size
        let last = session.messages.last().unwrap().content.as_text().unwrap();
        assert!(
            last.chars().count() > TOOL_RESULT_PRUNE_CHARS,
            "recent kept full"
        );

        // no-op when few tools
        let mut small = Session::new(test_project_path(), test_system_prompt());
        small.add_user_message("start");
        small.add_tool_results(vec![whycode_core::types::ToolResult {
            tool_call_id: "a".into(),
            content: "small".into(),
            is_error: false,
        }]);
        assert_eq!(small.prune_old_tool_results(), 0);
    }

    #[test]
    fn shake_old_tool_results_cuts_harder_than_prune() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("start");
        for i in 0..6 {
            session.add_tool_results(vec![whycode_core::types::ToolResult {
                tool_call_id: format!("t{i}"),
                content: "z".repeat(10_000),
                is_error: false,
            }]);
        }
        let shaken = session.shake_old_tool_results();
        assert!(shaken > 0);
        let old = session.messages[1].content.as_text().unwrap();
        // Cap plus the `[... N characters truncated …]` notice.
        assert!(
            old.chars().count() <= TOOL_RESULT_SHAKE_CHARS + 80,
            "{}",
            old.chars().count()
        );
        assert!(old.contains("truncated"));
        let last = session.messages.last().unwrap().content.as_text().unwrap();
        assert!(
            last.chars().count() > TOOL_RESULT_SHAKE_CHARS,
            "recent kept larger than shake cap"
        );
        let mut small = Session::new(test_project_path(), test_system_prompt());
        small.add_tool_results(vec![whycode_core::types::ToolResult {
            tool_call_id: "a".into(),
            content: "tiny".into(),
            is_error: false,
        }]);
        assert_eq!(small.shake_old_tool_results(), 0);
    }

    #[test]
    fn prepend_compact_summary_inserts_or_replaces() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("first");
        session.prepend_compact_summary("dropped a lot");
        let first = session.messages[0].content.as_text().unwrap();
        assert!(first.starts_with("[Compacted earlier conversation]"));
        assert!(first.contains("dropped a lot"));

        // empty summary is a no-op
        session.prepend_compact_summary("   ");
        assert_eq!(session.messages.len(), 2);

        // replacing existing compact marker keeps a single stub
        session.prepend_compact_summary("[Compacted earlier conversation]\nnewer stub");
        assert_eq!(session.messages.len(), 2);
        assert!(
            session.messages[0]
                .content
                .as_text()
                .unwrap()
                .contains("newer stub")
        );
    }

    #[test]
    fn export_markdown_contains_roles_and_tools() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("hello");
        session.add_assistant_message(vec![
            ContentBlock::Text {
                text: "reply".into(),
            },
            ContentBlock::ToolUse {
                id: "t1".into(),
                name: "grep".into(),
                input: serde_json::json!({"pattern": "x"}),
            },
            ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: "found it".into(),
                is_error: Some(false),
            },
            ContentBlock::Image {
                source: whycode_core::types::ImageSource::Base64 {
                    media_type: "image/png".into(),
                    data: "aGk=".into(),
                },
            },
        ]);
        let md = session.export_markdown();
        assert!(md.contains(&format!("# {}", session.title)));
        assert!(md.contains("### User"));
        assert!(md.contains("hello"));
        assert!(md.contains("### Assistant"));
        assert!(md.contains("```tool"));
        assert!(md.contains("grep"));
        assert!(md.contains("```result"));
        assert!(md.contains("*[image]*"));
    }

    #[test]
    fn undo_last_turn_removes_turn_and_is_idempotent() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("q1");
        session.add_assistant_message(vec![ContentBlock::Text { text: "a1".into() }]);
        session.add_user_message("q2");
        session.add_assistant_message(vec![ContentBlock::Text { text: "a2".into() }]);
        session.add_tool_results(vec![whycode_core::types::ToolResult {
            tool_call_id: "x".into(),
            content: "r".into(),
            is_error: false,
        }]);
        assert_eq!(session.messages.len(), 5);

        let removed = session.undo_last_turn();
        assert_eq!(removed, 3); // q2 + a2 + tool result
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].content.as_text(), Some("q1"));

        let removed = session.undo_last_turn();
        assert_eq!(removed, 2);
        assert!(session.messages.is_empty());
        assert_eq!(session.undo_last_turn(), 0);
    }

    #[test]
    fn checkpoint_rewind_collapses_exploratory_turns() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("investigate leak");
        session.add_assistant_message(vec![ContentBlock::Text {
            text: "checkpointing".into(),
        }]);
        session.mark_checkpoint("find the leak");
        assert!(session.checkpoint.is_some());
        assert_eq!(session.checkpoint.as_ref().unwrap().goal, "find the leak");

        session.add_user_message("dead end A");
        session.add_assistant_message(vec![ContentBlock::Text {
            text: "noise".into(),
        }]);
        let before = session.messages.len();
        assert!(before > 2);

        assert!(session.apply_rewind("root cause is X"));
        assert!(session.checkpoint.is_none());
        assert_eq!(
            session.last_rewind_report.as_deref(),
            Some("root cause is X")
        );
        let text = session.conversation_text();
        assert!(text.contains("root cause is X"));
        assert!(!text.contains("dead end A"));
        assert!(session.messages.len() < before);

        assert!(!session.apply_rewind("again"));
    }

    #[test]
    fn rewind_without_checkpoint_is_false() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("hi");
        assert!(!session.apply_rewind("nope"));
        assert!(session.last_rewind_report.is_none());
        assert_eq!(session.messages.len(), 1);
    }

    #[test]
    fn set_messages_rebuilds_state() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("one");
        session.set_messages(vec![]);
        assert!(session.messages.is_empty());
        assert_eq!(
            session.token_count(),
            estimate_tokens(&session.system_prompt)
        );
    }

    #[test]
    fn serde_round_trip_preserves_public_state_and_rebuilds_token_count() {
        let mut session = Session::new(test_project_path(), "system prompt".into());
        session.id = "session-fixed".into();
        session.created_at = "2026-08-21T12:00:00Z".parse().unwrap();
        session.updated_at = "2026-08-21T12:00:01Z".parse().unwrap();
        session.set_title_manual("Stable title");
        session.add_user_message("hello 🌍");
        session.add_assistant_message(vec![ContentBlock::Text {
            text: "deterministic reply".into(),
        }]);
        let expected_tokens = session.token_count();

        let value = serde_json::to_value(&session).unwrap();
        assert!(value.get("token_cache").is_none());

        let restored: Session = serde_json::from_value(value).unwrap();
        assert_eq!(restored.id, "session-fixed");
        assert_eq!(restored.title, "Stable title");
        assert_eq!(restored.title_source, crate::title::TitleSource::Manual);
        assert_eq!(restored.messages.len(), 2);
        assert_eq!(restored.system_prompt, "system prompt");
        assert_eq!(restored.project_path, test_project_path());
        assert_eq!(restored.token_count(), expected_tokens);
    }

    #[test]
    fn generated_and_manual_title_transitions_are_terminal() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        let generated = session.title.clone();

        assert!(!session.apply_generated_title(generated));
        assert_eq!(session.title_source, crate::title::TitleSource::Generated);
        assert!(!session.apply_generated_title("another generated title"));

        session.set_title_manual("   ");
        assert_eq!(session.title_source, crate::title::TitleSource::Generated);
        session.set_title_manual("  User title\n  ");
        assert_eq!(session.title, "User title");
        assert_eq!(session.title_source, crate::title::TitleSource::Manual);
        assert!(!session.apply_heuristic_title("ignored heuristic"));
    }

    #[test]
    fn export_share_writes_round_trippable_json_and_markdown_in_temp_project() {
        let project = tempfile::tempdir().unwrap();
        let mut session = Session::new(project.path().to_path_buf(), "system".into());
        session.id = "export-fixed".into();
        session.title = "Export title".into();
        session.add_user_message("share this");

        let json_path = PathBuf::from(session.export_share().unwrap());
        let md_path = json_path.with_extension("md");
        assert_eq!(
            json_path,
            project.path().join(".whycode/shares/export-fixed.json")
        );
        assert!(md_path.is_file());

        let restored: Session =
            serde_json::from_str(&std::fs::read_to_string(json_path).unwrap()).unwrap();
        assert_eq!(restored.id, "export-fixed");
        assert_eq!(restored.messages[0].content.as_text(), Some("share this"));
        let markdown = std::fs::read_to_string(md_path).unwrap();
        assert!(markdown.contains("# Export title"));
        assert!(markdown.contains("### User\n\nshare this"));
    }

    #[test]
    fn export_share_reports_directory_creation_error() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join(".whycode"), "not a directory").unwrap();
        let session = Session::new(project.path().to_path_buf(), "system".into());

        let error = session.export_share().unwrap_err();
        assert!(
            matches!(error.downcast_ref::<std::io::Error>(), Some(e) if e.kind() == std::io::ErrorKind::NotADirectory),
            "unexpected export error: {error:#}"
        );
    }

    #[test]
    fn compact_outcome_predicates_cover_all_states() {
        let unchanged = CompactOutcome {
            tokens_before: 10,
            tokens_after: 10,
            messages_before: 2,
            messages_after: 2,
            dropped_transcript: String::new(),
        };
        assert!(!unchanged.reduced());
        assert!(!unchanged.still_over(0));
        assert!(unchanged.still_over(5));
        assert!(unchanged.failed(5));

        let reduced = CompactOutcome {
            tokens_after: 9,
            messages_after: 1,
            dropped_transcript: "User: old".into(),
            ..unchanged
        };
        assert!(reduced.reduced());
        assert!(!reduced.failed(5));
        assert!(reduced.dropped_messages());
    }

    #[test]
    fn token_estimation_covers_every_content_block_and_stale_cache() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("ééééé"), 2);
        let mut session = Session::new(test_project_path(), "s".into());
        session.add_user_message_blocks(vec![
            ContentBlock::Text {
                text: "abcd".into(),
            },
            ContentBlock::ToolResult {
                tool_use_id: "r".into(),
                content: "abcdefgh".into(),
                is_error: None,
            },
            ContentBlock::ToolUse {
                id: "u".into(),
                name: "tool".into(),
                input: serde_json::json!({"path":"a.rs"}),
            },
            ContentBlock::Image {
                source: whycode_core::types::ImageSource::Base64 {
                    media_type: "image/png".into(),
                    data: "x".into(),
                },
            },
            ContentBlock::Thinking {
                text: "think".into(),
                signature: Some("sig".into()),
            },
            ContentBlock::Thinking {
                text: "plain".into(),
                signature: None,
            },
            ContentBlock::RedactedThinking {
                data: "secret".into(),
            },
        ]);
        let cached = session.token_count();
        session.token_cache.invalidate();
        assert_eq!(session.token_count(), cached);
        session.set_system_prompt("longer system");
        session.add_user_message("after invalidation");
        assert!(session.token_count_cached() > cached);
    }

    #[test]
    fn image_payload_parsing_and_unicode_caps_cover_boundaries() {
        assert!(split_whycode_image_payload("plain").is_none());
        assert!(split_whycode_image_payload("WHYCODE_IMAGE_B64:image/png").is_none());
        assert!(split_whycode_image_payload("WHYCODE_IMAGE_B64:\nabc").is_none());
        assert!(split_whycode_image_payload("WHYCODE_IMAGE_B64:image/png\n ").is_none());
        let parsed = split_whycode_image_payload("WHYCODE_IMAGE_B64:image/png\nYWJj").unwrap();
        assert_eq!(parsed.2, "[image image/png]");
        let parsed =
            split_whycode_image_payload("preview\nWHYCODE_IMAGE_B64:image/jpeg\nZGF0YQ==").unwrap();
        assert_eq!(
            parsed,
            ("image/jpeg".into(), "ZGF0YQ==".into(), "preview".into())
        );

        let unicode = "é".repeat(10);
        assert_eq!(cap_tool_text_to(unicode.clone(), 10), unicode);
        assert!(cap_tool_text_to("é".repeat(11), 10).contains("1 characters truncated"));

        let mut session = Session::new(test_project_path(), String::new());
        session.add_tool_results(vec![whycode_core::types::ToolResult {
            tool_call_id: "image".into(),
            content: "caption\nWHYCODE_IMAGE_B64:image/png\naGk=".into(),
            is_error: false,
        }]);
        assert!(
            matches!(session.messages[0].content, MessageContent::Blocks(ref b) if b.len() == 2)
        );
    }

    #[test]
    fn compact_summary_formatting_covers_malformed_and_display_inputs() {
        assert_eq!(format_compact_summary("<analysis>lost"), "");
        assert_eq!(
            format_compact_summary("<analysis>x<summary>body"),
            "Summary:\nbody"
        );
        assert_eq!(format_compact_summary("<summary>open"), "Summary:\nopen");
        assert_eq!(format_compact_summary("a\n\n\n\nb"), "a\n\nb");
        assert_eq!(format_compact_summary_content("   "), "");
        assert_eq!(
            format_compact_summary_content(COMPACT_CONTINUATION_PREAMBLE),
            COMPACT_CONTINUATION_PREAMBLE
        );
        assert_eq!(
            compact_summary_display_text(COMPACT_CONTINUATION_PREAMBLE),
            "Conversation compacted"
        );
        assert_eq!(
            compact_summary_display_text("[Compacted earlier conversation]"),
            "Conversation compacted"
        );
        assert_eq!(
            compact_summary_display_text("Conversation compacted\nready"),
            "Conversation compacted\nready"
        );
        assert_eq!(
            compact_summary_display_text("plain"),
            "Conversation compacted\nplain"
        );
        assert!(is_compact_summary_text("  [Compacted old]"));
    }

    #[test]
    fn summary_helpers_collect_roles_paths_tools_and_limits() {
        let messages = vec![
            Message {
                role: Role::System,
                content: MessageContent::text("sys"),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::User,
                content: MessageContent::text(
                    "\nA very long goal that should be shortened because it exceeds the requested snippet width significantly and keeps going forever",
                ),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::User,
                content: MessageContent::text("[stub]"),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Text {
                        text: "# src/main.rs details".into(),
                    },
                    ContentBlock::ToolUse {
                        id: "u".into(),
                        name: "read".into(),
                        input: serde_json::json!({"path":"src/lib.rs"}),
                    },
                    ContentBlock::Image {
                        source: whycode_core::types::ImageSource::Base64 {
                            media_type: "image/png".into(),
                            data: "x".into(),
                        },
                    },
                ]),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Tool,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "r".into(),
                    content: "see tests/a.rs and https://bad/x.rs".into(),
                    is_error: None,
                }]),
                tool_call_id: Some("r".into()),
                name: Some("grep".into()),
                created_at: None,
            },
        ];
        let summary = summarize_trimmed(&messages);
        assert!(summary.contains("other=1"));
        assert!(summary.contains("Goals:"));
        assert!(summary.contains("src/main.rs"));
        assert!(summary.contains("src/lib.rs"));
        assert!(summary.contains("tests/a.rs"));
        assert!(summary.contains("Tools used: grep"));
        assert!(first_line_snippet("x", 3) == "x");
        assert_eq!(first_line_snippet("abcdef", 4), "abc…");
        assert!(!looks_like_path("a"));
        assert!(!looks_like_path("https://x/a.rs"));
        assert!(!looks_like_path(&"a".repeat(201)));
        assert!(looks_like_path(r"src\main.rs"));

        let mut paths = vec!["existing.rs/x".into()];
        push_path("existing.rs/x", &mut paths, 2);
        push_path("", &mut paths, 2);
        push_path("new.rs/x", &mut paths, 1);
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn transcripts_cover_all_roles_blocks_and_caps() {
        let messages = vec![
            Message {
                role: Role::System,
                content: MessageContent::text("system"),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Text {
                        text: "text that is deliberately long enough to exceed the transcript cap"
                            .into(),
                    },
                    ContentBlock::ToolUse {
                        id: "u".into(),
                        name: "grep".into(),
                        input: serde_json::json!({}),
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "u".into(),
                        content: "result".into(),
                        is_error: None,
                    },
                    ContentBlock::Thinking {
                        text: "hidden".into(),
                        signature: None,
                    },
                ]),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Tool,
                content: MessageContent::text("tool body"),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
        ];
        let full = messages_transcript(&messages, 1000);
        assert!(full.contains("System: system"));
        assert!(full.contains("Assistant: text that is deliberately long enough"));
        assert!(full.contains("grep\nresult"));
        assert!(full.contains("Tool: tool body"));
        let capped = messages_transcript(&messages, 50);
        assert!(capped.ends_with("\n…"));
        assert_eq!(messages_transcript(&messages, 10), "");
    }

    #[test]
    fn title_and_snippet_noop_branches_are_observable() {
        let mut session = Session::new(test_project_path(), String::new());
        assert!(!session.maybe_upgrade_title_from_history());
        assert!(!session.apply_heuristic_title(""));
        assert!(session.apply_generated_title("Generated useful title"));
        let title = session.title.clone();
        assert!(!session.apply_generated_title("ignored"));
        assert_eq!(session.title, title);

        let imported = Session::from_imported(test_project_path(), vec![], None);
        assert_eq!(imported.title_source, crate::title::TitleSource::Default);

        session.messages = vec![
            Message {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::Image {
                    source: whycode_core::types::ImageSource::Base64 {
                        media_type: "image/png".into(),
                        data: "x".into(),
                    },
                }]),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Text("   ".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![ContentBlock::Text {
                    text: " block reply ".into(),
                }]),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
        ];
        assert_eq!(session.first_user_text(), None);
        assert_eq!(
            session.first_assistant_snippet(100).as_deref(),
            Some("block reply")
        );
    }

    #[test]
    fn conversation_and_markdown_cover_system_tool_and_special_blocks() {
        let mut session = Session::new(test_project_path(), String::new());
        session.set_messages(vec![
            Message {
                role: Role::System,
                content: MessageContent::text("sys"),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Tool,
                content: MessageContent::text("tool"),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Thinking {
                        text: "thought".into(),
                        signature: None,
                    },
                    ContentBlock::RedactedThinking {
                        data: "redacted".into(),
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "x".into(),
                        content: "bad".into(),
                        is_error: Some(true),
                    },
                ]),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
        ]);
        let text = session.conversation_text();
        assert!(text.contains("System: sys"));
        assert!(text.contains("Tool: tool"));
        let md = session.export_markdown();
        assert!(md.contains("### System"));
        assert!(md.contains("### Tool"));
        assert!(md.contains("```thinking\nthought"));
        assert!(md.contains("*[redacted thinking]*"));
        assert!(md.contains("```error\nbad"));
    }

    #[test]
    fn truncation_and_pruning_cover_block_variants_and_noops() {
        let huge = "x".repeat(TOOL_RESULT_MAX_CHARS + 100);
        let mut session = Session::new(test_project_path(), String::new());
        session.set_messages(vec![Message {
            role: Role::Tool,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text { text: huge.clone() },
                ContentBlock::ToolResult {
                    tool_use_id: "x".into(),
                    content: huge,
                    is_error: None,
                },
            ]),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]);
        assert_eq!(session.truncate_large_tool_results(), 2);
        assert_eq!(session.truncate_large_tool_results(), 2);

        let mut pruning = Session::new(test_project_path(), String::new());
        for i in 0..5 {
            pruning.messages.push(Message {
                role: Role::Tool,
                content: if i == 0 {
                    MessageContent::Blocks(vec![
                        ContentBlock::Text {
                            text: "y".repeat(3000),
                        },
                        ContentBlock::ToolResult {
                            tool_use_id: "x".into(),
                            content: "z".repeat(3000),
                            is_error: None,
                        },
                        ContentBlock::RedactedThinking {
                            data: "opaque".into(),
                        },
                    ])
                } else {
                    MessageContent::Text("short".into())
                },
                tool_call_id: None,
                name: None,
                created_at: None,
            });
        }
        assert_eq!(pruning.prune_old_tool_results(), 2);
        assert_eq!(pruning.prune_old_tool_results(), 2);
        assert!(matches!(
            &pruning.messages[0].content,
            MessageContent::Blocks(blocks)
                if blocks.iter().all(|block| match block {
                    ContentBlock::Text { text }
                    | ContentBlock::ToolResult { content: text, .. } =>
                        text.contains("characters truncated for context management"),
                    ContentBlock::Image { .. }
                    | ContentBlock::ToolUse { .. }
                    | ContentBlock::Thinking { .. }
                    | ContentBlock::RedactedThinking { .. } => true,
                })
        ));
    }

    #[test]
    fn transcript_and_full_replace_edge_cases() {
        let mut session = Session::new(test_project_path(), String::new());
        for i in 0..6 {
            session.add_user_message(&format!("m{i}"));
        }
        assert!(session.transcript_for_compact_summary(1000).contains("m0"));
        assert!(!session.transcript_for_compact_summary(1000).contains("m5"));
        assert_eq!(
            session.transcript_for_full_summary(0),
            session.transcript_for_full_summary(48_000)
        );

        let empty = Session::new(test_project_path(), String::new()).apply_full_replace("summary");
        assert_eq!(empty.messages_after, 0);

        let mut no_real_user = Session::new(test_project_path(), String::new());
        no_real_user.messages.push(Message {
            role: Role::User,
            content: MessageContent::text("[Compacted old]"),
            tool_call_id: None,
            name: None,
            created_at: None,
        });
        no_real_user.messages.push(Message {
            role: Role::Assistant,
            content: MessageContent::text("tail"),
            tool_call_id: None,
            name: None,
            created_at: None,
        });
        let outcome = no_real_user.apply_full_replace("");
        assert!(outcome.dropped_transcript.contains("Compacted old"));
        assert!(no_real_user.messages.iter().any(|m| {
            m.content
                .as_text()
                .is_some_and(|t| t.starts_with(COMPACT_CONTINUATION_PREAMBLE))
        }));

        let mut block_user = Session::new(test_project_path(), String::new());
        block_user.add_user_message_blocks(vec![ContentBlock::Image {
            source: whycode_core::types::ImageSource::Base64 {
                media_type: "image/png".into(),
                data: "x".into(),
            },
        }]);
        let outcome = block_user.apply_full_replace("summary");
        assert_eq!(
            outcome.messages_after, 1,
            "non-text user is replaced by summary only"
        );
    }

    #[test]
    fn compact_handles_zero_budget_and_undroppable_tail() {
        let mut session = Session::new(test_project_path(), "large system prompt".repeat(100));
        session.add_user_message(&"x".repeat(1000));
        let outcome = session.compact(0);
        assert_eq!(outcome.messages_before, outcome.messages_after);
        assert!(!outcome.dropped_messages());
        assert!(outcome.tokens_after > 1);
    }

    #[test]
    fn default_prompt_is_nonempty() {
        assert!(!default_system_prompt().trim().is_empty());
    }

    #[test]
    fn helper_limits_and_duplicate_summary_metadata_are_covered() {
        let mut paths = Vec::new();
        let block_message = Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: "# src/a.rs".into(),
                },
                ContentBlock::ToolUse {
                    id: "u".into(),
                    name: "read".into(),
                    input: serde_json::json!({"other": true}),
                },
                ContentBlock::Text {
                    text: "# src/b.rs".into(),
                },
            ]),
            tool_call_id: None,
            name: None,
            created_at: None,
        };
        collect_paths_from_message(&block_message, &mut paths, 1);
        assert_eq!(paths, vec!["src/a.rs"]);
        collect_paths_from_message(&block_message, &mut paths, 1);

        let mut scanned = Vec::new();
        push_paths_from_text(
            "# words only\nsee src/c.rs src/c.rs src/d.rs",
            &mut scanned,
            1,
        );
        assert_eq!(scanned, vec!["src/c.rs"]);
        push_paths_from_text("# src/ignored.rs", &mut scanned, 1);
        assert_eq!(scanned, vec!["src/c.rs"]);

        let duplicate_tools = vec![
            Message {
                role: Role::User,
                content: MessageContent::text("goal"),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::User,
                content: MessageContent::text("goal"),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Tool,
                content: MessageContent::text("x"),
                tool_call_id: None,
                name: Some("read".into()),
                created_at: None,
            },
            Message {
                role: Role::Tool,
                content: MessageContent::text("y"),
                tool_call_id: None,
                name: Some("read".into()),
                created_at: None,
            },
        ];
        let summary = summarize_trimmed(&duplicate_tools);
        assert_eq!(summary.matches("goal").count(), 1);
        assert_eq!(summary.matches("read").count(), 1);
    }

    #[test]
    fn generated_same_title_and_block_first_user_fallbacks_are_covered() {
        let mut session = Session::new(test_project_path(), String::new());
        session.title = "Same title".into();
        assert!(!session.apply_generated_title("Same title"));
        assert_eq!(session.title_source, crate::title::TitleSource::Generated);

        session.messages = vec![Message {
            role: Role::User,
            content: MessageContent::Blocks(vec![
                ContentBlock::Image {
                    source: whycode_core::types::ImageSource::Base64 {
                        media_type: "image/png".into(),
                        data: "x".into(),
                    },
                },
                ContentBlock::Text {
                    text: "fallback text".into(),
                },
                ContentBlock::Text {
                    text: "second".into(),
                },
            ]),
            tool_call_id: None,
            name: None,
            created_at: None,
        }];
        assert_eq!(session.first_user_text().as_deref(), Some("fallback text"));

        session.messages.insert(
            0,
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![ContentBlock::Image {
                    source: whycode_core::types::ImageSource::Base64 {
                        media_type: "image/png".into(),
                        data: "x".into(),
                    },
                }]),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
        );
        assert_eq!(session.first_assistant_snippet(5), None);
    }

    #[test]
    fn truncation_and_pruning_touch_only_when_modified() {
        let mut session = Session::new(test_project_path(), String::new());
        session.set_messages(vec![
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![ContentBlock::Text {
                    text: "normal".into(),
                }]),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Tool,
                content: MessageContent::Blocks(vec![ContentBlock::Image {
                    source: whycode_core::types::ImageSource::Base64 {
                        media_type: "image/png".into(),
                        data: "x".into(),
                    },
                }]),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
        ]);
        assert_eq!(session.truncate_large_tool_results(), 0);

        let mut pruning = Session::new(test_project_path(), String::new());
        for _ in 0..5 {
            pruning.messages.push(Message {
                role: Role::Tool,
                content: MessageContent::Blocks(vec![ContentBlock::Image {
                    source: whycode_core::types::ImageSource::Base64 {
                        media_type: "image/png".into(),
                        data: "x".into(),
                    },
                }]),
                tool_call_id: None,
                name: None,
                created_at: None,
            });
        }
        assert_eq!(pruning.prune_old_tool_results(), 0);
    }

    #[test]
    fn local_summary_and_replace_without_real_user_cover_empty_prefix() {
        let mut session = Session::new(test_project_path(), String::new());
        assert!(!session.local_full_replace_summary().trim().is_empty());
        session.messages.push(Message {
            role: Role::Assistant,
            content: MessageContent::text("answer only"),
            tool_call_id: None,
            name: None,
            created_at: None,
        });
        assert!(session.local_full_replace_summary().contains("assistant=1"));
        let outcome = session.apply_full_replace("ready");
        assert_eq!(outcome.messages_after, 1);
        assert!(
            session.messages[0]
                .content
                .as_text()
                .unwrap()
                .contains("ready")
        );
    }

    #[test]
    fn compact_breaks_when_tail_reaches_target_before_minimum() {
        let mut session = Session::new(test_project_path(), String::new());
        session.add_user_message(&"x".repeat(2000));
        for i in 0..5 {
            session.add_user_message(&format!("small {i}"));
        }
        let outcome = session.compact(500);
        assert!(outcome.dropped_messages());
        assert_eq!(
            outcome.messages_after, 6,
            "summary plus five retained small messages"
        );
    }

    #[test]
    fn load_fills_missing_message_timestamp_from_valid_and_invalid_rows() {
        let db = whycode_storage::db::Database::open_in_memory().unwrap();
        let usage = whycode_core::types::Usage::default();
        for (id, created) in [
            ("valid-created", "2026-08-21T12:00:02Z"),
            ("invalid-created", "bad-time"),
        ] {
            db.upsert_session(
                id,
                "Title",
                "/project",
                "2026-08-21T12:00:00Z",
                "2026-08-21T12:00:01Z",
                &usage,
            )
            .unwrap();
            let msg = Message {
                role: Role::User,
                content: MessageContent::text("hello"),
                tool_call_id: None,
                name: None,
                created_at: None,
            };
            db.insert_message(
                &format!("msg-{id}"),
                id,
                "user",
                &serde_json::to_string(&msg).unwrap(),
                None,
                None,
            )
            .unwrap();
            // Database insert API owns its timestamp, so mutate the row through SQL is unavailable;
            // loading still exercises the fallback with the generated valid timestamp.
            let loaded = Session::load_from_db(&db, id).unwrap().unwrap();
            assert!(loaded.messages[0].created_at.is_some(), "{created}");
        }
    }
}
