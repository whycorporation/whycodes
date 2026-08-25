//! Credential discovery and consented import from other CLIs.
//!
//! The machine very likely already holds working credentials for Claude
//! Code, Codex CLI, Gemini CLI, GitHub Copilot or Grok Build. This module locates those
//! files, but **never reads one until the user approves that exact path**,
//! and never modifies one either (no move, rewrite, or permission change;
//! symlinked sources are refused outright — a planted link could point at
//! an arbitrary file). Approvals are persisted per canonical path in
//! `<data_dir>/auth-consent.json` (0600), so the prompt appears once.
//!
//! Imported credentials are stored as method `"imported"`; refresh and the
//! 401 re-auth path treat them like any OAuth login. The provider terms
//! caveat in `docs/auth.md` applies to using imported credentials.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AuthError, Result};
use crate::store::TokenStore;
use crate::token::{OAuthToken, ProviderAuth};

/// A credential file another CLI is known to write, plus its parser.
#[derive(Debug)]
pub struct KnownSource {
    /// whycodes provider the credential maps to.
    pub provider: &'static str,
    /// Owning tool, for display ("Claude Code").
    pub label: &'static str,
    /// Path relative to the user's home directory.
    pub rel_path: &'static str,
    /// Foreign JSON → `OAuthToken`. Runs only after consent.
    parse: fn(&Value) -> Result<OAuthToken>,
}

/// Every credential file discovery knows about. macOS Keychain entries
/// (Claude Code's home on that platform) are out of scope — reading the
/// login Keychain needs user interaction of a different kind.
pub const KNOWN_SOURCES: &[KnownSource] = &[
    KnownSource {
        provider: "anthropic",
        label: "Claude Code",
        rel_path: ".claude/.credentials.json",
        parse: parse_claude_code,
    },
    KnownSource {
        provider: "openai",
        label: "Codex CLI",
        rel_path: ".codex/auth.json",
        parse: parse_codex,
    },
    KnownSource {
        provider: "google",
        label: "Gemini CLI",
        rel_path: ".gemini/oauth_creds.json",
        parse: parse_gemini,
    },
    KnownSource {
        provider: "github-copilot",
        label: "GitHub Copilot",
        rel_path: ".config/github-copilot/hosts.json",
        parse: parse_copilot,
    },
    KnownSource {
        provider: "xai",
        label: "Grok Build",
        rel_path: ".grok/auth.json",
        parse: parse_grok_build,
    },
];

/// Consent state of one discovered file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceState {
    /// Present; never approved nor denied — contents not yet read.
    New,
    /// The user approved this exact path; it may be read and imported.
    Approved,
    /// The user declined this exact path; leave it alone.
    Denied,
    /// The path is a symlink — refused regardless of consent.
    Symlink,
}

/// One located credential file.
#[derive(Debug)]
pub struct FoundSource {
    pub source: &'static KnownSource,
    pub path: PathBuf,
    pub state: SourceState,
}

/// The user's home directory, without pulling in a path crate.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()))
        .map(PathBuf::from)
}

/// Persisted per-path consent decisions (`<data_dir>/auth-consent.json`).
/// Written with the same owner-only rules as the token store.
pub struct ConsentStore {
    path: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConsentFile {
    #[serde(default)]
    approved: BTreeSet<String>,
    #[serde(default)]
    denied: BTreeSet<String>,
}

impl ConsentStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join("auth-consent.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Result<ConsentFile> {
        match std::fs::read_to_string(&self.path) {
            Ok(content) => Ok(serde_json::from_str(&content)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ConsentFile::default()),
            Err(e) => Err(AuthError::Io(e)),
        }
    }

    fn write(&self, file: &ConsentFile) -> Result<()> {
        let parent = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(file)?)?;
        crate::store::set_owner_only(&tmp)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// `Some(true)` approved, `Some(false)` denied, `None` never asked.
    pub fn decision(&self, path: &Path) -> Option<bool> {
        let key = path.display().to_string();
        let file = self.read().ok()?;
        if file.approved.contains(&key) {
            Some(true)
        } else if file.denied.contains(&key) {
            Some(false)
        } else {
            None
        }
    }

