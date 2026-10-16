use super::*;
use std::sync::Arc;
use whycodes_core::types::{ImageSource, Role, ToolDefinition};

const KEY: [u8; 32] = [7u8; 32];

fn redactor() -> Redactor {
    Redactor::with_key(KEY)
}

fn placeholder(text: &str) -> bool {
    text.starts_with(PLACEHOLDER_PREFIX) && text.ends_with(PLACEHOLDER_SUFFIX) && text.len() == 27
}

#[test]
fn placeholders_are_stable_per_key_and_differ_across_keys() {
    let a = redactor();
    let b = Redactor::with_key([9u8; 32]);
    let p1 = a.register("hunter2hunter2");
    let p2 = a.register("hunter2hunter2");
    assert_eq!(p1, p2, "idempotent");
    assert!(placeholder(&p1), "{p1}");
    assert_ne!(p1, b.register("hunter2hunter2"), "keyed");
    assert_ne!(p1, a.register("another-secret-value"));
    assert_eq!(a.len(), 2);
    assert!(!a.is_empty());
    assert!(b.register("x").len() == 27);
    assert!(format!("{a:?}").contains("known: 2"));
}

#[test]
fn env_registration_filters_names_and_values() {
    let r = redactor();
    let vars = [
        ("OPENAI_API_KEY", "sk-live-0123456789"),
        ("DB_PASSWORD", "p@ssw0rd-long"),
        ("GITHUB_TOKEN", "long-enough-token-value"),
        ("MY_CREDENTIAL", "cred-value-1"),
        ("SOME_PASSWD", "swordfish99"),
        ("PATH", "/usr/bin:/bin"),
        ("KEYBOARD", "us"),
        ("PUBLIC_KEY_FILE", "/home/u/.ssh/id.pub"),
        ("WIN_KEY_PATH", "C:\\keys\\k.pem"),
        ("HOME_KEY", "~/secret"),
        ("DOT_KEY", "./relative"),
        ("SPACED_TOKEN", "two words here"),
        ("SHORT_TOKEN", "abc"),
        ("TOKEN_WITH_TRIM", "  padded-secret  "),
    ];
    let n = r.register_env(
        vars.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    );
    assert_eq!(n, 6, "{:?}", r);
    assert!(r.redact_text("use sk-live-0123456789 now").is_some());
    assert!(r.redact_text("/usr/bin:/bin").is_none());
    assert!(
        r.redact_text("padded-secret").is_some(),
        "values are trimmed"
    );
    assert!(looks_like_env_secret("abcdefgh"));
    assert!(!looks_like_env_secret("D:"));
}

#[test]
fn redact_text_handles_known_secrets_longest_first_and_patterns() {
    let r = redactor();
    r.register("abc12345");
    r.register("abc12345-extended");
    let out = r.redact_text("x abc12345-extended y abc12345 z").unwrap();
    assert!(!out.contains("abc12345"), "{out}");
    assert_eq!(out.matches(PLACEHOLDER_PREFIX).count(), 2);

    // Token-shaped fixtures are assembled at runtime so the tracked-secrets
    // ratchet does not mistake them for live credentials.
    let alpha = "abcdefghijklmnopqrstuvwxyz";
    let github = format!("gh{}_{alpha}0123", "p");
    let pat = format!("github_{}_{alpha}", "pat");
    let slack = format!("xox{}-123456789012-abc", "b");
    let aws = format!("AK{}ABCDEFGHIJKLMNOP", "IA");
    let google = format!("AI{}SyAbCdEfGhIjKlMnOpQrStUvWxYz0123456", "za");
    let pem_head = format!("-----BEGIN RSA {} KEY-----", "PRIVATE");
    let pem = format!(
        "{pem_head}
MIIE
-----END RSA {} KEY-----",
        "PRIVATE"
    );
    let samples: Vec<(String, &str)> = vec![
        (
            format!("key sk-{alpha} done"),
            "sk-abcdefghijklmnopqrstuvwxyz",
        ),
        (github.clone(), github.as_str()),
        (pat.clone(), pat.as_str()),
        (slack.clone(), slack.as_str()),
        (aws.clone(), aws.as_str()),
        (google.clone(), google.as_str()),
        (pem.clone(), pem_head.as_str()),
        (
            "postgres://user:s3cretpw@db.example.com/app".into(),
            "s3cretpw",
        ),
        (format!("Authorization: Bearer {alpha}"), alpha),
        ("API_KEY=verysecretvalue123".into(), "verysecretvalue123"),
        (
            "password: \"hunter2hunter2hunter2\"".into(),
            "hunter2hunter2hunter2",
        ),
    ];
    for (text, secret) in &samples {
        let out = r
            .redact_text(text)
            .unwrap_or_else(|| panic!("no redaction for {text:?}"));
        assert!(!out.contains(secret), "{text:?} -> {out}");
        assert!(out.contains(PLACEHOLDER_PREFIX), "{out}");
    }
    // The URL keeps its user and host; only the password moves.
    let url = r
        .redact_text("postgres://user:s3cretpw@db.example.com/app")
        .unwrap();
    assert!(url.starts_with("postgres://user:$$SECRET:"), "{url}");
    assert!(url.ends_with("@db.example.com/app"), "{url}");

    // Plain text and an existing placeholder are left alone.
    assert!(r.redact_text("nothing secret here").is_none());
    let already = r.register(&format!("gh{}_{}", "p", "z".repeat(30)));
    assert!(
        r.redact_text(&format!("token: {already}")).is_none(),
        "placeholder is not re-redacted"
    );
}

