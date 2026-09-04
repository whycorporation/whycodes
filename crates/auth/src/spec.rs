//! Owned OAuth provider specification.
//!
//! Built-in WhyCodes has an empty registry. Subscription login is added by
//! installing an auth plugin (`plugin.json` with `kind: "auth"`) into the
//! user or project plugins directory. The OAuth engine in `providers` stays
//! generic; client ids and identity headers live in the plugin file.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::error::{AuthError, Result};

/// Browser / device grant used by [`crate::providers`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlowKind {
    /// Browser → provider redirects to a loopback listener we host.
    LoopbackPkce,
    /// Browser shows `code#state` on a provider page; the user pastes it
    /// back (used when the public client's registered redirect is fixed and
    /// is not a loopback address).
    PasteCodePkce,
    /// GitHub device-code grant (user enters a short code on github.com).
    DeviceCode,
}

/// How the token endpoint wants grant payloads encoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenEncoding {
    /// `application/x-www-form-urlencoded` — the RFC 6749 standard.
    #[default]
    Form,
    /// `application/json` — Anthropic's token endpoint.
    Json,
}

/// A provider whose API credential is *derived* from the OAuth token by a
/// second exchange (GitHub OAuth token → short-lived Copilot API token).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedCredential {
    /// Exchange endpoint, called as GET with the OAuth token.
    pub url: String,
    /// Authorization header scheme for the exchange: "token" (GitHub) or
    /// "Bearer".
    pub auth_scheme: String,
    /// Extra request headers (client gating).
    pub headers: Vec<(String, String)>,
}

/// Description of one provider's OAuth endpoints and optional inference
/// identity. Adding a provider is installing a plugin JSON, not a core
/// match arm.
#[derive(Clone, Debug)]
pub struct ProviderSpec {
    pub name: String,
    pub label: String,
    pub flow: FlowKind,
    pub client_id: String,
    /// Installed-app client secret where the provider requires one (Google).
    pub client_secret: Option<String>,
    pub authorize_url: String,
    pub token_url: String,
    pub scopes: String,
    /// Grant encoding for `token_url`.
    pub token_encoding: TokenEncoding,
    /// Fixed redirect for flows whose client has one registered
    /// (`PasteCodePkce`). Loopback flows construct theirs from the bound
    /// port, so this stays `None` there.
    pub redirect_uri: Option<String>,
    /// Fixed loopback port when the registered redirect demands one
    /// (OpenAI). `None` → bind an ephemeral port.
    pub loopback_port: Option<u16>,
    /// Host used in the loopback redirect URI. `None` → `localhost`.
    pub loopback_host: Option<String>,
    /// Path the loopback listener answers on.
    pub callback_path: String,
    /// Extra authorize-url query pairs (provider-specific switches).
    pub extra_authorize: Vec<(String, String)>,
    /// Set when the API credential is derived from the OAuth token by a
    /// second exchange instead of being the access token itself.
    pub derived: Option<DerivedCredential>,
    /// Models worth offering in pickers for a subscription login.
    pub suggested_models: Vec<String>,
    /// Optional inference-time identity (User-Agent, originator, …).
    pub inference: Option<InferenceIdentity>,
}

/// How LLM HTTP calls should identify when this plugin's OAuth token is used.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceIdentity {
    /// Override `User-Agent`. Empty / omitted → WhyCodes identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// Extra request headers (e.g. `originator`, `X-XAI-Token-Auth`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
}

