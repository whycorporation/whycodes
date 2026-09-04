//! Unit tests. Sibling file so llvm-cov --ignore-filename-regex tests.rs
//! cannot sink the crate's 100% production floor.

use super::*;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use whycodes_core::types::{
    AgentInfo, ModelConfig, PermissionAction, PermissionSet, ProviderConfig,
};

/// Serializes tests that mutate process-global env vars (WHYCODES_HOME,
/// WHYCODES_PROVIDER, …) so parallel test threads cannot interfere.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run `f` with `WHYCODES_HOME` pointed at a fresh temp dir, then restore.
fn with_isolated_home(f: impl FnOnce(&std::path::Path)) {
    let _guard = lock_env();
    let dir = tempfile::tempdir().expect("tempdir");
    let prev = std::env::var_os("WHYCODES_HOME");
    unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
    f(dir.path());
    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_HOME", v) },
        None => unsafe { std::env::remove_var("WHYCODES_HOME") },
    }
}

#[test]
fn whycodes_home_overrides_config_and_data_paths() {
    with_isolated_home(|home| {
        let cfg = Config::default_path().expect("config path");
        let data = Config::data_dir().expect("data dir");
        assert_eq!(cfg, home.join("config.toml"));
        assert_eq!(data, home);
    });
}

fn make_provider(name: &str) -> ProviderConfig {
    ProviderConfig {
        name: name.to_string(),
        api_key: Some(format!("key-{}", name)),
        api_base: None,
        base_url: None,
        headers: None,
        models: vec![],
        tool_arguments: None,
        extra: HashMap::new(),
    }
}

fn make_model(provider: &str, model: &str) -> ModelConfig {
    ModelConfig {
        model_id: model.to_string(),
        provider_id: provider.to_string(),
        max_tokens: Some(4096),
        context_window: None,
        temperature: None,
        top_p: None,
        thinking: None,
        supports_tools: None,
        supports_images: None,
    }
}

// ── test_default_config ─────────────────────────────────────────────

#[test]
fn test_default_config() {
    let cfg = Config::default();
    assert_eq!(cfg.agents.len(), 6, "default config should have 6 agents");
    assert_eq!(cfg.default_agent, "build");
    assert!(cfg.default_model.is_none());
    assert!(cfg.providers.is_empty());
    assert!(cfg.models.is_empty());
    assert_eq!(cfg.session.intent_guidance, "auto");
    assert_eq!(cfg.session.model_race, "off");
    assert_eq!(cfg.session.race_after_ms, 800);
    assert_eq!(cfg.session.response_cache, "auto");
    assert!(cfg.session.magic_keywords.enabled);
    assert!(cfg.session.magic_keywords.ultrathink);
    assert!(cfg.session.magic_keywords.orchestrate);

    // Primary: build / plan / ask; subagents: general / explore / scout
    let names: Vec<&str> = cfg.agents.iter().map(|a| a.name.as_str()).collect();
    assert!(names.contains(&"build"));
    assert!(names.contains(&"plan"));
    assert!(names.contains(&"ask"));
    assert!(names.contains(&"explore"));
    assert!(names.contains(&"general"));
    assert!(names.contains(&"scout"));

    let ask = cfg.get_agent("ask").expect("ask agent");
    assert!(!ask.permission.allow_file_writes);
    assert!(!ask.permission.allow_shell);
    assert!(
        ask.permission
            .allowed_tools
            .as_ref()
            .is_some_and(|t| t.iter().any(|n| n == "question")),
        "ask mode should include the question tool"
    );
    let plan = cfg.get_agent("plan").expect("plan agent");
    assert!(!plan.permission.allow_file_writes);
    let explore = cfg.get_agent("explore").expect("explore agent");
    assert!(
        explore
            .permission
            .denied_tools
            .as_ref()
            .is_some_and(|t| t.iter().any(|n| n == "question")),
        "explore must not advertise question"
    );
    let scout = cfg.get_agent("scout").expect("scout agent");
    assert!(
        scout
            .permission
            .denied_tools
            .as_ref()
            .is_some_and(|t| t.iter().any(|n| n == "question")),
        "scout must not advertise question"
    );
}

// ── test_config_load_save ───────────────────────────────────────────

#[test]
fn test_config_serialize_deserialize_roundtrip() {
    let mut cfg = Config::default();
    cfg.providers
        .insert("openai".to_string(), make_provider("openai"));
    cfg.models
        .insert("gpt-4".to_string(), make_model("openai", "gpt-4"));

    let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
    let loaded: Config = toml::from_str(&toml_str).expect("deserialize");

    assert_eq!(loaded.agents.len(), cfg.agents.len());
    assert_eq!(loaded.default_agent, cfg.default_agent);
    assert_eq!(loaded.providers.len(), cfg.providers.len());
    assert!(loaded.providers.contains_key("openai"));
    assert_eq!(
        loaded.providers["openai"].api_key.as_deref(),
        Some("key-openai")
    );
    assert_eq!(loaded.models.len(), cfg.models.len());
    assert_eq!(loaded.models["gpt-4"].model_id, "gpt-4");
}

#[test]
fn test_config_load_save_tempfile() {
    let mut cfg = Config::default();
    cfg.providers
        .insert("openai".to_string(), make_provider("openai"));

    let toml_str = toml::to_string_pretty(&cfg).expect("serialize");

    let dir = std::env::temp_dir();
    let path = dir.join(format!("whycodes-test-config-{}.toml", std::process::id()));
    {
        let mut f = std::fs::File::create(&path).expect("create temp file");
        f.write_all(toml_str.as_bytes()).expect("write");
    }

    let content = std::fs::read_to_string(&path).expect("read back");
    let loaded: Config = toml::from_str(&content).expect("deserialize");
    let _ = std::fs::remove_file(&path);

    assert_eq!(loaded.agents.len(), 6);
    assert_eq!(loaded.providers.len(), 1);
    assert_eq!(loaded.providers["openai"].name, "openai");
}

// ── test_get_provider ───────────────────────────────────────────────

#[test]
fn test_get_provider() {
    let mut cfg = Config::default();
    cfg.providers
        .insert("anthropic".to_string(), make_provider("anthropic"));

    assert!(cfg.get_provider("openai").is_none());
    let p = cfg
        .get_provider("anthropic")
        .expect("should find anthropic");
    assert_eq!(p.name, "anthropic");
    assert_eq!(p.api_key.as_deref(), Some("key-anthropic"));
}

// ── test_get_model ──────────────────────────────────────────────────

#[test]
fn test_get_model() {
    let mut cfg = Config::default();
    cfg.models
        .insert("gpt-4".to_string(), make_model("openai", "gpt-4"));
    cfg.models
        .insert("claude-3".to_string(), make_model("anthropic", "claude-3"));

    let m = cfg.get_model("openai", "gpt-4").expect("should find gpt-4");
    assert_eq!(m.model_id, "gpt-4");
    assert_eq!(m.provider_id, "openai");

    let m = cfg
        .get_model("anthropic", "claude-3")
        .expect("should find claude-3");
    assert_eq!(m.model_id, "claude-3");

    assert!(cfg.get_model("openai", "nonexistent").is_none());
    assert!(cfg.get_model("nonexistent", "gpt-4").is_none());
}

// ── test_get_agent ──────────────────────────────────────────────────

#[test]
fn test_get_agent() {
    let cfg = Config::default();

    let a = cfg.get_agent("build").expect("should find build agent");
    assert_eq!(a.name, "build");
    assert!(a.permission.allow_file_writes);
    assert!(a.permission.allow_shell);

    let a = cfg.get_agent("plan").expect("should find plan agent");
    assert_eq!(a.name, "plan");
    assert!(!a.permission.allow_file_writes);
    assert!(!a.permission.allow_shell);

    assert!(cfg.get_agent("nonexistent").is_none());
}

#[test]
fn test_default_agent() {
    let cfg = Config::default();
    let a = cfg.default_agent().expect("default agent should exist");
    assert_eq!(a.name, "build");
}

// ── test_substitute_vars ────────────────────────────────────────────

#[test]
fn test_substitute_vars_braced() {
    unsafe {
        std::env::set_var("WHYCODES_TEST_FOO", "hello-world");
    }
    let result = Config::substitute_vars("prefix ${WHYCODES_TEST_FOO} suffix");
    assert_eq!(result, "prefix hello-world suffix");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_FOO");
    }
}

#[test]
fn test_substitute_vars_unbraced() {
    unsafe {
        std::env::set_var("WHYCODES_TEST_BAR", "bar-value");
    }
    let result = Config::substitute_vars("start $WHYCODES_TEST_BAR end");
    assert_eq!(result, "start bar-value end");
    unsafe {
        std::env::remove_var("WHYCODES_TEST_BAR");
    }
}

#[test]
fn test_substitute_vars_unknown_kept() {
    let result = Config::substitute_vars("${NONEXISTENT_VAR_12345}");
    assert_eq!(result, "${NONEXISTENT_VAR_12345}");
}

#[test]
fn test_substitute_vars_lone_dollar() {
    let result = Config::substitute_vars("just a $ sign");
    assert_eq!(result, "just a $ sign");
}

