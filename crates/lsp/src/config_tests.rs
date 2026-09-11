use super::*;

#[test]
fn builtins_cover_the_existing_languages() {
    let s = LspSettings::builtins();
    assert_eq!(s.spec_for_ext("rs").unwrap().0, "rust-analyzer");
    assert_eq!(s.spec_for_ext("go").unwrap().0, "gopls");
    assert_eq!(
        s.spec_for_ext("ts").unwrap().0,
        "typescript-language-server"
    );
    assert_eq!(s.spec_for_ext("py").unwrap().0, "pyright");
    assert_eq!(s.spec_for_ext("c").unwrap().0, "clangd");
    assert_eq!(s.spec_for_ext("java").unwrap().0, "jdtls");
    assert_eq!(s.spec_for_ext("cs").unwrap().0, "omnisharp");
    assert_eq!(s.spec_for_ext("lua").unwrap().0, "lua-language-server");
    assert_eq!(s.spec_for_ext("zig").unwrap().0, "zls");
    assert_eq!(s.spec_for_ext("swift").unwrap().0, "sourcekit-lsp");
    assert!(s.spec_for_ext("xyz").is_none());
}

#[test]
fn overlay_replaces_nested_objects_and_can_disable() {
    let mut overlay = LspSettings {
        idle_timeout_ms: Some(60_000),
        ..LspSettings::default()
    };
    overlay.servers.insert(
        "rust-analyzer".into(),
        LspServerSpec {
            args: vec!["--log-file".into(), "/tmp/ra.log".into()],
            file_types: vec![".rs".into(), ".ron".into()],
            root_markers: vec!["Cargo.toml".into(), "rust-toolchain.toml".into()],
            settings: Some(serde_json::json!({"checkOnSave": false})),
            disabled: Some(true),
            ..LspServerSpec::default()
        },
    );
    overlay.servers.insert(
        "my-lsp".into(),
        LspServerSpec {
            command: Some("my-lsp".into()),
            file_types: vec![".xyz".into()],
            root_markers: vec![".xyz-project".into()],
            ..LspServerSpec::default()
        },
    );
    let merged = LspSettings::resolved(&overlay);
    assert_eq!(merged.idle_timeout_ms, Some(60_000));
    let ra = merged.servers.get("rust-analyzer").unwrap();
    assert_eq!(ra.command.as_deref(), Some("rust-analyzer"));
    assert_eq!(ra.args, vec!["--log-file", "/tmp/ra.log"]);
    assert_eq!(ra.file_types, vec![".rs", ".ron"]);
    assert_eq!(ra.root_markers, vec!["Cargo.toml", "rust-toolchain.toml"]);
    assert_eq!(ra.settings, Some(serde_json::json!({"checkOnSave": false})));
    assert!(ra.is_disabled());
    assert!(merged.spec_for_ext("rs").is_none());
    let custom = merged.servers.get("my-lsp").unwrap();
    assert_eq!(custom.command.as_deref(), Some("my-lsp"));
    assert!(custom.handles_ext("xyz"));
}

#[test]
fn file_type_dots_are_optional() {
    let spec = LspServerSpec {
        file_types: vec!["rs".into(), ".TOML".into()],
        ..LspServerSpec::default()
    };
    assert!(spec.handles_ext(".rs"));
    assert!(spec.handles_ext("toml"));
}

#[test]
fn resolve_skips_missing_binaries() {
    let settings = LspSettings::builtins();
    let cwd = Path::new(".");
    // rust-analyzer may or may not be installed; missing command must not panic.
    let _ = settings.resolve("rs", cwd);
    let missing = LspSettings {
        idle_timeout_ms: None,
        servers: HashMap::from([(
            "nope".into(),
            LspServerSpec {
                command: Some("whycodes-lsp-bin-that-does-not-exist".into()),
                file_types: vec![".zzz".into()],
                ..LspServerSpec::default()
            },
        )]),
    };
    assert!(missing.resolve("zzz", cwd).is_none());
}

#[test]
fn overlay_serde_aliases_match_toml_keys() {
    let spec: LspServerSpec = serde_json::from_value(serde_json::json!({
        "command": "gopls",
        "fileTypes": [".go"],
        "languageId": "go",
        "rootMarkers": ["go.mod"],
        "initOptions": {"foo": 1},
        "isLinter": true
    }))
    .unwrap();
    assert_eq!(spec.file_types, vec![".go"]);
    assert_eq!(spec.language_id.as_deref(), Some("go"));
    assert_eq!(spec.root_markers, vec!["go.mod"]);
    assert_eq!(spec.init_options, Some(serde_json::json!({"foo": 1})));
    assert!(spec.is_linter());
}

