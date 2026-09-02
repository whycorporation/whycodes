//! Value completions for `--provider`, `--model`, `auth login`, session ids.
//!
//! Invoked from clap `possible_values` while generating scripts (`whycodes
//! completions`) and from shells that query clap. Must not create `$WHYCODES_HOME`,
//! rewrite `config.toml`, or start Tokio.

use clap::builder::{PossibleValue, TypedValueParser};
use std::ffi::OsStr;
use std::path::PathBuf;

const BUILTIN_PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "ollama",
    "google",
    "google-antigravity",
    "github-copilot",
    "groq",
    "deepseek",
    "xai",
    "openrouter",
];

#[derive(Clone, Debug, Default)]
pub(crate) struct ProviderValueParser;

#[derive(Clone, Debug, Default)]
pub(crate) struct ModelValueParser;

#[derive(Clone, Debug, Default)]
pub(crate) struct AuthProviderValueParser;

#[derive(Clone, Debug, Default)]
pub(crate) struct SessionIdValueParser;

fn possible_values_from(ids: Vec<String>) -> Box<dyn Iterator<Item = PossibleValue> + 'static> {
    Box::new(ids.into_iter().map(PossibleValue::new))
}

impl TypedValueParser for ProviderValueParser {
    type Value = String;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<String, clap::Error> {
        clap::builder::StringValueParser::new().parse_ref(cmd, arg, value)
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        Some(possible_values_from(provider_ids()))
    }
}

impl TypedValueParser for ModelValueParser {
    type Value = String;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<String, clap::Error> {
        clap::builder::StringValueParser::new().parse_ref(cmd, arg, value)
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        Some(possible_values_from(model_ids()))
    }
}

impl TypedValueParser for AuthProviderValueParser {
    type Value = String;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<String, clap::Error> {
        clap::builder::StringValueParser::new().parse_ref(cmd, arg, value)
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        Some(possible_values_from(auth_provider_ids()))
    }
}

impl TypedValueParser for SessionIdValueParser {
    type Value = String;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<String, clap::Error> {
        clap::builder::StringValueParser::new().parse_ref(cmd, arg, value)
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        Some(possible_values_from(session_id_prefixes()))
    }
}

fn load_config_readonly() -> whycodes_config::Config {
    let Ok(path) = whycodes_config::Config::default_path() else {
        return whycodes_config::Config::default();
    };
    if !path.exists() {
        return whycodes_config::Config::default();
    }
    let Ok(content) = std::fs::read_to_string(&path) else {
        return whycodes_config::Config::default();
    };
    toml::from_str(&content).unwrap_or_default()
}

pub(crate) fn provider_ids() -> Vec<String> {
    let mut names: Vec<String> = BUILTIN_PROVIDERS.iter().map(|s| (*s).to_string()).collect();
    let cfg = load_config_readonly();
    for key in cfg.providers.keys() {
        if !names.iter().any(|n| n == key) {
            names.push(key.clone());
        }
    }
    names.sort();
    names.dedup();
    names
}

pub(crate) fn model_ids() -> Vec<String> {
    let cfg = load_config_readonly();
    let mut names: Vec<String> = cfg.models.keys().cloned().collect();
    for pc in cfg.providers.values() {
        for m in &pc.models {
            if !m.is_empty() {
                names.push(m.clone());
            }
        }
    }
    if let Some(default) = cfg.default_model.as_ref()
        && !default.model_id.is_empty()
    {
        names.push(default.model_id.clone());
    }
    names.sort();
    names.dedup();
    names
}

pub(crate) fn auth_provider_ids() -> Vec<String> {
    // Read-only discovery: `load_from_dirs` skips missing folders.
    let mut dirs = Vec::new();
    if let Some(global) = whycodes_plugin::global_plugins_dir() {
        dirs.push(global);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    dirs.push(whycodes_plugin::project_plugins_dir(&cwd));
    let loaded = whycodes_auth::plugin::load_from_dirs(&dirs);
    if loaded > 0 {
        tracing::debug!(count = loaded, "completion: loaded auth plugins");
    }
    let mut names = whycodes_auth::oauth_providers();
    if names.is_empty() {
        names = provider_ids();
    }
    names.sort();
    names.dedup();
    names
}

pub(crate) fn session_id_prefixes() -> Vec<String> {
    let Ok(data_dir) = whycodes_config::Config::data_dir() else {
        return Vec::new();
    };
    let db_path = data_dir.join("whycodes.db");
    let Ok(Some(db)) =
        whycodes_storage::db::Database::open_existing_readonly(&db_path.to_string_lossy())
    else {
        return Vec::new();
    };
    let Ok(sessions) = db.list_sessions() else {
        return Vec::new();
    };
    sessions
        .into_iter()
        .take(20)
        .map(|s| {
            if s.id.len() > 12 {
                s.id[..12].to_string()
            } else {
                s.id
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_providers_include_openai() {
        let names = provider_ids();
        assert!(names.iter().any(|n| n == "openai"), "{names:?}");
        assert!(names.iter().any(|n| n == "anthropic"), "{names:?}");
    }

    #[test]
    fn model_ids_never_panic_without_config() {
        let _ = model_ids();
    }

    #[test]
    fn session_prefixes_empty_without_db() {
        // Isolated: no assertion on HOME; just must not create files / panic.
        let _ = session_id_prefixes();
    }
}