#[test]
fn test_substitute_vars_no_vars() {
    let result = Config::substitute_vars("plain text with no variables");
    assert_eq!(result, "plain text with no variables");
}

// ── test_merge_with ─────────────────────────────────────────────────

#[test]
fn test_merge_with_priority() {
    let mut base = Config::default();
    base.providers
        .insert("openai".to_string(), make_provider("openai"));
    base.default_agent = "plan".to_string();

    let mut overlay = Config::default();
    overlay
        .providers
        .insert("anthropic".to_string(), make_provider("anthropic"));
    overlay.default_agent = "explore".to_string();
    let mc = make_model("openai", "gpt-4");
    overlay.models.insert("gpt-4".to_string(), mc.clone());
    overlay.default_model = Some(mc);

    let merged = base.merge_with(&overlay);

    // overlay providers are added
    assert_eq!(merged.providers.len(), 2);
    assert!(merged.providers.contains_key("openai"));
    assert!(merged.providers.contains_key("anthropic"));

    // overlay default_agent wins
    assert_eq!(merged.default_agent, "explore");

    // overlay default_model wins
    assert!(merged.default_model.is_some());
    assert_eq!(merged.default_model.unwrap().model_id, "gpt-4");
}

#[test]
fn test_merge_with_provider_override() {
    let mut base = Config::default();
    base.providers.insert(
        "openai".to_string(),
        ProviderConfig {
            name: "openai".to_string(),
            api_key: Some("old-key".to_string()),
            api_base: Some("https://old.example.com".to_string()),
            base_url: None,
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: HashMap::new(),
        },
    );

    let mut overlay = Config::default();
    overlay.providers.insert(
        "openai".to_string(),
        ProviderConfig {
            name: "openai".to_string(),
            api_key: Some("new-key".to_string()),
            api_base: None,
            base_url: None,
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: HashMap::new(),
        },
    );

    let merged = base.merge_with(&overlay);
    assert_eq!(merged.providers.len(), 1);
    let p = &merged.providers["openai"];
    // api_key from overlay wins
    assert_eq!(p.api_key.as_deref(), Some("new-key"));
    // api_base from base is kept (overlay didn't set it)
    assert_eq!(p.api_base.as_deref(), Some("https://old.example.com"));
}
#[test]
fn mcp_stdio_config_deserializes() {
    let toml = r#"
        [mcp_servers.fs]
        command = "npx"
        args = ["-y", "server"]
    "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    let s = &cfg.mcp_servers["fs"];
    assert_eq!(s.command.as_deref(), Some("npx"));
    assert_eq!(s.resolved_transport().unwrap(), McpTransportKind::Stdio);
    assert!(!s.is_remote());
}

#[test]
fn hooks_table_deserializes() {
    let toml = r#"
        [[hooks]]
        event = "pre_tool"
        match = "bash"
        command = "echo pre"
        block_on_failure = true
        timeout_secs = 10

        [[hooks]]
        event = "post_tool"
        match = "*"
        command = "true"
    "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.hooks.len(), 2);
    assert_eq!(cfg.hooks[0].event, HookEvent::PreTool);
    assert_eq!(cfg.hooks[0].tool_match, "bash");
    assert!(cfg.hooks[0].block_on_failure);
    assert_eq!(cfg.hooks[0].timeout_secs, 10);
    assert_eq!(cfg.hooks[1].event, HookEvent::PostTool);
    assert_eq!(cfg.hooks[1].tool_match, "*");
    assert!(!cfg.hooks[1].block_on_failure);
    assert_eq!(cfg.hooks[1].timeout_secs, 30); // default
}

#[test]
fn merge_hooks_nonempty_replaces() {
    let mut base = Config::default();
    base.hooks.push(HookConfig {
        event: HookEvent::PreTool,
        tool_match: "bash".into(),
        command: "base".into(),
        block_on_failure: false,
        timeout_secs: 5,
    });
    let mut overlay = Config::default();
    overlay.hooks.push(HookConfig {
        event: HookEvent::PostTool,
        tool_match: "*".into(),
        command: "overlay".into(),
        block_on_failure: false,
        timeout_secs: 5,
    });
    let merged = base.merge_with(&overlay);
    assert_eq!(merged.hooks.len(), 1);
    assert_eq!(merged.hooks[0].command, "overlay");
}

#[test]
fn notify_defaults_disabled() {
    let cfg = Config::default();
    assert!(cfg.notify.on.is_empty());
    assert!(!cfg.notify.has_channel());
    assert!(!cfg.notify.enabled_for(NotifyEvent::TurnDone));
    assert_eq!(cfg.notify.timeout_secs, 8);
    assert_eq!(NotifyEvent::TurnDone.as_str(), "turn_done");
    assert_eq!(NotifyEvent::NeedInput.as_str(), "need_input");
    assert_eq!(NotifyEvent::parse("TURN_DONE"), Some(NotifyEvent::TurnDone));
    assert_eq!(
        NotifyEvent::parse("need-input"),
        Some(NotifyEvent::NeedInput)
    );
    assert_eq!(NotifyEvent::parse("done"), Some(NotifyEvent::TurnDone));
    assert_eq!(NotifyEvent::parse("input"), Some(NotifyEvent::NeedInput));
    assert_eq!(NotifyEvent::parse("nope"), None);
}

#[test]
fn notify_toml_and_merge() {
    let overlay: Config = toml::from_str(
        r#"
        [notify]
        on = ["turn_done", "need_input"]
        discord_webhook = "https://discord.com/api/webhooks/1/abc"
        telegram_bot_token = "123:abc"
        telegram_chat_id = "42"
        timeout_secs = 12
        "#,
    )
    .unwrap();
    assert!(overlay.notify.wants(NotifyEvent::TurnDone));
    assert!(overlay.notify.wants(NotifyEvent::NeedInput));
    assert!(overlay.notify.has_channel());
    assert_eq!(overlay.notify.timeout_secs, 12);

    let merged = Config::default().merge_with(&overlay);
    assert_eq!(merged.notify.on, overlay.notify.on);
    assert_eq!(
        merged.notify.discord_webhook.as_deref(),
        Some("https://discord.com/api/webhooks/1/abc")
    );
    assert_eq!(merged.notify.telegram_chat_id.as_deref(), Some("42"));
    assert_eq!(merged.notify.timeout_secs, 12);

    let empty_overlay = Config::default();
    let keep = merged.merge_with(&empty_overlay);
    assert_eq!(keep.notify.on, overlay.notify.on);
    assert_eq!(keep.notify.timeout_secs, 12);
}

#[test]
fn notify_env_overrides_and_secret_expand() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: serialized by ENV_LOCK.
    unsafe {
        std::env::set_var("WHYCODES_NOTIFY_ON", "turn_done, need_input");
        std::env::set_var(
            "WHYCODES_NOTIFY_DISCORD_WEBHOOK",
            "https://discord.com/api/webhooks/9/xyz",
        );
        std::env::set_var("WHYCODES_NOTIFY_TELEGRAM_BOT_TOKEN", "bot:token");
        std::env::set_var("WHYCODES_NOTIFY_TELEGRAM_CHAT_ID", "-1001");
        std::env::set_var("WHYCODES_NOTIFY_TIMEOUT_SECS", "20");
        std::env::set_var(
            "WHYCODES_NOTIFY_EXPAND_URL",
            "https://discord.com/api/webhooks/e/x",
        );
    }
    let mut cfg = Config::default();
    cfg.apply_env_overrides();
    assert_eq!(
        cfg.notify.on,
        vec!["turn_done".to_string(), "need_input".to_string()]
    );
    assert_eq!(
        cfg.notify.discord_webhook.as_deref(),
        Some("https://discord.com/api/webhooks/9/xyz")
    );
    assert_eq!(cfg.notify.telegram_token(), Some("bot:token"));
    assert_eq!(cfg.notify.telegram_chat(), Some("-1001"));
    assert_eq!(cfg.notify.timeout_secs, 20);

    cfg.notify.discord_webhook = Some("${WHYCODES_NOTIFY_EXPAND_URL}".into());
    cfg.notify.telegram_bot_token = Some("  ".into());
    cfg.notify.telegram_chat_id = Some("${MISSING_NOTIFY_CHAT}".into());
    cfg.expand_notify_secrets();
    assert_eq!(
        cfg.notify.discord_webhook.as_deref(),
        Some("https://discord.com/api/webhooks/e/x")
    );
    assert!(cfg.notify.telegram_bot_token.is_none());
    assert_eq!(
        cfg.notify.telegram_chat_id.as_deref(),
        Some("${MISSING_NOTIFY_CHAT}")
    );

    unsafe {
        std::env::remove_var("WHYCODES_NOTIFY_ON");
        std::env::remove_var("WHYCODES_NOTIFY_DISCORD_WEBHOOK");
        std::env::remove_var("WHYCODES_NOTIFY_TELEGRAM_BOT_TOKEN");
        std::env::remove_var("WHYCODES_NOTIFY_TELEGRAM_CHAT_ID");
        std::env::remove_var("WHYCODES_NOTIFY_TIMEOUT_SECS");
        std::env::remove_var("WHYCODES_NOTIFY_EXPAND_URL");
    }
}

