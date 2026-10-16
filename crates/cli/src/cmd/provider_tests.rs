use super::*;

#[test]
fn default_config_has_named_agents() {
    let cfg = whycodes_config::Config::default();
    assert!(cfg.get_agent("build").is_some());
    assert!(cfg.get_agent("plan").is_some());
}

#[test]
fn provider_printer_helpers() {
    assert_eq!(provider_key_status(true), "set");
    assert_eq!(provider_key_status(false), "not set");
    assert_eq!(
        provider_base_url(Some("https://a"), Some("https://b")),
        "https://a"
    );
    assert_eq!(provider_base_url(None, Some("https://b")), "https://b");
    assert_eq!(provider_base_url(None, None), "(default)");
    assert!(provider_add_summary(true).contains("API key"));
    assert!(provider_add_summary(false).contains("no API key"));
    let headers = parse_provider_headers("A=1, B=2, skip, C=");
    assert_eq!(headers.get("A").map(String::as_str), Some("1"));
    assert_eq!(headers.get("B").map(String::as_str), Some("2"));
    assert!(!headers.contains_key("skip"));
    assert!(headers.contains_key("C"));
    assert!(agent_default_marker("build", "build").contains("default"));
    assert!(agent_default_marker("plan", "build").is_empty());
    assert!(provider_updating_line("acme").contains("already exists"));
    assert!(provider_saved_line("acme", "added with API key").contains("acme"));
    assert!(provider_removed_line("acme").contains("removed"));
    assert!(provider_not_found_line("acme").contains("not found"));
    assert!(provider_default_set_line("acme").contains("Default"));
    assert!(provider_default_use_line("acme").contains("-P acme"));
    assert!(provider_default_missing_line("acme").contains("provider add"));
    assert!(model_default_set_line("openai", "gpt").contains("openai"));
    assert!(agent_not_found_line("plan").contains("plan"));
    let none = provider_none_lines("anthropic, openai");
    assert!(none.iter().any(|l| l.contains("No providers")));
    assert!(none.iter().any(|l| l.contains("anthropic, openai")));
    assert!(provider_list_header().contains("Configured"));
    let models = model_none_lines(Some("/tmp/c.toml"));
    assert!(models.iter().any(|l| l.contains("/tmp/c.toml")));
    assert!(
        model_none_lines(None)
            .iter()
            .any(|l| l.contains("No models"))
    );
    assert!(model_list_header().contains("Configured"));
    let plugins = plugins_empty_lines();
    assert!(plugins.iter().any(|l| l.contains("No shell plugins")));
    assert!(plugins_header(2).contains("2"));
    assert!(no_agents_configured_line().contains("no agents"));
    let empty = empty_provider_lines();
    assert!(empty.iter().any(|l| l.contains("No providers")));
}

fn model(id: &str, max_tokens: Option<u32>) -> whycodes_core::types::ModelConfig {
    whycodes_core::types::ModelConfig {
        model_id: id.into(),
        provider_id: "baseonly".into(),
        max_tokens,
        context_window: None,
        temperature: None,
        top_p: None,
        thinking: None,
        supports_tools: None,
        supports_images: None,
    }
}

/// `api_base` without `base_url`, a model that prints `max_tokens`, and an
/// empty agent table. The printer helpers do not walk these arms.
#[tokio::test]
async fn provider_list_api_base_model_cap_and_empty_agents() {
    let _home = IsolatedHome::new();
    let mut cfg = whycodes_config::Config::default();
    cfg.providers.insert(
        "baseonly".into(),
        whycodes_core::types::ProviderConfig {
            name: "baseonly".into(),
            api_key: None,
            api_base: Some("https://api.example".into()),
            base_url: None,
            headers: None,
            models: vec![String::new(), "listed-model".into()],
            tool_arguments: None,
            extra: Default::default(),
            credentials: Default::default(),
        },
    );
    cfg.models
        .insert("alias".into(), model("m-with-cap", Some(64)));
    cfg.models.insert("blank-id".into(), model("", None));
    cfg.agents.clear();
    cfg.save().unwrap();

    cfg.agents.clear();
    cfg.save().unwrap();

    cmd_provider(&ProviderCmd::List).await.unwrap();
    cmd_model(&ModelCmd::List).await.unwrap();
    cmd_agent(None).await.unwrap();
    cmd_agent(Some("missing")).await.unwrap();
}

/// Allow/deny lists and a pinned model are printed only for a named agent.
/// An unknown name still lists the remaining agents.
#[tokio::test]
async fn cmd_agent_prints_tool_lists_and_model() {
    let _home = IsolatedHome::new();
    let mut cfg = whycodes_config::Config::default();
    cfg.agents.clear();
    cfg.agents.push(whycodes_core::types::AgentInfo {
        name: "locked".into(),
        description: "restricted".into(),
        mode: whycodes_core::types::AgentMode::Primary,
        permission: whycodes_core::types::PermissionSet {
            allowed_tools: Some(vec!["read".into()]),
            denied_tools: Some(vec!["bash".into()]),
            allow_file_writes: false,
            allow_network: false,
            allow_shell: false,
            allowed_paths: None,
            rules: Default::default(),
        },
        model: Some(model("tiny", None)),
        system_prompt: None,
        temperature: None,
        top_p: None,
    });
    cfg.save().unwrap();

    cmd_agent(Some("locked")).await.unwrap();
    cmd_agent(Some("missing")).await.unwrap();
}

