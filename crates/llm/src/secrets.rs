//! Secret redaction for provider-bound text.
//!
//! Values that look like credentials — environment variables whose names say
//! so, well-known token shapes, URL passwords, `KEY=value` lines in tool
//! output — become placeholders before a request leaves the machine. The
//! agent restores placeholders inside model-authored tool arguments before
//! execution, so the model can route a secret it never saw. Placeholders are
//! keyed HMAC tags, stable for one install (`<data_dir>/secret-salt`), so the
//! prompt-cache prefix does not change between steps.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use regex::Regex;
use sha2::{Digest, Sha256};
use whycodes_core::types::{ContentBlock, LlmRequest, Message, MessageContent};

/// `$$SECRET:<16 hex>$$`.
pub const PLACEHOLDER_PREFIX: &str = "$$SECRET:";
pub const PLACEHOLDER_SUFFIX: &str = "$$";
/// Shorter environment values are not treated as secrets.
pub const MIN_ENV_SECRET_LEN: usize = 8;
/// Per-install HMAC key, hex, under the data directory.
pub const KEY_FILE: &str = "secret-salt";

/// Token shapes redacted wherever they appear. A capture group names the
/// secret part; without one the whole match is the secret.
const PATTERNS: &[&str] = &[
    r"\bsk-[A-Za-z0-9_\-]{20,}",
    r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{20,}",
    r"\bgithub_pat_[A-Za-z0-9_]{20,}",
    r"\bxox[abprs]-[A-Za-z0-9\-]{10,}",
    r"\bAKIA[0-9A-Z]{16}\b",
    r"\bAIza[0-9A-Za-z_\-]{30,}",
    r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
    r"://[^/\s:@]+:([^@\s/]{4,})@",
    r"(?i)\bbearer\s+([A-Za-z0-9._\-]{20,})",
    r#"(?i)\b(?:api[_-]?key|secret|token|password|passwd)\s*[=:]\s*["']?([A-Za-z0-9._\-]{12,})"#,
];

/// Environment variable names that hold credentials.
const ENV_NAME: &str = r"(?i)(KEY|SECRET|TOKEN|PASSWORD|PASSWD|CREDENTIAL)";

#[derive(Default)]
struct State {
    /// secret → placeholder
    by_secret: HashMap<String, String>,
    /// placeholder → secret
    by_placeholder: HashMap<String, String>,
}

static INSTALLED: OnceLock<Arc<Redactor>> = OnceLock::new();

/// Make `redactor` the process default: every `LlmTransport` without an
/// explicit redactor uses it, and the agent restores placeholders with it.
/// The first call wins; later calls return `false` and change nothing.
pub fn install(redactor: Arc<Redactor>) -> bool {
    INSTALLED.set(redactor).is_ok()
}

/// The process default set by [`install`], if any.
pub fn installed() -> Option<Arc<Redactor>> {
    INSTALLED.get().cloned()
}

/// Replaces secrets with placeholders and placeholders with secrets.
pub struct Redactor {
    key: [u8; 32],
    patterns: Vec<Regex>,
    env_name: Option<Regex>,
    state: Mutex<State>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redactor")
            .field("known", &self.len())
            .finish()
    }
}

impl Redactor {
    /// A redactor with no registered secrets; token shapes still apply.
    pub fn with_key(key: [u8; 32]) -> Self {
        Self {
            key,
            patterns: compile_patterns(PATTERNS),
            env_name: compile_patterns(&[ENV_NAME]).pop(),
            state: Mutex::new(State::default()),
        }
    }

    /// `with_key` plus every credential-looking variable of this process.
    pub fn from_env(key: [u8; 32]) -> Self {
        let redactor = Self::with_key(key);
        redactor.register_env(std::env::vars().collect());
        redactor
    }