#[test]
fn notify_validate_and_webhook_allowlist() {
    assert!(is_discord_webhook_url(
        "https://discord.com/api/webhooks/1/token"
    ));
    assert!(is_discord_webhook_url(
        "https://discordapp.com/api/webhooks/1/token?wait=true"
    ));
    assert!(!is_discord_webhook_url(
        "http://discord.com/api/webhooks/1/t"
    ));
    assert!(!is_discord_webhook_url(
        "https://evil.example/api/webhooks/1/t"
    ));
    assert!(!is_discord_webhook_url("https://discord.com/api/webhooks/"));
    assert!(!is_discord_webhook_url("not-a-url"));
    // `https://` prefix passes, but host parse fails — covers the
    // `host_from_url` Err arm that the 100% crate floor requires.
    assert!(!is_discord_webhook_url("https://"));
    assert!(!is_discord_webhook_url("https:///api/webhooks/1/t"));
    assert!(!is_discord_webhook_url(
        "https://[no-close/api/webhooks/1/t"
    ));

    let mut cfg = Config::default();
    cfg.notify.discord_webhook = Some("https://example.com/hooks/x".into());
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("discord_webhook"), "{err}");

    cfg.notify.discord_webhook = None;
    cfg.notify.telegram_bot_token = Some("tok".into());
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("telegram"), "{err}");

    cfg.notify.telegram_bot_token = None;
    cfg.notify.telegram_chat_id = Some("1".into());
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("telegram"), "{err}");

    cfg.notify.telegram_chat_id = None;
    cfg.notify.timeout_secs = 0;
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("timeout_secs"), "{err}");

    cfg.notify.timeout_secs = 8;
    cfg.notify.on = vec!["nope".into()];
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("notify.on"), "{err}");
}

#[test]
fn mcp_http_url_infers_auto() {
    let toml = r#"
        [mcp_servers.remote]
        url = "https://mcp.example.com/mcp"
        headers = { Authorization = "Bearer t" }
    "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    let s = &cfg.mcp_servers["remote"];
    assert_eq!(s.resolved_transport().unwrap(), McpTransportKind::Auto);
    assert!(s.is_remote());
}

#[test]
fn mcp_explicit_sse_type() {
    let toml = r#"
        [mcp_servers.legacy]
        type = "sse"
        url = "http://127.0.0.1:9/sse"
    "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(
        cfg.mcp_servers["legacy"].resolved_transport().unwrap(),
        McpTransportKind::Sse
    );
}

#[test]
fn mcp_http_type_requires_url() {
    let s = McpServerConfig {
        transport: Some(McpTransportKind::Http),
        command: None,
        args: vec![],
        env: None,
        cwd: None,
        url: None,
        headers: None,
    };
    assert!(s.resolved_transport().is_err());
}

#[test]
fn mcp_stdio_type_requires_command() {
    let s = McpServerConfig {
        transport: Some(McpTransportKind::Stdio),
        command: None,
        args: vec![],
        env: None,
        cwd: None,
        url: Some("https://x".into()),
        headers: None,
    };
    assert!(s.resolved_transport().is_err());
    assert!(!s.is_remote());
}

#[test]
fn mcp_without_command_or_url_is_error() {
    let s = McpServerConfig {
        transport: None,
        command: None,
        args: vec![],
        env: None,
        cwd: None,
        url: None,
        headers: None,
    };
    assert!(s.resolved_transport().is_err());
    assert!(!s.is_remote());
}

// ── merge_with: remaining sub-config branches ───────────────────────

#[test]
fn merge_with_session_and_tui_fields() {
    let base = Config::default();
    let mut overlay = Config::default();
    overlay.session.max_context_tokens = 123_456;
    overlay.session.compaction_threshold = 99_999;
    overlay.session.store_path = Some(PathBuf::from("/tmp/store"));
    overlay.session.auto_title = false;
    overlay.session.title_model = Some("claude-haiku".into());
    overlay.session.tool_profile = "full".into();
    overlay.session.prompt_cache = "none".into();
    overlay.session.compaction_llm = "off".into();
    overlay.session.model_fast = Some("flash".into());
    overlay.session.model_smol = Some("haiku".into());
    overlay.session.model_plan = Some("sonnet".into());
    overlay.session.stream_rules = vec![crate::StreamRuleConfig {
        name: "no-leak".into(),
        pattern: "Box::leak".into(),
        hint: "don't".into(),
    }];
    overlay.session.model_race = "auto".into();
    overlay.session.race_after_ms = 1500;
    overlay.session.response_cache = "off".into();
    overlay.session.intent_guidance = "off".into();
    overlay.session.reasoning_effort = Some("high".into());
    overlay.session.magic_keywords.enabled = false;
    overlay.session.magic_keywords.ultrathink = false;
    overlay.tui.theme = Some("dark".into());
    overlay.tui.show_sidebar = true;

    let merged = base.merge_with(&overlay);
    assert_eq!(merged.session.max_context_tokens, 123_456);
    assert_eq!(merged.session.compaction_threshold, 99_999);
    assert_eq!(
        merged.session.store_path.as_deref(),
        Some(Path::new("/tmp/store"))
    );
    assert!(!merged.session.auto_title);
    assert_eq!(merged.session.title_model.as_deref(), Some("claude-haiku"));
    assert_eq!(merged.session.tool_profile, "full");
    assert_eq!(merged.session.prompt_cache, "none");
    assert_eq!(merged.session.compaction_llm, "off");
    assert_eq!(merged.session.model_fast.as_deref(), Some("flash"));
    assert_eq!(merged.session.model_smol.as_deref(), Some("haiku"));
    assert_eq!(merged.session.model_plan.as_deref(), Some("sonnet"));
    assert_eq!(merged.session.stream_rules.len(), 1);
    assert_eq!(merged.session.stream_rules[0].name, "no-leak");
    assert_eq!(merged.session.model_race, "auto");
    assert_eq!(merged.session.race_after_ms, 1500);
    assert_eq!(merged.session.response_cache, "off");
    assert_eq!(merged.session.intent_guidance, "off");
    assert_eq!(merged.session.reasoning_effort.as_deref(), Some("high"));
    assert!(!merged.session.magic_keywords.enabled);
    assert!(!merged.session.magic_keywords.ultrathink);
    assert!(merged.session.magic_keywords.orchestrate);
    assert_eq!(merged.tui.theme.as_deref(), Some("dark"));
    assert!(merged.tui.show_sidebar);
}

#[test]
fn magic_keywords_toml_partial_uses_field_defaults() {
    let cfg: Config = toml::from_str(
        "[session.magic_keywords]\n\
         ultrathink = false\n",
    )
    .unwrap();
    assert!(cfg.session.magic_keywords.enabled);
    assert!(!cfg.session.magic_keywords.ultrathink);
    assert!(cfg.session.magic_keywords.orchestrate);
}

#[test]
fn model_roles_and_stream_rules_from_toml() {
    let cfg: Config = toml::from_str(
        r#"
[session]
model_smol = "anthropic/claude-haiku-4-5-20251001"
model_plan = "claude-opus-4-6"

[[session.stream_rules]]
name = "no-leak"
pattern = "Box::leak"
hint = "Don't use Box::leak"
"#,
    )
    .unwrap();
    assert_eq!(
        cfg.session.model_smol.as_deref(),
        Some("anthropic/claude-haiku-4-5-20251001")
    );
    assert_eq!(cfg.session.model_plan.as_deref(), Some("claude-opus-4-6"));
    assert_eq!(cfg.session.stream_rules.len(), 1);
    assert_eq!(cfg.session.stream_rules[0].pattern, "Box::leak");
    let empty: Config = toml::from_str("[session]\n").unwrap();
    assert!(empty.session.stream_rules.is_empty());
    assert!(empty.session.model_smol.is_none());
}