fn registry() -> &'static Mutex<HashMap<String, ProviderSpec>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, ProviderSpec>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn lock_registry() -> std::sync::MutexGuard<'static, HashMap<String, ProviderSpec>> {
    match registry().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Install or replace a provider spec. Last write for a name wins.
pub fn register_spec(spec: ProviderSpec) {
    let name = spec.name.clone();
    lock_registry().insert(name, spec);
}

/// Drop every registered OAuth spec. Used by tests; production only
/// registers, never clears.
pub fn clear_registry() {
    lock_registry().clear();
}

/// Names currently registered, sorted for stable CLI / TUI lists.
pub fn registered_providers() -> Vec<String> {
    let mut names: Vec<String> = lock_registry().keys().cloned().collect();
    names.sort();
    names
}

/// Look up a cloned spec. `None` when no plugin registered that provider.
pub fn spec_get(provider: &str) -> Option<ProviderSpec> {
    lock_registry().get(provider).cloned()
}

/// Look up the OAuth spec for a provider name.
pub fn spec_for(provider: &str) -> Result<ProviderSpec> {
    spec_get(provider).ok_or_else(|| AuthError::UnsupportedProvider(provider.to_string()))
}

/// True when `provider` has an OAuth plugin loaded.
pub fn supports_oauth(provider: &str) -> bool {
    lock_registry().contains_key(provider)
}

/// Models worth offering in pickers for a subscription (OAuth) login.
pub fn suggested_models(provider: &str) -> Vec<String> {
    spec_get(provider)
        .map(|s| s.suggested_models)
        .unwrap_or_default()
}

/// Inference identity from a loaded plugin, if any.
pub fn inference_identity(provider: &str) -> Option<InferenceIdentity> {
    spec_get(provider).and_then(|s| s.inference)
}

/// Validate a provider spec against the invariants the flow code relies on.
/// Returns the list of violations (empty = valid).
pub fn validate(spec: &ProviderSpec) -> Vec<String> {
    fn check(issues: &mut Vec<String>, cond: bool, msg: impl Into<String>) {
        if !cond {
            issues.push(msg.into());
        }
    }

    let mut issues: Vec<String> = Vec::new();

    check(&mut issues, !spec.name.is_empty(), "name is empty");
    check(
        &mut issues,
        !spec.label.is_empty(),
        format!("{}: label is empty", spec.name),
    );
    check(
        &mut issues,
        !spec.client_id.is_empty() && !spec.client_id.contains(char::is_whitespace),
        format!("{}: client_id is empty or contains whitespace", spec.name),
    );
    for (field, url) in [
        ("authorize_url", spec.authorize_url.as_str()),
        ("token_url", spec.token_url.as_str()),
    ] {
        let ok = matches!(url::Url::parse(url), Ok(u) if u.scheme() == "https");
        check(
            &mut issues,
            ok,
            format!("{}: {field} must be an absolute https URL", spec.name),
        );
    }
    check(
        &mut issues,
        !spec.scopes.trim().is_empty(),
        format!("{}: scopes are empty", spec.name),
    );
    if let Some(secret) = spec.client_secret.as_deref() {
        check(
            &mut issues,
            !secret.is_empty(),
            format!("{}: client_secret is Some(\"\")", spec.name),
        );
    }

    match spec.flow {
        FlowKind::LoopbackPkce => {
            check(
                &mut issues,
                spec.callback_path.starts_with('/'),
                format!(
                    "{}: loopback flow needs callback_path starting with '/'",
                    spec.name
                ),
            );
            if let Some(host) = spec.loopback_host.as_deref() {
                check(
                    &mut issues,
                    host == "localhost" || host == "127.0.0.1",
                    format!(
                        "{}: loopback_host must be localhost or 127.0.0.1",
                        spec.name
                    ),
                );
            }
            check(
                &mut issues,
                spec.redirect_uri.is_none(),
                format!(
                    "{}: loopback flow constructs redirect_uri; set redirect_uri = None",
                    spec.name
                ),
            );
        }
        FlowKind::PasteCodePkce => {
            let ok = match spec.redirect_uri.as_deref() {
                Some(uri) => matches!(url::Url::parse(uri), Ok(u) if u.scheme() == "https"),
                None => false,
            };
            check(
                &mut issues,
                ok,
                format!(
                    "{}: paste-code flow needs a registered https redirect_uri",
                    spec.name
                ),
            );
            check(
                &mut issues,
                spec.loopback_host.is_none(),
                format!("{}: loopback_host is only for LoopbackPkce", spec.name),
            );
            check(
                &mut issues,
                spec.loopback_port.is_none() && spec.callback_path.is_empty(),
                format!(
                    "{}: paste-code flow has no loopback listener; leave loopback_port None and callback_path empty",
                    spec.name
                ),
            );
        }
        FlowKind::DeviceCode => {
            check(
                &mut issues,
                spec.loopback_host.is_none(),
                format!("{}: loopback_host is only for LoopbackPkce", spec.name),
            );
            check(
                &mut issues,
                spec.redirect_uri.is_none() && spec.callback_path.is_empty(),
                format!(
                    "{}: device flow has no redirect; leave redirect_uri None and callback_path empty",
                    spec.name
                ),
            );
        }
    }

    for (i, (k, v)) in spec.extra_authorize.iter().enumerate() {
        check(
            &mut issues,
            !k.is_empty() && !v.is_empty(),
            format!(
                "{}: extra_authorize[{i}] has an empty key or value",
                spec.name
            ),
        );
        check(
            &mut issues,
            spec.extra_authorize
                .iter()
                .filter(|(ok, _)| ok == k)
                .count()
                == 1,
            format!("{}: extra_authorize has duplicate key `{k}`", spec.name),
        );
    }

    if let Some(d) = spec.derived.as_ref() {
        let ok = matches!(url::Url::parse(&d.url), Ok(u) if u.scheme() == "https");
        check(
            &mut issues,
            ok,
            format!("{}: derived.url must be an absolute https URL", spec.name),
        );
        check(
            &mut issues,
            matches!(d.auth_scheme.as_str(), "token" | "Bearer"),
            format!(
                "{}: derived.auth_scheme must be \"token\" or \"Bearer\"",
                spec.name
            ),
        );
        for (k, v) in &d.headers {
            check(
                &mut issues,
                !k.is_empty() && !v.is_empty(),
                format!("{}: derived.headers has an empty key or value", spec.name),
            );
        }
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_name_is_unsupported() {
        assert!(!supports_oauth("definitely-missing-oauth-provider"));
        assert!(spec_for("definitely-missing-oauth-provider").is_err());
        assert!(suggested_models("definitely-missing-oauth-provider").is_empty());
        assert!(inference_identity("definitely-missing-oauth-provider").is_none());
    }

    #[test]
    fn register_then_lookup() {
        register_spec(ProviderSpec {
            name: "spec-register-demo".into(),
            label: "Demo".into(),
            flow: FlowKind::DeviceCode,
            client_id: "cid".into(),
            client_secret: None,
            authorize_url: "https://example.com/auth".into(),
            token_url: "https://example.com/token".into(),
            scopes: "read".into(),
            token_encoding: TokenEncoding::Form,
            redirect_uri: None,
            loopback_port: None,
            loopback_host: None,
            callback_path: String::new(),
            extra_authorize: vec![],
            derived: None,
            suggested_models: vec!["m1".into()],
            inference: None,
        });
        assert!(supports_oauth("spec-register-demo"));
        assert!(
            registered_providers()
                .iter()
                .any(|n| n == "spec-register-demo")
        );
        assert_eq!(
            suggested_models("spec-register-demo"),
            vec!["m1".to_string()]
        );
        // Drop only this spec so parallel tests keep extras plugins.
        lock_registry().remove("spec-register-demo");
    }

    #[test]
    fn clear_registry_is_restored_from_saved_specs() {
        let saved: Vec<ProviderSpec> = lock_registry().values().cloned().collect();
        register_spec(ProviderSpec {
            name: "spec-clear-demo".into(),
            label: "Demo".into(),
            flow: FlowKind::DeviceCode,
            client_id: "cid".into(),
            client_secret: None,
            authorize_url: "https://example.com/auth".into(),
            token_url: "https://example.com/token".into(),
            scopes: "read".into(),
            token_encoding: TokenEncoding::Form,
            redirect_uri: None,
            loopback_port: None,
            loopback_host: None,
            callback_path: String::new(),
            extra_authorize: vec![],
            derived: None,
            suggested_models: vec![],
            inference: None,
        });
        clear_registry();
        assert!(!supports_oauth("spec-clear-demo"));
        for spec in saved {
            register_spec(spec);
        }
    }

    #[test]
    fn lock_registry_recovers_from_poison() {
        let _saved: Vec<ProviderSpec> = lock_registry().values().cloned().collect();
        let handle = std::thread::spawn(|| {
            let _guard = lock_registry();
            panic!("poison the auth spec registry");
        });
        let _ = handle.join();
        let _guard = lock_registry();
        drop(_guard);
        for spec in _saved {
            register_spec(spec);
        }
    }
}