    /// Read the per-install key from `dir/secret-salt`, creating it on the
    /// first run (owner-only on Unix).
    pub fn load_or_create_key(dir: &Path) -> std::io::Result<[u8; 32]> {
        let path = dir.join(KEY_FILE);
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Some(key) = parse_key_hex(text.trim())
        {
            return Ok(key);
        }
        let mut key = [0u8; 32];
        key[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        key[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        std::fs::create_dir_all(dir)?;
        std::fs::write(&path, hex(&key)).and_then(|()| restrict_permissions(&path))?;
        Ok(key)
    }

    /// Register the values of credential-looking variables. Returns how many.
    /// (Not generic on purpose: the coverage floor counts a generic body per
    /// instantiation, and the real environment rarely takes every branch.)
    pub fn register_env(&self, vars: Vec<(String, String)>) -> usize {
        let mut n = 0;
        for (name, value) in vars {
            let named = self.env_name.as_ref().is_some_and(|re| re.is_match(&name));
            if named && looks_like_env_secret(&value) {
                self.register(value.trim());
                n += 1;
            }
        }
        n
    }

    /// Register one secret; returns its placeholder (idempotent).
    pub fn register(&self, secret: &str) -> String {
        let mut state = recover(&self.state);
        if let Some(p) = state.by_secret.get(secret) {
            return p.clone();
        }
        let placeholder = self.placeholder_for(secret);
        state
            .by_placeholder
            .insert(placeholder.clone(), secret.to_string());
        state
            .by_secret
            .insert(secret.to_string(), placeholder.clone());
        placeholder
    }

    /// Number of registered secrets.
    pub fn len(&self) -> usize {
        recover(&self.state).by_secret.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn placeholder_for(&self, secret: &str) -> String {
        let tag = hmac_sha256(&self.key, secret.as_bytes());
        format!(
            "{PLACEHOLDER_PREFIX}{}{PLACEHOLDER_SUFFIX}",
            &hex(&tag)[..16]
        )
    }

    /// `Some(redacted)` when the text changed, `None` when it has no secret.
    pub fn redact_text(&self, text: &str) -> Option<String> {
        let mut out = self.replace_known(text);
        for pattern in &self.patterns {
            let current = out.as_deref().unwrap_or(text);
            if let Some(next) = self.replace_pattern(pattern, current) {
                out = Some(next);
            }
        }
        out
    }

    fn replace_known(&self, text: &str) -> Option<String> {
        let mut pairs: Vec<(String, String)> = {
            let state = recover(&self.state);
            state
                .by_secret
                .iter()
                .filter(|(secret, _)| text.contains(secret.as_str()))
                .map(|(s, p)| (s.clone(), p.clone()))
                .collect()
        };
        if pairs.is_empty() {
            return None;
        }
        // Longest first, so a secret that contains another is replaced whole.
        pairs.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(&b.0)));
        let mut out = text.to_string();
        for (secret, placeholder) in pairs {
            out = out.replace(&secret, &placeholder);
        }
        Some(out)
    }

    fn replace_pattern(&self, pattern: &Regex, text: &str) -> Option<String> {
        let mut out = String::new();
        let mut last = 0;
        let mut changed = false;
        // The last present group is the secret: group 1 when the pattern
        // names one, else the whole match (group 0 always exists).
        for hit in pattern
            .captures_iter(text)
            .filter_map(|caps| caps.iter().flatten().last())
        {
            let secret = hit.as_str();
            // Never redact (part of) a placeholder: the `KEY=value` shape
            // would otherwise match the `SECRET:<hex>` inside one.
            if secret.starts_with(PLACEHOLDER_PREFIX)
                || text[..hit.start()].ends_with(PLACEHOLDER_PREFIX)
            {
                continue;
            }
            out.push_str(&text[last..hit.start()]);
            out.push_str(&self.register(secret));
            last = hit.end();
            changed = true;
        }
        if !changed {
            return None;
        }
        out.push_str(&text[last..]);
        Some(out)
    }

    /// Redact every string in a JSON value; `Some` when something changed.
    pub fn redact_json(&self, value: &serde_json::Value) -> Option<serde_json::Value> {
        let mut copy = value.clone();
        self.map_strings(&mut copy, &|s| self.redact_text(s))
            .then_some(copy)
    }

    /// Redact the system prompt and every message. Messages are shared
    /// behind an `Arc`; a new slice is built only when something changed.
    pub fn redact_request(&self, request: &LlmRequest) -> LlmRequest {
        let mut out = request.clone();
        if let Some(system) = self.redact_text(&request.system) {
            out.system = system;
        }
        let mut changed = false;
        let messages: Vec<Message> = request
            .messages
            .iter()
            .map(|m| match self.redact_message(m) {
                Some(redacted) => {
                    changed = true;
                    redacted
                }
                None => m.clone(),
            })
            .collect();
        if changed {
            out.messages = messages.into();
        }
        out
    }

    fn redact_message(&self, message: &Message) -> Option<Message> {
        let content = match &message.content {
            MessageContent::Text(text) => MessageContent::Text(self.redact_text(text)?),
            MessageContent::Blocks(blocks) => {
                let mut changed = false;
                let blocks: Vec<ContentBlock> = blocks
                    .iter()
                    .map(|b| match self.redact_block(b) {
                        Some(redacted) => {
                            changed = true;
                            redacted
                        }
                        None => b.clone(),
                    })
                    .collect();
                if !changed {
                    return None;
                }
                MessageContent::Blocks(blocks)
            }
        };
        Some(Message {
            content,
            ..message.clone()
        })
    }

    /// Thinking blocks are left alone: their signature covers the text, and
    /// the model only wrote what it had already been shown.
    fn redact_block(&self, block: &ContentBlock) -> Option<ContentBlock> {
        match block {
            ContentBlock::Text { text } => Some(ContentBlock::Text {
                text: self.redact_text(text)?,
            }),
            ContentBlock::ToolUse { id, name, input } => Some(ContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: self.redact_json(input)?,
            }),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some(ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: self.redact_text(content)?,
                is_error: *is_error,
            }),
            _ => None,
        }
    }

    /// `Some(restored)` when a known placeholder was replaced by its secret.
    pub fn restore_text(&self, text: &str) -> Option<String> {
        if !text.contains(PLACEHOLDER_PREFIX) {
            return None;
        }
        let state = recover(&self.state);
        let mut out = text.to_string();
        let mut changed = false;
        for (placeholder, secret) in &state.by_placeholder {
            if out.contains(placeholder.as_str()) {
                out = out.replace(placeholder.as_str(), secret);
                changed = true;
            }
        }
        changed.then_some(out)
    }

    /// Restore placeholders in every string of a JSON value, in place.
    pub fn restore_json(&self, value: &mut serde_json::Value) -> bool {
        self.map_strings(value, &|s| self.restore_text(s))
    }

    fn map_strings(
        &self,
        value: &mut serde_json::Value,
        f: &dyn Fn(&str) -> Option<String>,
    ) -> bool {
        match value {
            serde_json::Value::String(s) => match f(s) {
                Some(next) => {
                    *s = next;
                    true
                }
                None => false,
            },
            serde_json::Value::Array(items) => {
                let mut changed = false;
                for item in items {
                    changed |= self.map_strings(item, f);
                }
                changed
            }
            serde_json::Value::Object(map) => {
                let mut changed = false;
                for item in map.values_mut() {
                    changed |= self.map_strings(item, f);
                }
                changed
            }
            _ => false,
        }
    }
}