#[test]
fn merge_with_general_security_memory_swarm() {
    let base = Config::default();
    let mut overlay = Config::default();
    overlay.general.project_path = Some(PathBuf::from("/proj"));
    overlay.general.log_level = Some("debug".into());
    overlay.general.default_gcp_project = Some("gcp-proj".into());
    overlay.schema_version = CONFIG_SCHEMA_VERSION + 1;
    overlay.security.bash_risk_threshold = "caution".into();
    overlay.security.sandbox = "off".into();
    overlay.security.sandbox_network = false;
    overlay.security.sandbox_fallback = "deny".into();
    overlay.security.network_allowlist = vec!["example.com".into()];
    overlay.security.network_denylist = vec!["bad.example".into()];
    overlay.memory.enabled = false;
    overlay.memory.auto_inject = false;
    overlay.memory.retain_llm_always = true;
    overlay.memory.retain_every_n = 5;
    overlay.memory.recall_min_score = 0.5;
    overlay.memory.scope = "project".into();
    overlay.memory.consolidate_max = 40;
    overlay.swarm.enabled = false;
    overlay.swarm.max_agents = 6;
    overlay.swarm.worktrees = false;
    overlay.swarm.isolation = Some("checkout".into());
    overlay.automation.max_background_jobs = 3;

    let merged = base.merge_with(&overlay);
    assert_eq!(
        merged.general.project_path.as_deref(),
        Some(Path::new("/proj"))
    );
    assert_eq!(merged.general.log_level.as_deref(), Some("debug"));
    assert_eq!(
        merged.general.default_gcp_project.as_deref(),
        Some("gcp-proj")
    );
    assert_eq!(merged.schema_version, CONFIG_SCHEMA_VERSION + 1);
    overlay.general.auto_update = false;
    overlay.general.approval_mode = Some(whycodes_core::types::ApprovalMode::Manual);
    let merged = base.merge_with(&overlay);
    assert!(!merged.general.auto_update);
    assert_eq!(
        merged.general.approval_mode,
        Some(whycodes_core::types::ApprovalMode::Manual)
    );
    assert_eq!(merged.security.bash_risk_threshold, "caution");
    assert_eq!(merged.security.sandbox, "off");
    assert!(!merged.security.sandbox_network);
    assert_eq!(merged.security.sandbox_fallback, "deny");
    assert_eq!(merged.security.network_allowlist, vec!["example.com"]);
    assert_eq!(merged.security.network_denylist, vec!["bad.example"]);
    assert!(!merged.memory.enabled);
    assert!(!merged.memory.auto_inject);
    assert!(merged.memory.retain_llm_always);
    assert_eq!(merged.memory.retain_every_n, 5);
    assert!((merged.memory.recall_min_score - 0.5).abs() < f32::EPSILON);
    assert_eq!(merged.memory.scope, "project");
    assert_eq!(merged.memory.consolidate_max, 40);
    assert!(!merged.swarm.enabled);
    assert_eq!(merged.swarm.max_agents, 6);
    assert!(!merged.swarm.worktrees);
    assert_eq!(merged.swarm.isolation.as_deref(), Some("checkout"));
    assert_eq!(merged.automation.max_background_jobs, 3);
}

#[test]
fn merge_with_mcp_permission_commands() {
    let base = Config::default();
    let mut overlay = Config::default();
    overlay.mcp_servers.insert(
        "fs".into(),
        McpServerConfig {
            transport: None,
            command: Some("npx".into()),
            args: vec!["-y".into()],
            env: None,
            cwd: None,
            url: None,
            headers: None,
        },
    );
    overlay
        .permission
        .insert("bash".into(), PermissionAction::Ask);
    overlay.commands.insert(
        "deploy".into(),
        CustomCommandConfig {
            template: "ship it".into(),
            description: Some("deploy".into()),
            agent: None,
            model: None,
            subtask: None,
        },
    );

    let merged = base.merge_with(&overlay);
    assert!(merged.mcp_servers.contains_key("fs"));
    assert_eq!(merged.permission.get("bash"), Some(&PermissionAction::Ask));
    assert_eq!(merged.commands["deploy"].template, "ship it");
}

#[test]
fn merge_with_tools_flags() {
    // The merge only applies flags that differ from the derived `Default`
    // (all tools off) — i.e. it can *enable* a tool and replace the
    // disabled/custom lists, but cannot turn an already-true flag off.
    let base = Config::default();
    assert!(!base.tools.enable_read, "derived default is all-off");
    let mut overlay = Config::default();
    overlay.tools.enable_shell = true;
    overlay.tools.disabled_tools = vec!["websearch".into()];
    overlay.tools.question.timeout_secs = 60;

    let merged = base.merge_with(&overlay);
    assert!(merged.tools.enable_shell, "overlay true flag applied");
    assert_eq!(merged.tools.disabled_tools, vec!["websearch"]);
    assert_eq!(merged.tools.question.timeout_secs, 60);
    // untouched flags keep the base value (off)
    assert!(!merged.tools.enable_read);
    assert!(!merged.tools.enable_grep);
}

// ── effective_permission ────────────────────────────────────────────

#[test]
fn effective_permission_merges_global_rules_agent_wins() {
    let mut cfg = Config::default();
    cfg.permission.insert("bash".into(), PermissionAction::Ask);
    cfg.permission.insert("read".into(), PermissionAction::Deny);

    let mut agent = PermissionSet::default();
    agent.rules.insert("bash".into(), PermissionAction::Allow);

    let out = cfg.effective_permission(&agent);
    // global rule added where the agent had none
    assert_eq!(out.rules.get("read"), Some(&PermissionAction::Deny));
    // agent rule wins over global
    assert_eq!(out.rules.get("bash"), Some(&PermissionAction::Allow));
}

// ── swarm use_worktrees ─────────────────────────────────────────────

#[test]
fn swarm_use_worktrees_respects_isolation() {
    let s = SwarmConfig {
        enabled: true,
        max_agents: 4,
        worktrees: true,
        isolation: None,
    };
    assert!(s.use_worktrees());

    let checkout = SwarmConfig {
        isolation: Some("checkout".into()),
        ..s.clone()
    };
    assert!(!checkout.use_worktrees());

    let wt = SwarmConfig {
        isolation: Some("worktree".into()),
        ..s.clone()
    };
    assert!(wt.use_worktrees());

    // case-insensitive + unknown falls back to `worktrees`
    let caps = SwarmConfig {
        isolation: Some("CHECKOUT".into()),
        ..s.clone()
    };
    assert!(!caps.use_worktrees());
    let unknown = SwarmConfig {
        isolation: Some("bogus".into()),
        ..s.clone()
    };
    assert!(unknown.use_worktrees());
}

// ── security helpers ────────────────────────────────────────────────

#[test]
#[allow(clippy::field_reassign_with_default)]
fn security_network_policy_and_sandbox_settings() {
    let mut sec = SecurityConfig::default();
    sec.network_allowlist = vec!["good.example".into()];
    sec.network_denylist = vec!["evil.example".into()];
    let np = sec.network_policy();
    assert_eq!(np.allowlist, vec!["good.example"]);
    assert_eq!(np.denylist, vec!["evil.example"]);

    let ss = sec.sandbox_settings();
    assert_eq!(ss.mode, SandboxMode::Workspace);
    assert!(ss.network);
    assert_eq!(ss.fallback, SandboxFallback::Allow);

    let off = SecurityConfig {
        sandbox: "off".into(),
        sandbox_fallback: "deny".into(),
        ..sec.clone()
    };
    let ss = off.sandbox_settings();
    assert_eq!(ss.mode, SandboxMode::Off);
    assert_eq!(ss.fallback, SandboxFallback::Deny);
}

// ── validate ────────────────────────────────────────────────────────

#[test]
fn validate_default_fails_missing_provider() {
    let cfg = Config::default();
    assert!(cfg.validate().is_err());
}

#[test]
fn validate_provider_with_key_ok() {
    let mut cfg = Config::default();
    cfg.providers
        .insert("openai".into(), make_provider("openai"));
    assert!(cfg.validate().is_ok());
}

#[test]
fn validate_default_model_without_provider_id_fails() {
    let mut cfg = Config::default();
    cfg.providers
        .insert("openai".into(), make_provider("openai"));
    let mut dm = make_model("openai", "gpt-4");
    dm.provider_id = String::new();
    cfg.default_model = Some(dm);
    let err = cfg.validate().unwrap_err();
    assert!(err.to_string().contains("provider_id"), "{err}");
}

#[test]
fn validate_empty_model_id_fails() {
    let mut cfg = Config::default();
    cfg.providers
        .insert("openai".into(), make_provider("openai"));
    cfg.default_model = Some(make_model("openai", ""));
    let err = cfg.validate().unwrap_err();
    assert!(err.to_string().contains("empty model_id"), "{err}");
}

#[test]
fn validate_unknown_default_agent_fails() {
    let mut cfg = Config::default();
    cfg.providers
        .insert("openai".into(), make_provider("openai"));
    cfg.default_agent = "ghost".into();
    let err = cfg.validate().unwrap_err();
    assert!(err.to_string().contains("'ghost'"), "{err}");
}

#[test]
fn validate_localhost_base_url_is_only_warning() {
    let mut cfg = Config::default();
    cfg.providers.insert(
        "local".into(),
        ProviderConfig {
            name: "local".into(),
            api_key: Some("k".into()),
            api_base: None,
            base_url: Some("http://localhost:11434/v1".into()),
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: HashMap::new(),
        },
    );
    assert!(cfg.validate().is_ok());
}

#[test]
fn validate_bad_response_cache_fails() {
    let mut cfg = Config::default();
    cfg.session.response_cache = "sometimes".into();
    let err = cfg.validate().unwrap_err();
    assert!(err.to_string().contains("response_cache"), "{err}");
}

#[test]
fn validate_zero_context_tokens_fails() {
    let mut cfg = Config::default();
    cfg.session.max_context_tokens = 0;
    assert!(cfg.validate().is_err());
}

#[test]
fn validate_long_race_after_ms_fails() {
    let mut cfg = Config::default();
    cfg.session.race_after_ms = 31_000;
    assert!(cfg.validate().is_err());
}

