use super::*;

fn path(key: &str) -> Vec<String> {
    parse_key(key).unwrap()
}

fn value(text: &str) -> Value {
    Value::Table(toml::from_str(text).unwrap())
}

#[test]
fn parse_and_format_keys_round_trip_quoted_segments() {
    assert_eq!(path("session.tool_profile"), ["session", "tool_profile"]);
    let quoted = path("models.\"gpt-4.1\".context_window");
    assert_eq!(quoted, ["models", "gpt-4.1", "context_window"]);
    assert_eq!(format_key(&quoted), "models.\"gpt-4.1\".context_window");
    assert_eq!(format_key(&path("agents.build")), "agents.build");
    let err = parse_key("a..b").unwrap_err().to_string();
    assert!(err.contains("invalid config key 'a..b'"), "{err}");
    assert!(parse_key("").is_err());
}

#[test]
fn lookup_walks_tables_and_picks_array_elements_by_name_or_index() {
    let root = value(
        r#"
default_agent = "build"
[[agents]]
name = "build"
mode = "primary"
[[agents]]
name = "plan"
[session]
tool_profile = "core"
"#,
    );
    assert_eq!(
        lookup(&root, &path("session.tool_profile")),
        Some(&Value::String("core".into()))
    );
    assert_eq!(
        lookup(&root, &path("agents.build.mode")),
        Some(&Value::String("primary".into()))
    );
    assert_eq!(
        lookup(&root, &path("agents.1.name")),
        Some(&Value::String("plan".into()))
    );
    assert_eq!(lookup(&root, &path("agents.nope")), None);
    assert_eq!(lookup(&root, &path("agents.7")), None);
    assert_eq!(lookup(&root, &path("default_agent.x")), None);
    assert_eq!(lookup(&root, &path("missing")), None);
}

#[test]
fn leaves_are_sorted_and_name_array_elements() {
    let root = value(
        r#"
b = 1
empty = {}
list = ["x", "y"]
[a]
c = true
[[hooks]]
event = "x"
[[agents]]
name = "ask"
mode = "primary"
"#,
    );
    let keys: Vec<String> = leaves(&root).iter().map(|(p, _)| format_key(p)).collect();
    assert_eq!(
        keys,
        [
            "a.c",
            "agents.ask.mode",
            "agents.ask.name",
            "b",
            "empty",
            "hooks.0.event",
            "list"
        ]
    );
    let empty_list = value("list = []");
    assert_eq!(leaves(&empty_list)[0].1, Value::Array(vec![]));
}

#[test]
fn secret_keys_follow_the_last_segment_and_headers() {
    for secret in [
        "providers.openai.api_key",
        "providers.openai.key",
        "providers.x.ApiKey",
        "notify.telegram_bot_token",
        "notify.discord_webhook",
        "notify.slack_webhook_url",
        "mcp_servers.gh.env.GITHUB_TOKEN",
        "mcp_servers.gh.headers.X-Anything",
        "websearch.custom.headers.Accept",
        "x.client_secret",
        "db.password",
        "x.authorization",
    ] {
        assert!(is_secret_key(&path(secret)), "{secret}");
    }
    for plain in [
        "tools.key",
        "session.max_context_tokens",
        "notify.telegram_chat_id",
        "websearch.custom.header_env.Authorization_VAR",
        "providers.openai.base_url",
    ] {
        assert!(!is_secret_key(&path(plain)), "{plain}");
    }
    assert!(!is_secret_key(&[]));
}