fn local_server(provider: &str, models: &[&str]) -> whycodes_llm::LocalServer {
    whycodes_llm::LocalServer {
        provider: provider.into(),
        label: "Local".into(),
        api_base: "http://127.0.0.1:1/v1".into(),
        models: models.iter().map(|m| m.to_string()).collect(),
    }
}

#[test]
fn provider_discover_lines_text_json_and_empty() {
    let found = vec![
        local_server("lmstudio", &["qwen", "llama"]),
        local_server("vllm", &[]),
    ];
    let text = provider_discover_lines(&found, false).unwrap();
    assert!(text[0].contains("Local model servers"));
    assert!(text.iter().any(|l| l.contains("lmstudio")));
    assert!(text.iter().any(|l| l.trim() == "qwen"));
    assert!(text.iter().any(|l| l.contains("no models loaded")));

    let json = provider_discover_lines(&found, true).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json[0]).unwrap();
    assert_eq!(parsed[0]["provider"], "lmstudio");
    assert_eq!(parsed[0]["models"][1], "llama");
    assert_eq!(parsed[1]["api_base"], "http://127.0.0.1:1/v1");

    let none = provider_discover_lines(&[], false).unwrap();
    assert!(none[0].contains("No local model servers"));
    assert!(none[0].contains("LM Studio :1234"));
    assert!(none[1].contains("--save"));
    assert_eq!(provider_discover_lines(&[], true).unwrap(), vec!["[]"]);
}

#[test]
fn save_discovered_providers_never_overwrites_configured_blocks() {
    let mut config = whycodes_config::Config::default();
    let (lines, changed) =
        save_discovered_providers(&mut config, &[local_server("llamacpp", &["m"])]);
    assert!(changed);
    assert!(lines[0].contains("saved (api_base, no key)"));
    let pc = config.get_provider("llamacpp").unwrap();
    assert_eq!(pc.api_base.as_deref(), Some("http://127.0.0.1:1/v1"));
    assert!(pc.api_key.is_none());
    assert_eq!(pc.models, vec!["m".to_string()]);

    // Same api_base again: left as is (user edits to the block survive).
    config.providers.get_mut("llamacpp").unwrap().api_key = Some("mine".into());
    let (lines, changed) =
        save_discovered_providers(&mut config, &[local_server("llamacpp", &["other"])]);
    assert!(!changed);
    assert!(lines[0].contains("left as is"), "{}", lines[0]);
    let pc = config.get_provider("llamacpp").unwrap();
    assert_eq!(pc.api_key.as_deref(), Some("mine"));
    assert_eq!(pc.models, vec!["m".to_string()]);

    // Different api_base: skipped, block untouched.
    config.providers.get_mut("llamacpp").unwrap().api_base = Some("http://gpu:8080/v1".into());
    let (lines, changed) = save_discovered_providers(&mut config, &[local_server("llamacpp", &[])]);
    assert!(!changed);
    assert!(
        lines[0].contains("different api_base (http://gpu:8080/v1); skipped"),
        "{}",
        lines[0]
    );
    assert_eq!(
        config.get_provider("llamacpp").unwrap().api_base.as_deref(),
        Some("http://gpu:8080/v1")
    );

    // No base configured: only api_base is filled in.
    config.providers.get_mut("llamacpp").unwrap().api_base = None;
    let (lines, changed) = save_discovered_providers(&mut config, &[local_server("llamacpp", &[])]);
    assert!(changed);
    assert!(lines[0].contains("api_base filled in, rest kept"));
    let pc = config.get_provider("llamacpp").unwrap();
    assert_eq!(pc.api_base.as_deref(), Some("http://127.0.0.1:1/v1"));
    assert_eq!(pc.api_key.as_deref(), Some("mine"));
}

#[tokio::test]
async fn cmd_provider_discover_runs_offline() {
    let _home = IsolatedHome::new();
    cmd_provider(&ProviderCmd::Discover {
        json: true,
        save: true,
    })
    .await
    .unwrap();
    cmd_provider(&ProviderCmd::Discover {
        json: false,
        save: false,
    })
    .await
    .unwrap();
}

#[test]
fn provider_discover_run_saves_and_keeps_json_clean() {
    let _home = IsolatedHome::new();
    let found = vec![local_server("vllm", &["m"])];

    let mut config = whycodes_config::Config::default();
    let json = provider_discover_run(&mut config, &found, true, true).unwrap();
    assert_eq!(json.len(), 1, "--json --save prints only the JSON");
    assert!(config.get_provider("vllm").is_some());

    let mut config = whycodes_config::Config::default();
    let text = provider_discover_run(&mut config, &found, false, true).unwrap();
    assert!(text.last().unwrap().contains("vllm"));
    assert!(text.last().unwrap().contains("saved (api_base, no key)"));
    let on_disk = whycodes_config::Config::load().unwrap();
    assert!(on_disk.get_provider("vllm").unwrap().api_key.is_none());

    // Without --save (or with nothing found) the config is untouched.
    let mut config = whycodes_config::Config::default();
    provider_discover_run(&mut config, &found, false, false).unwrap();
    provider_discover_run(&mut config, &[], false, true).unwrap();
    assert!(config.get_provider("vllm").is_none());
}