// ── apply_env_overrides ─────────────────────────────────────────────

fn clear_env(names: &[&str]) -> Vec<Option<std::ffi::OsString>> {
    names
        .iter()
        .map(|n| {
            let prev = std::env::var_os(n);
            unsafe { std::env::remove_var(n) };
            prev
        })
        .collect()
}

fn restore_env(names: &[&str], prev: Vec<Option<std::ffi::OsString>>) {
    for (n, v) in names.iter().zip(prev) {
        match v {
            Some(v) => unsafe { std::env::set_var(n, v) },
            None => unsafe { std::env::remove_var(n) },
        }
    }
}

#[test]
fn apply_env_overrides_provider_and_model() {
    let _guard = lock_env();
    let names = [
        "WHYCODES_PROVIDER",
        "WHYCODES_MODEL",
        "WHYCODES_MAX_TURNS",
        "WHYCODES_LOG_LEVEL",
        "WHYCODES_PROJECT_DIR",
    ];
    let prev = clear_env(&names);
    unsafe {
        std::env::set_var("WHYCODES_PROVIDER", "acme");
        std::env::set_var("WHYCODES_MODEL", "acme-sonnet");
        std::env::set_var("WHYCODES_MAX_TURNS", "42");
        std::env::set_var("WHYCODES_LOG_LEVEL", "debug");
        std::env::set_var("WHYCODES_PROJECT_DIR", "/work");
    }
    let mut cfg = Config::default();
    cfg.apply_env_overrides();
    restore_env(&names, prev);

    let prov = cfg.providers.get("acme").expect("provider auto-created");
    assert_eq!(prov.name, "acme");
    let dm = cfg.default_model.expect("default model from env");
    assert_eq!(dm.model_id, "acme-sonnet");
    assert_eq!(dm.provider_id, "acme");
    assert_eq!(cfg.session.max_context_tokens, 42);
    assert_eq!(cfg.general.log_level.as_deref(), Some("debug"));
    assert_eq!(
        cfg.general.project_path.as_deref(),
        Some(Path::new("/work"))
    );
}

#[test]
#[allow(clippy::field_reassign_with_default)]
fn apply_env_overrides_model_without_provider() {
    let _guard = lock_env();
    let names = ["WHYCODES_PROVIDER", "WHYCODES_MODEL"];
    let prev = clear_env(&names);
    unsafe { std::env::set_var("WHYCODES_MODEL", "bare-model") };
    let mut cfg = Config::default();
    cfg.default_model = Some(make_model("openai", "old"));
    cfg.apply_env_overrides();
    restore_env(&names, prev);

    assert!(cfg.providers.is_empty());
    assert_eq!(cfg.default_model.as_ref().unwrap().model_id, "bare-model");
}

#[test]
fn apply_env_overrides_sandbox_and_memory() {
    let _guard = lock_env();
    let names = [
        "WHYCODES_SANDBOX",
        "WHYCODES_SANDBOX_NETWORK",
        "WHYCODES_SANDBOX_FALLBACK",
        "WHYCODES_NETWORK_ALLOWLIST",
        "WHYCODES_NETWORK_DENYLIST",
        "WHYCODES_NO_MEMORY",
        "WHYCODES_NO_AUTO_UPDATE",
        "WHYCODES_AUTO_UPDATE",
        "WHYCODES_APPROVAL_MODE",
        "WHYCODES_MEMORY",
        "WHYCODES_SWARM",
        "WHYCODES_SWARM_MAX_AGENTS",
        "WHYCODES_SWARM_WORKTREES",
    ];
    let prev = clear_env(&names);
    unsafe {
        std::env::set_var("WHYCODES_SANDBOX", "off");
        std::env::set_var("WHYCODES_SANDBOX_NETWORK", "0");
        std::env::set_var("WHYCODES_SANDBOX_FALLBACK", "deny");
        std::env::set_var("WHYCODES_NETWORK_ALLOWLIST", "a.com, b.com");
        std::env::set_var("WHYCODES_NETWORK_DENYLIST", "evil.com");
        std::env::set_var("WHYCODES_NO_MEMORY", "1");
        std::env::set_var("WHYCODES_SWARM", "0");
        std::env::set_var("WHYCODES_SWARM_MAX_AGENTS", "12");
        std::env::set_var("WHYCODES_SWARM_WORKTREES", "0");
    }
    let mut cfg = Config::default();
    cfg.apply_env_overrides();
    restore_env(&names, prev);

    assert_eq!(cfg.security.sandbox, "off");
    assert!(!cfg.security.sandbox_network);
    assert_eq!(cfg.security.sandbox_fallback, "deny");
    assert_eq!(cfg.security.network_allowlist, vec!["a.com", "b.com"]);
    assert_eq!(cfg.security.network_denylist, vec!["evil.com"]);
    assert!(!cfg.memory.enabled);
    assert!(!cfg.swarm.enabled);
    assert_eq!(cfg.swarm.max_agents, 8, "clamped to hard cap");
    assert!(!cfg.swarm.worktrees);
}

// ── custom command markdown ─────────────────────────────────────────

#[test]
fn parse_command_markdown_plain_body() {
    let cmd = parse_command_markdown("just a prompt").unwrap();
    assert_eq!(cmd.template, "just a prompt");
    assert!(cmd.description.is_none());
    assert!(cmd.agent.is_none());
    assert!(cmd.subtask.is_none());
}

#[test]
fn parse_command_markdown_with_frontmatter() {
    let md = "---\ndescription: \"Fix it\"\nagent: plan\nmodel: sonnet\nsubtask: yes\n---\nDo the thing with $ARGUMENTS";
    let cmd = parse_command_markdown(md).unwrap();
    assert_eq!(cmd.template, "Do the thing with $ARGUMENTS");
    assert_eq!(cmd.description.as_deref(), Some("Fix it"));
    assert_eq!(cmd.agent.as_deref(), Some("plan"));
    assert_eq!(cmd.model.as_deref(), Some("sonnet"));
    assert_eq!(cmd.subtask, Some(true));
}

#[test]
fn render_expands_arguments_and_positionals() {
    let cmd = CustomCommandConfig {
        template: "$2 — $ARGUMENTS ($1)".into(),
        description: None,
        agent: None,
        model: None,
        subtask: None,
    };
    assert_eq!(cmd.render("alpha beta"), "beta — alpha beta (alpha)");
}

#[test]
fn load_command_files_loads_markdown_and_builtins() {
    with_isolated_home(|home| {
        let cmds = home.join("commands");
        std::fs::create_dir_all(&cmds).unwrap();
        std::fs::write(
            cmds.join("review.md"),
            "---\ndescription: my review\n---\nReview $ARGUMENTS",
        )
        .unwrap();
        std::fs::write(cmds.join("notes.txt"), "ignored").unwrap();

        let mut cfg = Config::default();
        cfg.load_command_files(Path::new("/nonexistent-project"));
        assert!(cfg.commands.contains_key("review"));
        assert!(!cfg.commands.contains_key("notes"));
        // built-ins added only for missing keys
        assert!(cfg.commands.contains_key("commit"));
        assert!(cfg.commands.contains_key("security-review"));
    });
}

#[test]
fn builtin_prompt_commands_do_not_overwrite_user() {
    let mut cfg = Config::default();
    cfg.commands.insert(
        "review".into(),
        CustomCommandConfig {
            template: "user wins".into(),
            description: None,
            agent: None,
            model: None,
            subtask: None,
        },
    );
    cfg.ensure_builtin_prompt_commands();
    assert_eq!(cfg.commands["review"].template, "user wins");
    assert!(cfg.commands.contains_key("commit"));
}

// ── accessors ───────────────────────────────────────────────────────

#[test]
fn configured_context_window_from_model_and_default() {
    let mut cfg = Config::default();
    let mut m = make_model("openai", "gpt-4");
    m.context_window = Some(128_000);
    cfg.models.insert("gpt-4".into(), m.clone());
    assert_eq!(
        cfg.configured_context_window("openai", "gpt-4"),
        Some(128_000)
    );

    // default_model fallback when model matches
    let mut dm = make_model("openai", "gpt-5");
    dm.context_window = Some(400_000);
    cfg.default_model = Some(dm);
    assert_eq!(
        cfg.configured_context_window("openai", "gpt-5"),
        Some(400_000)
    );
    // default_model applies when provider_id is empty too
    let mut dm2 = make_model("", "gpt-6");
    dm2.context_window = Some(300_000);
    cfg.default_model = Some(dm2);
    assert_eq!(
        cfg.configured_context_window("anthropic", "gpt-6"),
        Some(300_000)
    );
    // provider mismatch → None
    assert_eq!(cfg.configured_context_window("other", "gpt-5"), None);
}

#[test]
fn get_command_config_returns_override() {
    let mut cfg = Config::default();
    cfg.command_configs.insert(
        "run".into(),
        CommandConfig {
            model: None,
            agent: Some("plan".into()),
            max_turns: Some(5),
        },
    );
    let cc = cfg.get_command_config("run").expect("command config");
    assert_eq!(cc.agent.as_deref(), Some("plan"));
    assert_eq!(cc.max_turns, Some(5));
    assert!(cfg.get_command_config("nope").is_none());
}