/// Long enough, one token, and not a path.
fn looks_like_env_secret(value: &str) -> bool {
    let v = value.trim();
    v.len() >= MIN_ENV_SECRET_LEN
        && !v.chars().any(char::is_whitespace)
        && !v.starts_with('/')
        && !v.starts_with('~')
        && !v.starts_with('.')
        && !(v.len() > 2 && v.as_bytes()[1] == b':' && v.as_bytes()[0].is_ascii_alphabetic())
}

fn parse_key_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(key)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hmac_sha256(key: &[u8; 32], message: &[u8]) -> [u8; 32] {
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for (i, k) in key.iter().enumerate() {
        ipad[i] ^= k;
        opad[i] ^= k;
    }
    let inner = Sha256::new()
        .chain_update(ipad)
        .chain_update(message)
        .finalize();
    let outer = Sha256::new()
        .chain_update(opad)
        .chain_update(inner)
        .finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer);
    out
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn recover(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Compile the built-in patterns; one that does not compile is logged and
/// skipped rather than taking redaction down with it.
fn compile_patterns(patterns: &[&str]) -> Vec<Regex> {
    let mut out = Vec::with_capacity(patterns.len());
    for pattern in patterns {
        match Regex::new(pattern) {
            Ok(re) => out.push(re),
            Err(err) => tracing::error!(pattern, %err, "secret pattern does not compile"),
        }
    }
    out
}

#[cfg(test)]
#[path = "secrets_tests.rs"]
mod tests;