#[test]
fn redact_hides_secret_strings_and_keeps_env_references() {
    let root = value(
        r#"
[providers.openai]
api_key = "sk-live"
base_url = "https://api.example"
headers = { Authorization = "Bearer x", "X-Ref" = "${HEADER_VAR}" }
[notify]
telegram_bot_token = "${TG_TOKEN}"
discord_webhook = "https://discord.com/api/webhooks/1/abc"
timeout_secs = 5
[tools]
secret = ["a", "b"]
"#,
    );
    let out = redact(&root, &[]);
    let get = |key: &str| lookup(&out, &path(key)).unwrap().clone();
    assert_eq!(get("providers.openai.api_key").as_str(), Some(REDACTED));
    assert_eq!(
        get("providers.openai.base_url").as_str(),
        Some("https://api.example")
    );
    assert_eq!(
        get("providers.openai.headers.Authorization").as_str(),
        Some(REDACTED)
    );
    assert_eq!(
        get("providers.openai.headers.X-Ref").as_str(),
        Some("${HEADER_VAR}")
    );
    assert_eq!(
        get("notify.telegram_bot_token").as_str(),
        Some("${TG_TOKEN}")
    );
    assert_eq!(get("notify.discord_webhook").as_str(), Some(REDACTED));
    assert_eq!(get("notify.timeout_secs").as_integer(), Some(5));
    assert_eq!(
        get("tools.secret"),
        Value::Array(vec![REDACTED.into(), REDACTED.into()])
    );
    // A subtree looked up on its own is judged by its full path.
    let key = Value::String("sk".into());
    assert_eq!(
        redact(&key, &path("providers.p.api_key")).as_str(),
        Some(REDACTED)
    );
    // `${A}${B}`, `${}` and text around a reference are not one reference.
    for text in ["${A}${B}", "${}", "pre${A}"] {
        let v = Value::String(text.into());
        assert_eq!(redact(&v, &path("x.token")).as_str(), Some(REDACTED));
    }
}

#[test]
fn render_prints_strings_bare_and_tables_as_toml() {
    assert_eq!(render(&Value::String("plan".into())), "plan");
    assert_eq!(render(&Value::Integer(3)), "3");
    assert_eq!(render(&value("a = 1")), "a = 1\n");
}

fn set(doc: &str, key: &str, raw: &str) -> Result<String> {
    let mut doc: DocumentMut = doc.parse().unwrap();
    set_in_document(&mut doc, &path(key), raw.parse().unwrap())?;
    Ok(doc.to_string())
}

#[test]
fn set_in_document_changes_only_that_key() {
    let doc = "# top\ndefault_agent = \"build\" # keep me\n\n[session]\ntool_profile = \"core\"\n";
    assert_eq!(
        set(doc, "default_agent", "\"plan\"").unwrap(),
        "# top\ndefault_agent = \"plan\" # keep me\n\n[session]\ntool_profile = \"core\"\n"
    );
    assert_eq!(
        set(doc, "session.intent_guidance", "\"off\"").unwrap(),
        "# top\ndefault_agent = \"build\" # keep me\n\n[session]\ntool_profile = \"core\"\nintent_guidance = \"off\"\n"
    );
    assert_eq!(
        set("", "mcp_servers.gh.command", "\"gh\"").unwrap(),
        "[mcp_servers.gh]\ncommand = \"gh\"\n"
    );
    assert_eq!(
        set("a = { b = 1 }\n", "a.c.d", "2").unwrap(),
        "a = { b = 1 , c = { d = 2 } }\n"
    );
}

#[test]
fn set_in_document_addresses_arrays_of_tables() {
    let doc = "[[agents]]\nname = \"build\"\n\n[[agents]]\nname = \"plan\"\n";
    let by_name = set(doc, "agents.plan.temperature", "0.2").unwrap();
    assert!(
        by_name.ends_with("name = \"plan\"\ntemperature = 0.2\n"),
        "{by_name}"
    );
    let by_index = set(doc, "agents.0.temperature", "0.5").unwrap();
    assert!(
        by_index.contains("name = \"build\"\ntemperature = 0.5\n"),
        "{by_index}"
    );
    let err = set(doc, "agents.nope.temperature", "1").unwrap_err();
    assert!(
        err.to_string().contains("no 'agents' entry named 'nope'"),
        "{err}"
    );
    let err = set(doc, "agents.temperature", "1").unwrap_err();
    assert!(err.to_string().contains("'agents' is not a table"), "{err}");
    let err = set("x = 1\n", "x.y", "1").unwrap_err();
    assert!(err.to_string().contains("'x' is not a table"), "{err}");
    let mut empty = DocumentMut::new();
    let err = set_in_document(&mut empty, &[], toml_edit::Value::from(1)).unwrap_err();
    assert!(err.to_string().contains("empty config key"), "{err}");
}