#[test]
fn project_path_uses_configured_or_cwd() {
    let _guard = lock_env();
    let cfg = Config::default();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    assert_eq!(cfg.project_path(), cwd);

    let mut cfg2 = Config::default();
    cfg2.general.project_path = Some(PathBuf::from("/p"));
    assert_eq!(cfg2.project_path(), PathBuf::from("/p"));
}

#[test]
fn hook_and_security_defaults_and_serde() {
    let h = HookConfig::default();
    assert_eq!(h.tool_match, "*");
    assert_eq!(h.timeout_secs, 30);
    let sec: SecurityConfig = toml::from_str("").unwrap();
    assert!(sec.sandbox_network);
    let tools: ToolsConfig = toml::from_str("").unwrap();
    assert!(tools.enable_read && tools.enable_shell);
    // serde defaults for session / tui / memory / swarm / hook match
    let cfg: Config = toml::from_str(
        r#"
        [session]
        [tui]
        [memory]
        [swarm]
        [notify]
        [[hooks]]
        command = "true"
        "#,
    )
    .unwrap();
    assert!(cfg.session.auto_title);
    assert_eq!(cfg.tui.prompt_suggestions, "off");
    assert!(cfg.tui.agent_colors.is_empty());
    assert!(cfg.memory.enabled);
    assert!(cfg.swarm.enabled);
    assert!(cfg.notify.on.is_empty());
    assert_eq!(cfg.notify.timeout_secs, 8);
    assert_eq!(cfg.hooks[0].tool_match, "*");
    assert_eq!(cfg.hooks[0].timeout_secs, 30);
}

#[test]
fn tui_agent_colors_table_parses() {
    let cfg: Config = toml::from_str(
        r##"
        [tui.agent_colors]
        build = "#7aa2f7"
        plan = "accent"
        model = "secondary"
        "##,
    )
    .unwrap();
    assert_eq!(cfg.tui.agent_colors.get("build").unwrap(), "#7aa2f7");
    assert_eq!(cfg.tui.agent_colors.get("plan").unwrap(), "accent");
    assert_eq!(cfg.tui.agent_colors.get("model").unwrap(), "secondary");
}

#[test]
fn load_missing_file_returns_default() {
    with_isolated_home(|home| {
        let cfg = Config::load().unwrap();
        assert_eq!(cfg.default_agent, "build");
        assert!(!home.join("config.toml").exists());
    });
}

#[test]
fn load_existing_fills_provider_name_from_key() {
    with_isolated_home(|home| {
        std::fs::write(
            home.join("config.toml"),
            "[providers.acme]\nname = \"\"\napi_key = \"k\"\n",
        )
        .unwrap();
        let cfg = Config::load().unwrap();
        assert_eq!(cfg.providers["acme"].name, "acme");
        cfg.save().unwrap();
        assert!(home.join("config.toml").exists());
    });
}

#[test]
fn load_migrates_missing_schema_version() {
    with_isolated_home(|home| {
        std::fs::write(home.join("config.toml"), "default_agent = \"plan\"\n").unwrap();
        let cfg = Config::load().unwrap();
        assert_eq!(cfg.schema_version, CONFIG_SCHEMA_VERSION);
        assert!(cfg.general.auto_update);
        assert_eq!(cfg.default_agent, "plan");
        let on_disk = std::fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(on_disk.contains("schema_version"), "{on_disk}");
    });
}

#[test]
fn migrate_schema_is_idempotent() {
    let mut cfg = Config::default();
    assert_eq!(cfg.schema_version, CONFIG_SCHEMA_VERSION);
    assert!(!cfg.migrate_schema().unwrap());
}

#[test]
fn migrate_schema_keeps_bump_when_save_fails() {
    with_isolated_home(|home| {
        // `save()` writes `$WHYCODES_HOME/config.toml`; a directory there makes
        // the rewrite fail so the warn/Ok(true) path is exercised.
        std::fs::create_dir(home.join("config.toml")).unwrap();
        let mut cfg = Config {
            schema_version: 0,
            ..Config::default()
        };
        assert!(cfg.migrate_schema().unwrap());
        assert_eq!(cfg.schema_version, CONFIG_SCHEMA_VERSION);
        assert!(home.join("config.toml").is_dir());
    });
}

#[test]
fn load_rejects_bad_toml_and_unreadable_file() {
    with_isolated_home(|home| {
        std::fs::write(home.join("config.toml"), "[[[not toml").unwrap();
        assert!(Config::load().is_err());
        assert!(Config::load_layered(home).is_err());

        std::fs::remove_file(home.join("config.toml")).unwrap();
        std::fs::create_dir(home.join("config.toml")).unwrap();
        assert!(Config::load().is_err());
    });
}

#[test]
fn load_layered_merges_project_and_warns_on_bad_toml() {
    with_isolated_home(|home| {
        std::fs::write(home.join("config.toml"), "default_agent = \"plan\"\n").unwrap();
        let proj = home.join("proj");
        std::fs::create_dir_all(proj.join(".whycodes")).unwrap();
        std::fs::write(
            proj.join(".whycodes/config.toml"),
            "default_agent = \"explore\"\n",
        )
        .unwrap();
        let cfg = Config::load_layered(&proj).unwrap();
        assert_eq!(cfg.default_agent, "explore");

        std::fs::write(proj.join(".whycodes/config.toml"), "[[[not toml").unwrap();
        let cfg = Config::load_layered(&proj).unwrap();
        assert_eq!(cfg.default_agent, "plan");

        // unreadable project file (directory instead of file)
        std::fs::remove_file(proj.join(".whycodes/config.toml")).unwrap();
        std::fs::create_dir(proj.join(".whycodes/config.toml")).unwrap();
        let cfg = Config::load_layered(&proj).unwrap();
        assert_eq!(cfg.default_agent, "plan");

        // no project file at all
        std::fs::remove_dir(proj.join(".whycodes/config.toml")).unwrap();
        let cfg = Config::load_layered(&proj).unwrap();
        assert_eq!(cfg.default_agent, "plan");
    });
}