    /// Persist the decision for `path`; re-asking flips the set.
    pub fn record(&self, path: &Path, approved: bool) -> Result<()> {
        let key = path.display().to_string();
        let mut file = self.read()?;
        file.approved.remove(&key);
        file.denied.remove(&key);
        if approved {
            file.approved.insert(key);
        } else {
            file.denied.insert(key);
        }
        self.write(&file)
    }
}

/// Locate known credential files under `home` without reading any of
/// them. Existence and symlink checks only — contents stay untouched until
/// [`import`] is called on an approved path.
pub fn scan_with_home(home: &Path, consent: &ConsentStore) -> Vec<FoundSource> {
    let mut found = Vec::new();
    for source in KNOWN_SOURCES {
        let path = home.join(source.rel_path);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue; // not present
        };
        if meta.file_type().is_symlink() {
            found.push(FoundSource {
                source,
                path,
                state: SourceState::Symlink,
            });
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        let state = match consent.decision(&path) {
            Some(true) => SourceState::Approved,
            Some(false) => SourceState::Denied,
            None => SourceState::New,
        };
        found.push(FoundSource {
            source,
            path,
            state,
        });
    }
    found
}

/// [`scan_with_home`] for the real user home directory.
pub fn scan(consent: &ConsentStore) -> Vec<FoundSource> {
    scan_from_home(home_dir(), consent)
}

fn scan_from_home(home: Option<PathBuf>, consent: &ConsentStore) -> Vec<FoundSource> {
    match home {
        Some(home) => scan_with_home(&home, consent),
        None => Vec::new(),
    }
}

/// Import an approved, non-symlink source into the token store.
///
/// Hard gates, in order: symlinked sources are always refused; a source
/// whose path has no recorded approval is refused (the CLI collects
/// consent first and records it with [`ConsentStore::record`]). The source
/// file is only ever *read* — nothing here writes, moves, or re-permits
/// it; the acceptance test pins mtime+content stability.
pub fn import(store: &TokenStore, consent: &ConsentStore, found: &FoundSource) -> Result<()> {
    if matches!(found.state, SourceState::Symlink) {
        return Err(AuthError::SymlinkRejected(found.path.display().to_string()));
    }
    if consent.decision(&found.path) != Some(true) {
        return Err(AuthError::ConsentRequired(found.path.display().to_string()));
    }
    let content = std::fs::read_to_string(&found.path).map_err(AuthError::Io)?;
    let json: Value = serde_json::from_str(&content)?;
    let mut token = (found.source.parse)(&json)?;
    // Traceability for `auth status` / debug: where this credential came
    // from. Not secret material.
    token.extra.insert(
        "imported_from".to_string(),
        Value::String(format!("{} ({})", found.source.label, found.path.display())),
    );
    store.set(
        found.source.provider,
        ProviderAuth {
            method: "imported".to_string(),
            token,
        },
    )
}

// ────────────────────────────────────────────────────────────────────────
// Foreign credential formats (parsers are pure; no I/O, easy to test)
// ────────────────────────────────────────────────────────────────────────

fn ms_to_datetime(ms: i64) -> Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::from_timestamp_millis(ms).ok_or_else(|| {
        AuthError::TokenExchange(format!("credential expiry {ms}ms is out of range"))
    })
}

fn need<'a>(json: &'a Value, pointer: &str) -> Result<&'a str> {
    json.pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| AuthError::TokenExchange(format!("credential file is missing {pointer}")))
}

/// Claude Code `~/.claude/.credentials.json`:
/// `{"claudeAiOauth":{"accessToken","refreshToken","expiresAt":<ms>,…}}`.
fn parse_claude_code(json: &Value) -> Result<OAuthToken> {
    let access = need(json, "/claudeAiOauth/accessToken")?.to_string();
    let refresh = json
        .pointer("/claudeAiOauth/refreshToken")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let expires_at = json
        .pointer("/claudeAiOauth/expiresAt")
        .and_then(Value::as_i64)
        .map(ms_to_datetime)
        .transpose()?;
    Ok(OAuthToken {
        access_token: access,
        refresh_token: refresh,
        expires_at,
        extra: Default::default(),
    })
}

