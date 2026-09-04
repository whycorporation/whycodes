//! Auth plugins: `plugin.json` with `"kind": "auth"` registers an OAuth spec.
//!
//! WhyCodes does not ship subscription-login plugins in the default install.
//! Drop a plugin directory into the user or project plugins folder (or pass
//! extra dirs to [`load_from_dirs`]) to enable `whycodes auth login`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{AuthError, Result};
use crate::spec::{
    DerivedCredential, FlowKind, InferenceIdentity, ProviderSpec, TokenEncoding, register_spec,
    validate,
};

/// On-disk auth plugin. Shell plugins use the same filename; we only accept
/// objects with `"kind": "auth"` (and an `auth` object).
#[derive(Debug, Deserialize)]
struct AuthPluginFile {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    auth: Option<AuthPluginBody>,
}

#[derive(Debug, Deserialize)]
struct AuthPluginBody {
    provider: String,
    #[serde(default)]
    label: String,
    flow: FlowKind,
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
    authorize_url: String,
    token_url: String,
    scopes: String,
    #[serde(default)]
    token_encoding: TokenEncoding,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    loopback_port: Option<u16>,
    #[serde(default)]
    loopback_host: Option<String>,
    #[serde(default)]
    callback_path: String,
    #[serde(default)]
    extra_authorize: HashMap<String, String>,
    #[serde(default)]
    derived: Option<DerivedFile>,
    #[serde(default)]
    suggested_models: Vec<String>,
    #[serde(default)]
    inference: Option<InferenceIdentity>,
}

#[derive(Debug, Deserialize)]
struct DerivedFile {
    url: String,
    #[serde(default = "default_auth_scheme")]
    auth_scheme: String,
    #[serde(default)]
    headers: HashMap<String, String>,
}

fn default_auth_scheme() -> String {
    "Bearer".into()
}

/// Parse one plugin JSON document into a spec. `kind` must be `auth`.
pub fn spec_from_json(text: &str) -> Result<ProviderSpec> {
    let file: AuthPluginFile = serde_json::from_str(text)
        .map_err(|e| AuthError::TokenExchange(format!("auth plugin: {e}")))?;
    if !file.kind.is_empty() && file.kind != "auth" {
        return Err(AuthError::TokenExchange(format!(
            "auth plugin: kind {:?} is not \"auth\"",
            file.kind
        )));
    }
    let body = file
        .auth
        .ok_or_else(|| AuthError::TokenExchange("auth plugin: missing \"auth\" object".into()))?;
    let extra_authorize: Vec<(String, String)> = body.extra_authorize.into_iter().collect();
    let derived = body.derived.map(|d| DerivedCredential {
        url: d.url,
        auth_scheme: d.auth_scheme,
        headers: d.headers.into_iter().collect(),
    });
    let label = if body.label.is_empty() {
        if file.name.is_empty() {
            body.provider.clone()
        } else {
            file.name
        }
    } else {
        body.label
    };
    Ok(ProviderSpec {
        name: body.provider,
        label,
        flow: body.flow,
        client_id: body.client_id,
        client_secret: body.client_secret.filter(|s| !s.is_empty()),
        authorize_url: body.authorize_url,
        token_url: body.token_url,
        scopes: body.scopes,
        token_encoding: body.token_encoding,
        redirect_uri: body.redirect_uri.filter(|s| !s.is_empty()),
        loopback_port: body.loopback_port,
        loopback_host: body.loopback_host.filter(|s| !s.is_empty()),
        callback_path: body.callback_path,
        extra_authorize,
        derived,
        suggested_models: body.suggested_models,
        inference: body.inference,
    })
}

/// Parse, validate, and register one plugin JSON document.
pub fn register_from_json(text: &str) -> Result<String> {
    let spec = spec_from_json(text)?;
    let issues = validate(&spec);
    if !issues.is_empty() {
        return Err(AuthError::TokenExchange(format!(
            "auth plugin `{}` invalid: {}",
            spec.name,
            issues.join("; ")
        )));
    }
    let name = spec.name.clone();
    register_spec(spec);
    Ok(name)
}

/// Load every `plugin.json` / `manifest.json` under `dir` that looks like an
/// auth plugin. Shell plugins (no `kind: auth`) are skipped. Returns how
/// many specs were registered.
pub fn load_dir(dir: &Path) -> usize {
    let mut n = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            n += load_dir(&path);
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name != "plugin.json" && name != "manifest.json" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !looks_like_auth_plugin(&text) {
            continue;
        }
        match register_from_json(&text) {
            Ok(provider) => {
                tracing::info!(
                    path = %path.display(),
                    provider,
                    "registered auth plugin"
                );
                n += 1;
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "auth plugin rejected"
                );
            }
        }
    }
    n
}