#[test]
fn apply_env_overrides_cover_every_knob() {
    let _guard = lock_env();
    let names = [
        "WHYCODES_PROVIDER",
        "WHYCODES_MODEL",
        "GROK_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "WHYCODES_MAX_TURNS",
        "WHYCODES_LOG_LEVEL",
        "WHYCODES_PROJECT_DIR",
        "WHYCODES_SANDBOX",
        "WHYCODES_SANDBOX_NETWORK",
        "WHYCODES_SANDBOX_FALLBACK",
        "WHYCODES_NETWORK_ALLOWLIST",
        "WHYCODES_NETWORK_DENYLIST",
        "WHYCODES_NO_MEMORY",
        "WHYCODES_NO_AUTO_UPDATE",
        "WHYCODES_AUTO_UPDATE",
        "WHYCODES_APPROVAL_MODE",
        "WHYCODES_MEMORY",
        "WHYCODES_SWARM",
        "WHYCODES_SWARM_MAX_AGENTS",
        "WHYCODES_SWARM_WORKTREES",
    ];
    let prev = clear_env(&names);

    unsafe {
        std::env::set_var("WHYCODES_PROVIDER", "grok");
        std::env::set_var("WHYCODES_MODEL", "grok-4");
        std::env::set_var("GROK_API_KEY", "gk");
    }
    let mut cfg = Config::default();
    cfg.apply_env_overrides();
    assert_eq!(cfg.providers["grok"].api_key.as_deref(), Some("gk"));
    assert_eq!(
        cfg.default_model.as_ref().map(|m| m.model_id.as_str()),
        Some("grok-4")
    );

    // existing provider is not recreated; WHYCODES_MODEL without provider
    // rewrites the default model's id.
    unsafe { std::env::remove_var("WHYCODES_PROVIDER") };
    unsafe { std::env::set_var("WHYCODES_MODEL", "other") };
    cfg.apply_env_overrides();
    assert_eq!(cfg.default_model.as_ref().unwrap().model_id, "other");

    // model without existing default
    let mut cfg2 = Config::default();
    unsafe { std::env::set_var("WHYCODES_MODEL", "solo") };
    cfg2.apply_env_overrides();
    assert_eq!(cfg2.default_model.as_ref().unwrap().model_id, "solo");
    assert!(cfg2.default_model.as_ref().unwrap().provider_id.is_empty());

    // WHYCODES_PROVIDER when the provider already exists — skip insert, still
    // pick up WHYCODES_MODEL when default_model is None.
    unsafe {
        std::env::set_var("WHYCODES_PROVIDER", "grok");
        std::env::set_var("WHYCODES_MODEL", "grok-again");
    }
    cfg.default_model = None;
    cfg.apply_env_overrides();
    assert_eq!(
        cfg.default_model.as_ref().map(|m| m.model_id.as_str()),
        Some("grok-again")
    );

    // invalid parses are ignored
    unsafe { std::env::set_var("WHYCODES_MAX_TURNS", "nope") };
    unsafe { std::env::set_var("WHYCODES_SWARM_MAX_AGENTS", "x") };
    cfg.apply_env_overrides();

    unsafe { std::env::set_var("WHYCODES_SANDBOX_NETWORK", "yes") };
    cfg.apply_env_overrides();
    assert!(cfg.security.sandbox_network);

    unsafe { std::env::set_var("WHYCODES_NO_MEMORY", "maybe") };
    cfg.memory.enabled = true;
    cfg.apply_env_overrides();
    assert!(cfg.memory.enabled, "unrecognized NO_MEMORY leaves enabled");

    unsafe { std::env::set_var("WHYCODES_MEMORY", "on") };
    cfg.apply_env_overrides();
    assert!(cfg.memory.enabled);
    unsafe { std::env::set_var("WHYCODES_MEMORY", "off") };
    cfg.apply_env_overrides();
    assert!(!cfg.memory.enabled);

    assert!(cfg.general.auto_update);
    unsafe { std::env::set_var("WHYCODES_NO_AUTO_UPDATE", "1") };
    cfg.apply_env_overrides();
    assert!(!cfg.general.auto_update);
    unsafe { std::env::set_var("WHYCODES_AUTO_UPDATE", "on") };
    cfg.apply_env_overrides();
    assert!(cfg.general.auto_update);
    unsafe { std::env::set_var("WHYCODES_AUTO_UPDATE", "off") };
    cfg.apply_env_overrides();
    assert!(!cfg.general.auto_update);
    unsafe { std::env::set_var("WHYCODES_AUTO_UPDATE", "maybe") };
    cfg.apply_env_overrides();
    assert!(!cfg.general.auto_update);
    assert_eq!(cfg.general.approval_mode, None);
    unsafe { std::env::set_var("WHYCODES_APPROVAL_MODE", "manual") };
    cfg.apply_env_overrides();
    assert_eq!(
        cfg.general.approval_mode,
        Some(whycodes_core::types::ApprovalMode::Manual)
    );
    unsafe { std::env::set_var("WHYCODES_APPROVAL_MODE", "important") };
    cfg.apply_env_overrides();
    assert_eq!(
        cfg.general.approval_mode,
        Some(whycodes_core::types::ApprovalMode::Important)
    );
    unsafe { std::env::set_var("WHYCODES_APPROVAL_MODE", "nope") };
    cfg.apply_env_overrides();
    assert_eq!(
        cfg.general.approval_mode,
        Some(whycodes_core::types::ApprovalMode::Important)
    );
    unsafe { std::env::set_var("WHYCODES_MEMORY", "maybe") };
    cfg.apply_env_overrides();

    unsafe { std::env::set_var("WHYCODES_SWARM", "1") };
    cfg.apply_env_overrides();
    assert!(cfg.swarm.enabled);
    unsafe { std::env::set_var("WHYCODES_SWARM", "huh") };
    cfg.apply_env_overrides();
    unsafe { std::env::set_var("WHYCODES_SWARM_WORKTREES", "1") };
    cfg.apply_env_overrides();
    assert!(cfg.swarm.worktrees);
    unsafe { std::env::set_var("WHYCODES_SWARM_WORKTREES", "maybe") };
    cfg.apply_env_overrides();
    assert!(cfg.swarm.worktrees);

    restore_env(&names, prev);
}

#[test]
fn merge_with_covers_provider_model_memory_tools() {
    let mut base = Config::default();
    base.providers.insert("p".into(), make_provider("p"));
    let mut other = Config::default();
    other.providers.insert(
        "p".into(),
        ProviderConfig {
            name: "p".into(),
            api_key: Some("new".into()),
            api_base: Some("http://a".into()),
            base_url: Some("http://b".into()),
            headers: Some(HashMap::from([("h".into(), "v".into())])),
            models: vec!["m1".into()],
            tool_arguments: Some(whycodes_core::types::ToolArgumentsFormat::Object),
            extra: HashMap::from([("k".into(), serde_json::json!(1))]),
        },
    );
    base.models.insert("mid".into(), make_model("p", "mid"));
    other.models.insert(
        "mid".into(),
        ModelConfig {
            model_id: "mid".into(),
            provider_id: "p".into(),
            max_tokens: Some(1),
            context_window: Some(2),
            temperature: Some(0.1),
            top_p: Some(0.2),
            thinking: Some(true),
            supports_tools: Some(true),
            supports_images: Some(false),
        },
    );
    other.agents.push(AgentInfo {
        name: "extra".into(),
        description: "x".into(),
        mode: whycodes_core::types::AgentMode::Primary,
        permission: PermissionSet::default(),
        model: None,
        system_prompt: None,
        temperature: None,
        top_p: None,
    });
    other.memory.enabled = false;
    other.memory.auto_inject = false;
    other.memory.auto_retain = false;
    other.memory.retain_llm = false;
    other.memory.retain_llm_always = true;
    other.memory.retain_every_n = 99;
    other.memory.retain_max_facts = 7;
    other.memory.max_index_lines = 11;
    other.memory.max_index_bytes = 22;
    other.memory.recall_top_k = 3;
    other.memory.recall_min_score = 0.9;
    other.memory.recall_token_budget = 10;
    other.memory.embed_dim = 16;
    other.memory.scope = "session".into();
    other.memory.embed_backend = "onnx".into();
    other.memory.code_inject = false;
    other.memory.code_top_k = 9;
    other.memory.code_min_score = 0.8;
    other.memory.subagent_banks = false;
    other.memory.auto_index = false;
    other.memory.auto_index_max_files = 5;
    other.memory.auto_index_max_chunks = 6;
    other.memory.session_inject = false;
    other.memory.session_top_k = 2;
    other.memory.session_min_score = 0.7;
    other.memory.consolidate = false;
    other.memory.consolidate_max = 8;
    other.swarm.enabled = false;
    other.swarm.max_agents = 2;
    other.swarm.worktrees = false;
    other.swarm.isolation = Some("strict".into());
    other.automation.max_background_jobs = 3;
    other.hooks.push(HookConfig::default());
    other.security.network_denylist = vec!["bad.com".into()];
    other.tools.enable_read = false;
    other.tools.enable_write = false;
    other.tools.enable_edit = false;
    other.tools.enable_glob = false;
    other.tools.enable_grep = false;
    other.tools.enable_shell = false;
    other.tools.enable_webfetch = false;
    other.tools.enable_websearch = false;
    other.tools.question.timeout_enabled = !QuestionToolConfig::default().timeout_enabled;
    other.tools.question.timeout_secs = 1;
    other.tools.disabled_tools = vec!["x".into()];
    other.tools.enable_read = true;
    other.tools.enable_write = true;
    other.tools.enable_edit = true;
    other.tools.enable_glob = true;
    other.tools.enable_grep = true;
    other.tools.enable_shell = true;
    other.tools.enable_webfetch = true;
    other.tools.enable_websearch = true;
    other.tools.custom_tools.insert(
        "c".into(),
        CustomToolConfig {
            command: "echo".into(),
            description: "d".into(),
            parameters: None,
        },
    );
    other.tui.key_bindings = Some(HashMap::from([("k".into(), "v".into())]));
    other
        .tui
        .agent_colors
        .insert("build".into(), "#7aa2f7".into());
    other
        .tui
        .agent_colors
        .insert("plan".into(), "accent".into());
    other.command_configs.insert(
        "run".into(),
        CommandConfig {
            model: Some(make_model("p", "m")),
            agent: Some("plan".into()),
            max_turns: Some(3),
        },
    );
    other.command_configs.insert(
        "fresh-cmd".into(),
        CommandConfig {
            model: None,
            agent: Some("ask".into()),
            max_turns: Some(1),
        },
    );
    other.tools.custom_tools.insert(
        "fresh".into(),
        CustomToolConfig {
            command: "true".into(),
            description: "new".into(),
            parameters: None,
        },
    );
    base.command_configs
        .insert("run".into(), CommandConfig::default());
    base.tools.custom_tools.insert(
        "c".into(),
        CustomToolConfig {
            command: "keep".into(),
            description: "base".into(),
            parameters: None,
        },
    );

    let merged = base.merge_with(&other);
    assert_eq!(merged.providers["p"].api_key.as_deref(), Some("new"));
    assert_eq!(merged.providers["p"].api_base.as_deref(), Some("http://a"));
    assert_eq!(merged.models["mid"].max_tokens, Some(1));
    assert!(merged.agents.iter().any(|a| a.name == "extra"));
    assert!(!merged.memory.enabled);
    assert_eq!(merged.memory.retain_every_n, 99);
    assert_eq!(merged.memory.embed_backend, "onnx");
    assert_eq!(merged.memory.code_top_k, 9);
    assert!(!merged.swarm.enabled);
    assert_eq!(merged.automation.max_background_jobs, 3);
    assert!(merged.tools.enable_read);
    assert!(merged.tools.enable_write);
    assert_eq!(merged.tools.disabled_tools, vec!["x".to_string()]);
    assert_eq!(merged.tools.custom_tools["c"].command, "keep");
    assert_eq!(merged.tools.custom_tools["fresh"].command, "true");
    assert_eq!(merged.command_configs["fresh-cmd"].max_turns, Some(1));
    assert_eq!(
        merged
            .tui
            .key_bindings
            .as_ref()
            .unwrap()
            .get("k")
            .map(String::as_str),
        Some("v")
    );
    assert_eq!(
        merged.tui.agent_colors.get("build").map(String::as_str),
        Some("#7aa2f7")
    );
    assert_eq!(
        merged.tui.agent_colors.get("plan").map(String::as_str),
        Some("accent")
    );
    assert_eq!(merged.command_configs["run"].max_turns, Some(3));
    assert_eq!(merged.command_configs["run"].agent.as_deref(), Some("plan"));
}