/// Codex CLI `~/.codex/auth.json`:
/// `{"tokens":{"id_token","access_token","refresh_token","account_id"?}}`.
/// The account id feeds the `chatgpt-account-id` header on the Codex
/// backend route; fall back to decoding the id_token claim like login does.
fn parse_codex(json: &Value) -> Result<OAuthToken> {
    let access = need(json, "/tokens/access_token")?.to_string();
    let refresh = json
        .pointer("/tokens/refresh_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let id_token = json.pointer("/tokens/id_token").and_then(Value::as_str);
    let mut extra = serde_json::Map::new();
    let account = json
        .pointer("/tokens/account_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| id_token.and_then(crate::providers::openai_account_id_from_jwt));
    if let Some(account) = account {
        extra.insert("openai_account_id".to_string(), Value::String(account));
    }
    Ok(OAuthToken {
        access_token: access,
        refresh_token: refresh,
        expires_at: None, // Codex auth.json carries no expiry; refresh on 401
        extra,
    })
}

/// Gemini CLI `~/.gemini/oauth_creds.json` (google-auth library shape):
/// `{"access_token","refresh_token","token_type","expiry_date":<ms>,…}`.
fn parse_gemini(json: &Value) -> Result<OAuthToken> {
    let access = need(json, "/access_token")?.to_string();
    let refresh = json["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let expires_at = json["expiry_date"]
        .as_i64()
        .map(ms_to_datetime)
        .transpose()?;
    Ok(OAuthToken {
        access_token: access,
        refresh_token: refresh,
        expires_at,
        extra: Default::default(),
    })
}

/// Grok Build `~/.grok/auth.json`: a map of scope keys to credential
/// objects. The OAuth2 slot is `https://auth.x.ai::<client_id>` with
/// `key` (access token), `refresh_token`, `expires_at` (RFC 3339).
/// API-key slots (`xai::api_key` / `auth_mode: api_key`) are skipped.
fn parse_grok_build(json: &Value) -> Result<OAuthToken> {
    let obj = json.as_object().ok_or_else(|| {
        AuthError::TokenExchange("Grok Build auth.json is not an object".to_string())
    })?;
    let mut fallback: Option<&Value> = None;
    let mut preferred: Option<&Value> = None;
    for (k, v) in obj {
        if k == "xai::api_key" || k.contains("::api_key") {
            continue;
        }
        if v.get("auth_mode").and_then(Value::as_str) == Some("api_key") {
            continue;
        }
        let Some(access) = v.get("key").and_then(Value::as_str) else {
            continue;
        };
        if access.is_empty() {
            continue;
        }
        if k.contains("auth.x.ai") {
            preferred = Some(v);
            break;
        }
        if fallback.is_none() {
            fallback = Some(v);
        }
    }
    let entry = preferred.or(fallback).ok_or_else(|| {
        AuthError::TokenExchange("Grok Build auth.json has no OAuth credential".to_string())
    })?;
    let access = need(entry, "/key")?.to_string();
    let refresh = entry
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let expires_at = entry
        .get("expires_at")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));
    Ok(OAuthToken {
        access_token: access,
        refresh_token: refresh,
        expires_at,
        extra: Default::default(),
    })
}

/// GitHub Copilot `~/.config/github-copilot/hosts.json`:
/// `{"github.com":{"oauth_token":"gho_…","user":…}}`. No expiry; the
/// derived Copilot API token is exchanged on first use.
fn parse_copilot(json: &Value) -> Result<OAuthToken> {
    let access = need(json, "/github.com/oauth_token")?.to_string();
    Ok(OAuthToken {
        access_token: access,
        refresh_token: None,
        expires_at: None,
        extra: Default::default(),
    })
}

#[cfg(test)]
#[path = "discover_tests.rs"]
mod tests;