fn looks_like_auth_plugin(text: &str) -> bool {
    text.contains("\"kind\"") && text.contains("auth")
}

/// Load auth plugins from each existing directory. Later dirs override.
pub fn load_from_dirs(dirs: &[PathBuf]) -> usize {
    dirs.iter().map(|d| load_dir(d)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::supports_oauth;

    #[test]
    fn rejects_shell_kind() {
        let err = spec_from_json(r#"{"kind":"shell","name":"x","command":"echo"}"#).unwrap_err();
        assert!(err.to_string().contains("kind"), "{err}");
    }

    #[test]
    fn rejects_missing_auth_object() {
        let err = spec_from_json(r#"{"kind":"auth","name":"x"}"#).unwrap_err();
        assert!(err.to_string().contains("missing"), "{err}");
    }

    #[test]
    fn parses_device_flow() {
        let json = r#"{
            "kind": "auth",
            "auth": {
                "provider": "plugin-demo-device",
                "label": "Demo",
                "flow": "device-code",
                "client_id": "abc",
                "authorize_url": "https://example.com/device/code",
                "token_url": "https://example.com/token",
                "scopes": "read",
                "suggested_models": ["m1"]
            }
        }"#;
        let name = register_from_json(json).unwrap();
        assert_eq!(name, "plugin-demo-device");
        assert!(supports_oauth("plugin-demo-device"));
    }

    #[test]
    fn load_dir_registers_auth_and_skips_shell() {
        let tmp = tempfile::tempdir().unwrap();
        let auth = tmp.path().join("auth-plug");
        let shell = tmp.path().join("shell-plug");
        std::fs::create_dir_all(&auth).unwrap();
        std::fs::create_dir_all(&shell).unwrap();
        std::fs::write(
            auth.join("plugin.json"),
            r#"{
                "kind": "auth",
                "auth": {
                    "provider": "fixture-from-dir",
                    "label": "FromDir",
                    "flow": "device-code",
                    "client_id": "abc",
                    "authorize_url": "https://example.com/device/code",
                    "token_url": "https://example.com/token",
                    "scopes": "read"
                }
            }"#,
        )
        .unwrap();
        std::fs::write(
            shell.join("plugin.json"),
            r#"{"name":"hello","command":"echo hi"}"#,
        )
        .unwrap();
        assert_eq!(load_dir(tmp.path()), 1);
        assert!(supports_oauth("fixture-from-dir"));
    }

    fn valid_device_json(provider: &str, label: &str) -> String {
        format!(
            r#"{{
                "kind": "auth",
                "name": "PluginName",
                "auth": {{
                    "provider": "{provider}",
                    "label": "{label}",
                    "flow": "device-code",
                    "client_id": "abc",
                    "authorize_url": "https://example.com/device/code",
                    "token_url": "https://example.com/token",
                    "scopes": "read"
                }}
            }}"#
        )
    }

    #[test]
    fn parses_label_fallbacks_and_derived_defaults() {
        let from_file_name = spec_from_json(
            r#"{
                "kind": "auth",
                "name": "FileName",
                "auth": {
                    "provider": "plugin-label-file",
                    "flow": "device-code",
                    "client_id": "abc",
                    "authorize_url": "https://example.com/device/code",
                    "token_url": "https://example.com/token",
                    "scopes": "read",
                    "derived": {
                        "url": "https://example.com/derived"
                    }
                }
            }"#,
        )
        .unwrap();
        assert_eq!(from_file_name.label, "FileName");
        let derived = from_file_name.derived.expect("derived");
        assert_eq!(derived.auth_scheme, "Bearer");
        assert_eq!(default_auth_scheme(), "Bearer");

        let from_provider = spec_from_json(
            r#"{
                "auth": {
                    "provider": "plugin-label-provider",
                    "flow": "device-code",
                    "client_id": "abc",
                    "authorize_url": "https://example.com/device/code",
                    "token_url": "https://example.com/token",
                    "scopes": "read"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(from_provider.label, "plugin-label-provider");
    }

    #[test]
    fn parses_optional_fields_and_empty_filters() {
        let spec = spec_from_json(
            r#"{
                "kind": "auth",
                "auth": {
                    "provider": "plugin-optional",
                    "label": "Optional",
                    "flow": "loopback-pkce",
                    "client_id": "cid",
                    "client_secret": "",
                    "authorize_url": "https://example.com/auth",
                    "token_url": "https://example.com/token",
                    "scopes": "read",
                    "token_encoding": "json",
                    "redirect_uri": "",
                    "loopback_port": 1455,
                    "loopback_host": "",
                    "callback_path": "/cb",
                    "extra_authorize": {"audience": "api"},
                    "suggested_models": ["m1"],
                    "inference": {"user_agent": "ua", "headers": {"X-Test": "1"}}
                }
            }"#,
        )
        .unwrap();
        assert_eq!(spec.flow, FlowKind::LoopbackPkce);
        assert!(spec.client_secret.is_none());
        assert!(spec.redirect_uri.is_none());
        assert!(spec.loopback_host.is_none());
        assert_eq!(spec.loopback_port, Some(1455));
        assert_eq!(spec.token_encoding, TokenEncoding::Json);
        assert_eq!(
            spec.extra_authorize,
            vec![("audience".into(), "api".into())]
        );
        assert_eq!(spec.suggested_models, vec!["m1".to_string()]);
        let inference = spec.inference.expect("inference");
        assert_eq!(inference.user_agent.as_deref(), Some("ua"));
        assert_eq!(
            inference.headers.get("X-Test").map(String::as_str),
            Some("1")
        );

        let with_secret = spec_from_json(
            r#"{
                "kind": "auth",
                "auth": {
                    "provider": "plugin-secret",
                    "label": "Secret",
                    "flow": "loopback-pkce",
                    "client_id": "cid",
                    "client_secret": "sekrit",
                    "authorize_url": "https://example.com/auth",
                    "token_url": "https://example.com/token",
                    "scopes": "read",
                    "redirect_uri": "https://example.com/cb",
                    "loopback_host": "127.0.0.1",
                    "callback_path": "/cb"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(with_secret.client_secret.as_deref(), Some("sekrit"));
        assert_eq!(
            with_secret.redirect_uri.as_deref(),
            Some("https://example.com/cb")
        );
        assert_eq!(with_secret.loopback_host.as_deref(), Some("127.0.0.1"));
    }

    #[test]
    fn rejects_invalid_json() {
        let err = spec_from_json("not json").unwrap_err();
        assert!(err.to_string().contains("auth plugin"), "{err}");
    }

    #[test]
    fn register_from_json_rejects_invalid_spec() {
        let err = register_from_json(
            r#"{
                "kind": "auth",
                "auth": {
                    "provider": "broken",
                    "label": "Broken",
                    "flow": "device-code",
                    "client_id": "abc",
                    "authorize_url": "http://insecure.example/device",
                    "token_url": "https://example.com/token",
                    "scopes": "read"
                }
            }"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid"), "{err}");
    }

    #[test]
    fn load_dir_covers_skip_and_reject_paths() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load_dir(&tmp.path().join("missing")), 0);
        assert_eq!(load_from_dirs(&[tmp.path().join("missing")]), 0);

        let nested = tmp.path().join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "skip me").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let weird = tmp
                .path()
                .join(std::ffi::OsStr::from_bytes(b"plugin.json\xff"));
            std::fs::write(&weird, valid_device_json("never-utf8", "Nope")).unwrap();
        }
        std::fs::write(
            nested.join("manifest.json"),
            valid_device_json("fixture-from-manifest", "Manifest"),
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("plugin.json"),
            r#"{
                "kind": "auth",
                "auth": {
                    "provider": "broken-https",
                    "label": "Broken",
                    "flow": "device-code",
                    "client_id": "abc",
                    "authorize_url": "http://insecure.example/device",
                    "token_url": "https://example.com/token",
                    "scopes": "read"
                }
            }"#,
        )
        .unwrap();

        let unreadable = tmp.path().join("auth-unreadable");
        std::fs::create_dir_all(&unreadable).unwrap();
        let bad = unreadable.join("plugin.json");
        std::fs::write(&bad, valid_device_json("never-loaded", "Nope")).unwrap();
        let mut perms = std::fs::metadata(&bad).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o000);
        std::fs::set_permissions(&bad, perms).unwrap();

        let n = load_from_dirs(&[tmp.path().to_path_buf()]);
        let _ = std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o600));
        assert_eq!(n, 1);
        assert!(supports_oauth("fixture-from-manifest"));
        assert!(!supports_oauth("broken-https"));
        assert!(!supports_oauth("never-loaded"));
    }
}