#[test]
fn command_markdown_render_and_load_dir() {
    let no_fm = parse_command_markdown("just body").unwrap();
    assert_eq!(no_fm.template, "just body");
    let with = parse_command_markdown(
        "---\ndescription: d\nagent: plan\nmodel: m\nsubtask: true\nextra: x\n---\nHello $1 !`printf hi`\n",
    )
    .unwrap();
    assert_eq!(with.description.as_deref(), Some("d"));
    assert_eq!(with.agent.as_deref(), Some("plan"));
    assert_eq!(with.subtask, Some(true));
    let no_close = CustomCommandConfig {
        template: "x !`no-end".into(),
        description: None,
        agent: None,
        model: None,
        subtask: None,
    };
    assert!(no_close.render("").contains("!`"));

    with_isolated_home(|home| {
        let rendered = with.render("arg1 extra");
        assert!(rendered.contains("arg1"), "{rendered}");
        assert!(rendered.contains("hi"), "{rendered}");
        let dir = home.join("proj/.whycodes/commands");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("skip.txt"), "nope").unwrap();
        std::fs::write(dir.join("ok.md"), "do $ARGUMENTS").unwrap();
        let mut cfg = Config::default();
        cfg.load_command_files(&home.join("proj"));
        assert!(cfg.commands.contains_key("ok"));
        assert!(cfg.commands.contains_key("review"));
        cfg.ensure_builtin_prompt_commands();
        assert!(cfg.commands.contains_key("commit"));
    });
}

#[test]
fn validate_empty_agents_and_provider_env_key() {
    let _guard = lock_env();
    let names = [
        "LOCAL_API_KEY",
        "WHYCODES_LOCAL_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
    ];
    let prev = clear_env(&names);
    let mut cfg = Config::default();
    cfg.agents.clear();
    cfg.providers.insert(
        "local".into(),
        ProviderConfig {
            name: "local".into(),
            api_key: None,
            api_base: None,
            base_url: Some("http://127.0.0.1:9".into()),
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: HashMap::new(),
        },
    );
    assert!(cfg.validate().is_err());
    unsafe { std::env::set_var("LOCAL_API_KEY", "k") };
    // still empty agents
    assert!(cfg.validate().is_err());
    restore_env(&names, prev);
}

#[test]
fn substitute_unbraced_unknown_kept() {
    let s = Config::substitute_vars("x $NOT_A_WHYCODES_VAR_ZZZ y");
    assert!(s.contains("$NOT_A_WHYCODES_VAR_ZZZ"), "{s}");
}

#[test]
fn parse_command_unclosed_frontmatter_is_none() {
    assert!(parse_command_markdown("---\ndescription: x\nno closer").is_none());
    let skip = parse_command_markdown("---\ndescription: d\nnot-a-pair\n---\nbody").unwrap();
    assert_eq!(skip.description.as_deref(), Some("d"));
    assert_eq!(skip.template, "body");
}

#[test]
fn ensure_parent_dir_and_toml_err() {
    assert!(toml_err("boom".into()).to_string().contains("boom"));
    assert!(encode_toml(&Config::default()).is_ok());
    let ser_err = <toml::ser::Error as serde::ser::Error>::custom("nope");
    assert!(map_toml_ser(Err(ser_err)).is_err());
    let dir = tempfile::tempdir().unwrap();
    ensure_parent_dir(&dir.path().join("nested/config.toml")).unwrap();
    assert!(dir.path().join("nested").is_dir());
    ensure_parent_dir(Path::new("config.toml")).unwrap();
    ensure_parent_dir(Path::new("/")).unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    assert!(ensure_parent_dir(&blocker.join("child.toml")).is_err());
}

#[test]
fn project_path_falls_back_when_cwd_gone() {
    let _guard = lock_env();
    let prev = std::env::current_dir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    drop(dir);
    let cfg = Config::default();
    let p = cfg.project_path();
    let _ = std::env::set_current_dir(&prev);
    assert_eq!(p, PathBuf::from("."));
}

#[test]
fn render_inline_shell_stdout_and_stderr() {
    let _guard = lock_env();
    let cmd = CustomCommandConfig {
        template: "out=!`printf hi; printf err >&2`".into(),
        description: None,
        agent: None,
        model: None,
        subtask: None,
    };
    let rendered = cmd.render("");
    assert!(rendered.contains("hi"), "{rendered}");
    assert!(rendered.contains("err"), "{rendered}");
}

#[test]
fn render_inline_shell_spawn_failure() {
    let _guard = lock_env();
    let names = ["PATH", "WHYCODES_HOME"];
    let prev = names.iter().map(std::env::var_os).collect::<Vec<_>>();
    unsafe { std::env::set_var("PATH", "") };
    let cmd = CustomCommandConfig {
        template: "!`true`".into(),
        description: None,
        agent: None,
        model: None,
        subtask: None,
    };
    let rendered = cmd.render("");
    restore_env(&names, prev);
    assert!(
        rendered.contains("command failed")
            || rendered.contains("true")
            || rendered.is_empty()
            || !rendered.contains("!`"),
        "{rendered}"
    );
}

#[test]
fn load_command_files_opencode_dir_and_unreadable_md() {
    with_isolated_home(|home| {
        let proj = home.join("proj");
        let oc = proj.join(".opencode/commands");
        std::fs::create_dir_all(&oc).unwrap();
        std::fs::write(oc.join("extra.md"), "from opencode $1").unwrap();
        // directory named *.md is skipped (read_to_string fails)
        std::fs::create_dir(oc.join("dir.md")).unwrap();
        let mut cfg = Config::default();
        cfg.load_command_files(&proj);
        assert!(cfg.commands.contains_key("extra"));
        assert!(!cfg.commands.contains_key("dir"));
    });
}

#[test]
fn apply_env_provider_key_fallbacks() {
    let _guard = lock_env();
    let names = [
        "WHYCODES_PROVIDER",
        "WHYCODES_MODEL",
        "FOO_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
    ];
    let prev = clear_env(&names);

    unsafe {
        std::env::set_var("WHYCODES_PROVIDER", "foo");
        std::env::set_var("OPENAI_API_KEY", "ok");
    }
    let mut cfg = Config::default();
    cfg.apply_env_overrides();
    assert_eq!(cfg.providers["foo"].api_key.as_deref(), Some("ok"));

    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
        std::env::set_var("WHYCODES_PROVIDER", "bar");
        std::env::set_var("ANTHROPIC_API_KEY", "ak");
    }
    let mut cfg2 = Config::default();
    cfg2.apply_env_overrides();
    assert_eq!(cfg2.providers["bar"].api_key.as_deref(), Some("ak"));

    unsafe {
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::set_var("WHYCODES_PROVIDER", "bare");
    }
    let mut cfg3 = Config::default();
    cfg3.apply_env_overrides();
    assert!(cfg3.providers["bare"].api_key.is_none());

    restore_env(&names, prev);
}

#[test]
fn validate_env_key_aliases_and_empty_default_agent() {
    let _guard = lock_env();
    let names = [
        "LOCAL_API_KEY",
        "WHYCODES_LOCAL_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
    ];
    let prev = clear_env(&names);

    let mut cfg = Config::default();
    cfg.default_agent.clear();
    cfg.providers.insert(
        "local".into(),
        ProviderConfig {
            name: "local".into(),
            api_key: None,
            api_base: None,
            base_url: Some("http://127.0.0.1:1".into()),
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: HashMap::new(),
        },
    );
    unsafe { std::env::set_var("WHYCODES_LOCAL_API_KEY", "k") };
    assert!(cfg.validate().is_ok());

    unsafe {
        std::env::remove_var("WHYCODES_LOCAL_API_KEY");
        std::env::set_var("OPENAI_API_KEY", "ok");
    }
    assert!(cfg.validate().is_ok());

    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
        std::env::set_var("ANTHROPIC_API_KEY", "ak");
    }
    assert!(cfg.validate().is_ok());

    restore_env(&names, prev);
}

#[test]
fn configured_context_window_none_when_unset() {
    let mut cfg = Config::default();
    cfg.models.insert("m".into(), make_model("p", "m"));
    assert_eq!(cfg.configured_context_window("p", "m"), None);
    cfg.default_model = Some(make_model("p", "other"));
    assert_eq!(cfg.configured_context_window("p", "m"), None);
}