#[test]
fn overlay_keeps_base_args_when_overlay_args_are_empty() {
    let mut overlay = LspSettings::default();
    overlay.servers.insert(
        "typescript-language-server".into(),
        LspServerSpec {
            disabled: Some(false),
            ..LspServerSpec::default()
        },
    );
    let merged = LspSettings::resolved(&overlay);
    let ts = merged.servers.get("typescript-language-server").unwrap();
    assert_eq!(ts.args, vec!["--stdio"]);
    assert_eq!(ts.command.as_deref(), Some("typescript-language-server"));
    assert!(ts.language_id.is_none());
    assert!(!ts.is_linter());
}

#[test]
fn linters_sort_after_language_servers() {
    let mut overlay = LspSettings::default();
    overlay.servers.insert(
        "pyright".into(),
        LspServerSpec {
            is_linter: Some(true),
            ..LspServerSpec::default()
        },
    );
    let merged = LspSettings::resolved(&overlay);
    assert_eq!(merged.spec_for_ext("py").unwrap().0, "pylsp");
}

#[test]
fn resolve_skips_when_root_markers_are_missing() {
    let dir = tempfile::tempdir().unwrap();
    let settings = LspSettings::builtins();
    // rust-analyzer requires Cargo.toml in cwd, even if the binary exists.
    assert!(settings.resolve("rs", dir.path()).is_none());
}

#[test]
fn resolve_accepts_empty_markers_when_binary_exists() {
    let dir = tempfile::tempdir().unwrap();
    let settings = LspSettings {
        idle_timeout_ms: None,
        servers: HashMap::from([(
            "local".into(),
            LspServerSpec {
                command: Some("sh".into()),
                file_types: vec![".zzz".into()],
                ..LspServerSpec::default()
            },
        )]),
    };
    let resolved = settings
        .resolve("zzz", dir.path())
        .or_else(|| {
            let mut cmd = settings.clone();
            cmd.servers.get_mut("local").unwrap().command = Some("cmd".into());
            cmd.resolve("zzz", dir.path())
        })
        .or_else(|| {
            let mut py = settings;
            py.servers.get_mut("local").unwrap().command = Some("python3".into());
            py.resolve("zzz", dir.path())
        });
    assert!(resolved.is_some(), "expected sh, cmd, or python3 on PATH");
}

#[test]
fn remaining_builtin_extensions() {
    let s = LspSettings::builtins();
    assert_eq!(s.spec_for_ext("sh").unwrap().0, "bashls");
    assert_eq!(s.spec_for_ext("yaml").unwrap().0, "yaml-language-server");
    assert_eq!(s.spec_for_ext("yml").unwrap().0, "yaml-language-server");
    assert_eq!(s.spec_for_ext("bash").unwrap().0, "bashls");
    assert_eq!(s.spec_for_ext("pyi").unwrap().0, "pyright");
    assert_eq!(
        s.spec_for_ext("tsx").unwrap().0,
        "typescript-language-server"
    );
}

#[test]
fn resolve_skips_specs_without_a_command() {
    let dir = tempfile::tempdir().unwrap();
    let settings = LspSettings {
        idle_timeout_ms: None,
        servers: HashMap::from([(
            "nocmd".into(),
            LspServerSpec {
                command: None,
                file_types: vec![".zzz".into()],
                ..LspServerSpec::default()
            },
        )]),
    };
    assert!(settings.resolve("zzz", dir.path()).is_none());
    assert_eq!(settings.spec_for_ext("zzz").unwrap().0, "nocmd");
}

#[test]
fn missing_server_message_uses_name_when_command_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let settings = LspSettings {
        idle_timeout_ms: None,
        servers: HashMap::from([(
            "ghost".into(),
            LspServerSpec {
                command: None,
                file_types: vec![".zzz".into()],
                ..LspServerSpec::default()
            },
        )]),
    };
    let msg = crate::tool::missing_server_message_for_test(&settings, "zzz", dir.path());
    assert!(msg.contains("'ghost'"), "{msg}");
    assert!(msg.contains("was not found on PATH"), "{msg}");
}