#[test]
fn request_round_trips_through_redact_and_restore() {
    let r = redactor();
    r.register("TOPSECRET-VALUE-1");
    let messages: Vec<Message> = vec![
        Message {
            role: Role::User,
            content: MessageContent::Text("my key is TOPSECRET-VALUE-1".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        },
        Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: "plain".into(),
                },
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "curl -H 'x: TOPSECRET-VALUE-1'", "n": 1, "list": ["TOPSECRET-VALUE-1", 2]}),
                },
            ]),
            tool_call_id: None,
            name: None,
            created_at: None,
        },
        Message {
            role: Role::Tool,
            content: MessageContent::Blocks(vec![
                ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "env: API_KEY=anotherlongsecret42".into(),
                    is_error: Some(false),
                },
                ContentBlock::Image {
                    source: ImageSource::Base64 {
                        media_type: "image/png".into(),
                        data: "TOPSECRET-VALUE-1".into(),
                    },
                },
            ]),
            tool_call_id: Some("t1".into()),
            name: None,
            created_at: None,
        },
        Message {
            role: Role::Assistant,
            content: MessageContent::Text("no secrets".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        },
    ];
    let request = LlmRequest {
        system: "system has TOPSECRET-VALUE-1".into(),
        messages: messages.into(),
        tools: Arc::from(Vec::<ToolDefinition>::new()),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: true,
    };
    let redacted = r.redact_request(&request);
    let dump = serde_json::to_string(&redacted.messages).unwrap() + &redacted.system;
    assert!(
        !dump.contains("TOPSECRET-VALUE-1") || dump.contains("image/png"),
        "{dump}"
    );
    assert!(!redacted.system.contains("TOPSECRET"));
    assert!(
        !dump.contains("anotherlongsecret42"),
        "tool output is redacted"
    );
    assert!(redacted.use_prompt_cache, "other fields are kept");
    // The image block (base64 payload) and the clean message are untouched.
    match &redacted.messages[2].content {
        MessageContent::Blocks(blocks) => assert!(matches!(&blocks[1], ContentBlock::Image { .. })),
        _ => panic!("blocks"),
    }
    assert_eq!(redacted.messages[3].content.as_text(), Some("no secrets"));

    // Restoring the tool-use input brings the secret back, arrays included.
    let mut input = match &redacted.messages[1].content {
        MessageContent::Blocks(blocks) => match &blocks[1] {
            ContentBlock::ToolUse { input, .. } => input.clone(),
            _ => panic!("tool use"),
        },
        _ => panic!("blocks"),
    };
    assert!(r.restore_json(&mut input));
    assert_eq!(input["command"], "curl -H 'x: TOPSECRET-VALUE-1'");
    assert_eq!(input["list"][0], "TOPSECRET-VALUE-1");
    assert!(!r.restore_json(&mut input), "nothing left to restore");

    // A request without secrets keeps its shared message slice.
    let clean = LlmRequest {
        system: "clean".into(),
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text("hello".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]
        .into(),
        ..request.clone()
    };
    let same = r.redact_request(&clean);
    assert!(Arc::ptr_eq(&same.messages, &clean.messages));
    assert_eq!(same.system, "clean");
}

#[test]
fn restore_text_needs_a_known_placeholder() {
    let r = redactor();
    assert!(r.restore_text("no placeholder").is_none());
    assert!(
        r.restore_text("$$SECRET:0000000000000000$$ unknown")
            .is_none()
    );
    let p = r.register("real-secret-value");
    assert_eq!(
        r.restore_text(&format!("use {p} twice {p}")).unwrap(),
        "use real-secret-value twice real-secret-value"
    );
    let mut value = serde_json::json!({"a": [1, {"b": p.clone()}], "c": true, "d": null});
    assert!(r.restore_json(&mut value));
    assert_eq!(value["a"][1]["b"], "real-secret-value");
    assert!(r.redact_json(&serde_json::json!({"n": 1})).is_none());
}

#[test]
fn key_file_round_trips_and_replaces_garbage() {
    let dir = tempfile::tempdir().unwrap();
    let first = Redactor::load_or_create_key(dir.path()).unwrap();
    let again = Redactor::load_or_create_key(dir.path()).unwrap();
    assert_eq!(first, again, "the stored key is reused");
    assert_eq!(
        std::fs::read_to_string(dir.path().join(KEY_FILE))
            .unwrap()
            .len(),
        64
    );

    std::fs::write(dir.path().join(KEY_FILE), "not a key").unwrap();
    let fresh = Redactor::load_or_create_key(dir.path()).unwrap();
    assert_ne!(fresh, first);
    assert_eq!(Redactor::load_or_create_key(dir.path()).unwrap(), fresh);

    // A nested directory is created on demand; a file in the way is an error.
    let nested = dir.path().join("a").join("b");
    assert!(Redactor::load_or_create_key(&nested).is_ok());
    let blocked = dir.path().join("file");
    std::fs::write(&blocked, "x").unwrap();
    assert!(Redactor::load_or_create_key(&blocked).is_err());

    let env = Redactor::from_env(first);
    let _ = env.len();
}

#[test]
fn key_parsing_and_pattern_compilation_edge_cases() {
    assert!(parse_key_hex("abc").is_none());
    assert!(parse_key_hex(&"zz".repeat(32)).is_none());
    assert_eq!(parse_key_hex(&"ff".repeat(32)).unwrap(), [0xffu8; 32]);
    assert_eq!(hex(&[0, 255]), "00ff");
    assert_eq!(compile_patterns(&["(", "ok"]).len(), 1);
    let mac = hmac_sha256(&KEY, b"msg");
    assert_ne!(mac, hmac_sha256(&[8u8; 32], b"msg"));
    assert_eq!(mac, hmac_sha256(&KEY, b"msg"));
}

#[test]
fn poisoned_state_still_answers() {
    let r = Arc::new(redactor());
    let poisoner = Arc::clone(&r);
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.state.lock().unwrap();
        panic!("poison the redactor");
    })
    .join();
    assert_eq!(r.len(), 0);
    assert!(placeholder(&r.register("after-poison-secret")));
}

#[test]
fn process_default_installs_once() {
    // This binary never installs a redactor elsewhere, so the first call wins
    // here. Installing one with no registered secrets keeps the other tests'
    // fixtures untouched (token shapes only).
    let first = Arc::new(Redactor::with_key([5u8; 32]));
    let was_empty = installed().is_none();
    let set = install(Arc::clone(&first));
    assert_eq!(set, was_empty);
    assert!(installed().is_some());
    assert!(
        !install(Arc::new(Redactor::with_key([6u8; 32]))),
        "second install is a no-op"
    );
}
