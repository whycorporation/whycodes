use super::*;
use serde_json::json;

#[test]
fn normalize_models_url_variants() {
    assert_eq!(
        normalize_models_url("http://gateway.example/v1"),
        "http://gateway.example/v1/models"
    );
    assert_eq!(
        normalize_models_url("http://gateway.example/v1/chat/completions"),
        "http://gateway.example/v1/models"
    );
    assert_eq!(
        normalize_models_url("http://gateway.example/v1/models"),
        "http://gateway.example/v1/models"
    );
    assert_eq!(
        normalize_models_url("http://host:8080"),
        "http://host:8080/v1/models"
    );
}

#[test]
fn catalog_request_requires_config_base_url() {
    use whycodes_config::Config;
    use whycodes_core::types::ProviderConfig;

    let mut cfg = Config::default();
    // No base → no fetch request (do not invent hosts).
    cfg.providers.insert(
        "naked".into(),
        ProviderConfig {
            name: "naked".into(),
            api_key: Some("sk-test".into()),
            api_base: None,
            base_url: None,
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    assert!(catalog_request_from_config(&cfg, "naked", Some("sk-test")).is_none());

    cfg.providers.insert(
        "gw".into(),
        ProviderConfig {
            name: "gw".into(),
            api_key: Some("sk-from-config".into()),
            api_base: None,
            base_url: Some("http://gateway.example/v1".into()),
            headers: Some(HashMap::from([("x-api-key".into(), "header-key".into())])),
            models: vec![],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    let req = catalog_request_from_config(&cfg, "gw", Some("sk-runtime")).unwrap();
    assert_eq!(req.base_url, "http://gateway.example/v1");
    // Config key wins over runtime.
    assert_eq!(req.api_key.as_deref(), Some("sk-from-config"));
    assert_eq!(
        req.headers.get("x-api-key").map(String::as_str),
        Some("header-key")
    );
}

#[test]
fn parse_omniroute_style_models() {
    let json = json!({
        "object": "list",
        "data": [
            {
                "id": "auto/best-coding",
                "context_length": 1_050_000,
                "max_input_tokens": 1_050_000,
                "max_output_tokens": 1_048_576,
            },
            {
                "id": "gpt-4o",
                "context_length": 128_000,
            },
            {
                "id": "no-meta",
            }
        ]
    });
    let cat = parse_models_json(&json, "http://example/v1/models");
    assert_eq!(cat.context_window("auto/best-coding"), Some(1_050_000));
    assert_eq!(cat.context_window("gpt-4o"), Some(128_000));
    assert_eq!(cat.context_window("no-meta"), None);
    assert_eq!(
        cat.max_output_tokens.get("auto/best-coding"),
        Some(&1_048_576)
    );
}

#[test]
fn parse_vllm_max_model_len() {
    let json = json!({
        "data": [{ "id": "meta-llama", "max_model_len": 8192 }]
    });
    let cat = parse_models_json(&json, "u");
    assert_eq!(cat.context_window("meta-llama"), Some(8192));
}

#[test]
fn context_window_from_nested_openrouter() {
    let m = json!({
        "id": "x",
        "top_provider": { "context_length": 200_000 }
    });
    assert_eq!(context_window_from_model_value(&m), Some(200_000));
}

#[test]
fn context_window_for_model_id_exact_and_suffix() {
    let json = json!({
        "data": [
            { "id": "trk/moonshotai/kimi-k3-free", "context_length": 128_000 },
            { "id": "other", "context_length": 1_000 }
        ]
    });
    assert_eq!(
        context_window_for_model_id(&json, "trk/moonshotai/kimi-k3-free"),
        Some(128_000)
    );
    assert_eq!(context_window_for_model_id(&json, "missing"), None);
    // suffix match against the last path segment
    assert_eq!(
        context_window_for_model_id(&json, "kimi-k3-free"),
        Some(128_000)
    );
    assert_eq!(context_window_for_model_id(&json, ""), None);
    let with_empty = json!({
        "data": [
            { "id": "", "context_length": 1 },
            { "id": "org/kimi-k3-free", "context_length": 64_000 }
        ]
    });
    assert_eq!(
        context_window_for_model_id(&with_empty, "kimi-k3-free"),
        Some(64_000)
    );
}

#[test]
fn context_window_from_string_and_float_values() {
    let m = json!({"id": "x", "context_length": "96000"});
    assert_eq!(context_window_from_model_value(&m), Some(96_000));
    let m = json!({"id": "x", "max_tokens": 4096.5});
    assert_eq!(context_window_from_model_value(&m), Some(4096));
    let m = json!({"id": "x", "context_length": -5});
    assert_eq!(context_window_from_model_value(&m), None);
    let m = json!({"id": "x", "context_length": 0});
    assert_eq!(context_window_from_model_value(&m), None);
    let m = json!({"id": "x", "context_length": "not-a-number"});
    assert_eq!(context_window_from_model_value(&m), None);
}

#[test]
fn context_window_from_architecture_nested() {
    let m = json!({
        "id": "x",
        "architecture": { "context_length": 32_768 }
    });
    assert_eq!(context_window_from_model_value(&m), Some(32_768));
    // top_provider wins over architecture when both present
    let m = json!({
        "id": "x",
        "top_provider": { "context_length": 200_000 },
        "architecture": { "context_length": 32_768 }
    });
    assert_eq!(context_window_from_model_value(&m), Some(200_000));
}

#[test]
fn parse_models_json_skips_empty_ids_and_handles_top_level_array() {
    let json = json!([
        { "id": "", "context_length": 1 },
        { "id": "m1", "max_completion_tokens": 8_192 }
    ]);
    let cat = parse_models_json(&json, "u");
    assert!(cat.context_windows.is_empty());
    assert_eq!(cat.max_output_tokens.get("m1"), Some(&8_192));
    assert_eq!(cat.source_url, "u");
}

#[test]
fn parse_models_json_empty_body() {
    let cat = parse_models_json(&json!({}), "u");
    assert!(cat.is_empty());
    assert_eq!(cat.context_window("anything"), None);
}

#[test]
fn catalog_context_window_suffix_lookup() {
    let mut cat = ModelCatalog::default();
    cat.context_windows
        .insert("trk/vendor/gpt-5".to_string(), 400_000);
    // bare id resolves to the `…/gpt-5` entry
    assert_eq!(cat.context_window("gpt-5"), Some(400_000));
    assert_eq!(cat.context_window("vendor/gpt-5"), Some(400_000));
    assert_eq!(cat.context_window("other"), None);
}

#[test]
fn catalog_is_stale_without_or_after_ttl() {
    let cat = ModelCatalog::default();
    assert!(
        cat.is_stale(Duration::from_secs(60)),
        "never fetched = stale"
    );

    let cat = ModelCatalog {
        fetched_at: Some(Instant::now()),
        ..Default::default()
    };
    assert!(!cat.is_stale(Duration::from_secs(60)));
    assert!(cat.is_stale(Duration::from_millis(0)));
}

#[test]
fn base_url_from_provider_config_prefers_base_url() {
    use whycodes_core::types::ProviderConfig;
    let pc = ProviderConfig {
        name: "p".into(),
        api_key: None,
        api_base: Some("http://api.example/v1".into()),
        base_url: Some("http://direct.example/v1".into()),
        headers: None,
        models: vec![],
        tool_arguments: None,
        extra: Default::default(),
    };
    assert_eq!(
        base_url_from_provider_config(&pc).as_deref(),
        Some("http://direct.example/v1")
    );
    let pc = ProviderConfig {
        base_url: None,
        api_base: Some("  http://api.example/v1  ".into()),
        ..pc
    };
    assert_eq!(
        base_url_from_provider_config(&pc).as_deref(),
        Some("http://api.example/v1"),
        "api_base trimmed as fallback"
    );
    let pc = ProviderConfig {
        base_url: Some("   ".into()),
        api_base: None,
        ..pc
    };
    assert_eq!(
        base_url_from_provider_config(&pc),
        None,
        "blank url rejected"
    );
}

#[test]
fn normalize_models_url_edge_cases() {
    assert_eq!(
        normalize_models_url("http://host/v1/chat/completions/"),
        "http://host/v1/models"
    );
    assert_eq!(
        normalize_models_url("http://host/v1/custom/path"),
        "http://host/v1/custom/path/models"
    );
    assert_eq!(
        normalize_models_url("  http://host/v1  "),
        "http://host/v1/models"
    );
}

#[test]
fn context_window_field_priority_and_fallbacks() {
    let m = json!({
        "context_length": 64_000,
        "context_window": 32_000,
        "max_model_len": 16_000,
        "max_input_tokens": 8_000,
        "max_tokens": 4_000,
        "top_provider": { "context_length": 2_000 },
        "architecture": { "context_length": 1_000 }
    });
    assert_eq!(context_window_from_model_value(&m), Some(64_000));

    let m = json!({
        "context_length": 0,
        "context_window": "bad",
        "max_model_len": 16_000,
        "max_input_tokens": 8_000
    });
    assert_eq!(context_window_from_model_value(&m), Some(16_000));

    let m = json!({
        "top_provider": { "context_length": 0 },
        "architecture": { "context_length": "32768" }
    });
    assert_eq!(context_window_from_model_value(&m), Some(32_768));
}

#[test]
fn numeric_parsing_rejects_overflow_and_non_positive_values() {
    for value in [
        json!(u64::from(u32::MAX) + 1),
        json!(-1),
        json!(0.0),
        json!(u32::MAX as f64),
        json!(null),
        json!(true),
        json!("4294967296"),
    ] {
        assert_eq!(
            context_window_from_model_value(&json!({"context_length": value})),
            None,
            "unexpectedly accepted {value}"
        );
    }
}

#[test]
fn parse_models_uses_completion_alias_and_rejects_zero_output() {
    let cat = parse_models_json(
        &json!({"data": [
            {"id": "primary", "max_output_tokens": 2048, "max_completion_tokens": 1024},
            {"id": "alias", "max_completion_tokens": "4096"},
            {"id": "zero", "max_output_tokens": 0},
            {"context_length": 8192},
            "not-an-object"
        ]}),
        "source",
    );

    assert_eq!(cat.max_output_tokens.get("primary"), Some(&2_048));
    assert_eq!(cat.max_output_tokens.get("alias"), Some(&4_096));
    assert!(!cat.max_output_tokens.contains_key("zero"));
    assert_eq!(cat.max_output_tokens.len(), 2);
    assert!(cat.fetched_at.is_some());
}

#[test]
fn exact_model_id_wins_over_an_earlier_suffix_alias() {
    let json = json!({"data": [
        {"id": "provider/model", "context_length": 8_192},
        {"id": "model", "context_length": 32_768}
    ]});
    assert_eq!(context_window_for_model_id(&json, "model"), Some(32_768));
}

#[test]
fn model_lookup_skips_invalid_candidates_and_keeps_first_suffix() {
    let json = json!([
        {"id": "model", "context_length": 0},
        {"id": "first/model", "context_length": 8_192},
        {"id": "second/model", "context_length": 16_384}
    ]);
    assert_eq!(context_window_for_model_id(&json, "model"), Some(8_192));
    assert_eq!(
        context_window_for_model_id(&json!({"data": {}}), "model"),
        None
    );
}

#[test]
fn catalog_request_filters_unknown_provider_and_uses_runtime_fallback() {
    use whycodes_config::Config;
    use whycodes_core::types::ProviderConfig;

    let mut cfg = Config::default();
    cfg.providers.insert(
        "configured".into(),
        ProviderConfig {
            name: "configured".into(),
            api_key: Some("   ".into()),
            api_base: Some(" http://gateway.example/v1 ".into()),
            base_url: None,
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: Default::default(),
        },
    );

    assert!(catalog_request_from_config(&cfg, "unknown", Some("runtime")).is_none());
    let req = catalog_request_from_config(&cfg, "configured", Some(" runtime-key "))
        .expect("configured provider should produce a request");
    assert_eq!(req.provider_name, "configured");
    assert_eq!(req.base_url, "http://gateway.example/v1");
    assert_eq!(req.api_key.as_deref(), Some("runtime-key"));
    assert!(req.headers.is_empty());
}

#[test]
fn base_url_does_not_fall_through_when_present_value_is_blank() {
    use whycodes_core::types::ProviderConfig;

    let pc = ProviderConfig {
        name: "p".into(),
        api_key: None,
        api_base: Some("http://fallback.example/v1".into()),
        base_url: Some("   ".into()),
        headers: None,
        models: vec![],
        tool_arguments: None,
        extra: Default::default(),
    };
    assert_eq!(base_url_from_provider_config(&pc), None);
}

fn serve_once(status: &str, body: &str, content_type: &str) -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let payload = format!("{header}{body}");
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(payload.as_bytes());
        }
    });
    format!("http://{addr}/v1")
}

#[tokio::test]
async fn fetch_model_catalog_and_context_window_loopback() {
    let body = json!({
        "data": [
            {"id": "gpt-test", "context_length": 32000, "max_output_tokens": 4096},
            {"id": "trk/vendor/other", "context_length": 8000}
        ]
    })
    .to_string();
    let base = serve_once("200 OK", &body, "application/json");
    let mut headers = HashMap::new();
    headers.insert("x-api-key".into(), "header-key".into());
    let cat = fetch_model_catalog(&base, Some("sk-test"), &headers)
        .await
        .unwrap();
    assert_eq!(cat.context_window("gpt-test"), Some(32_000));

    let req = CatalogFetchRequest {
        base_url: serve_once("200 OK", &body, "application/json"),
        api_key: Some("sk-test".into()),
        headers: HashMap::from([("Authorization".into(), "Bearer already".into())]),
        provider_name: "gw".into(),
    };
    let cat = fetch_model_catalog_from_request(&req).await.unwrap();
    assert!(!cat.is_empty());
    let req = CatalogFetchRequest {
        base_url: serve_once("200 OK", &body, "application/json"),
        api_key: Some("sk-test".into()),
        headers: HashMap::from([("Authorization".into(), "Bearer already".into())]),
        provider_name: "gw".into(),
    };
    let cw = fetch_model_context_window(&req, "gpt-test").await.unwrap();
    assert_eq!(cw, Some(32_000));
}

#[tokio::test]
async fn fetch_catalog_errors_on_status_and_invalid_json() {
    let err_url = serve_once("401 Unauthorized", "nope", "text/plain");
    let err = fetch_model_catalog(&err_url, None, &HashMap::new())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("401") || err.to_string().contains("nope"),
        "{err}"
    );

    let bad = serve_once("200 OK", "not-json", "text/plain");
    let err = fetch_model_catalog(&bad, Some("sk"), &HashMap::new())
        .await
        .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("json"), "{err}");

    let err_url = serve_once("500 Internal Server Error", "boom-body", "text/plain");
    let req = CatalogFetchRequest {
        base_url: err_url,
        api_key: Some("sk".into()),
        headers: HashMap::new(),
        provider_name: "gw".into(),
    };
    let err = fetch_model_context_window(&req, "m").await.unwrap_err();
    assert!(
        err.to_string().contains("500") || err.to_string().contains("boom"),
        "{err}"
    );
}

#[tokio::test]
async fn fetch_context_window_rejects_oversized_body() {
    let huge = "x".repeat(8 * 1024 * 1024 + 8);
    let url = serve_once("200 OK", &huge, "application/json");
    let req = CatalogFetchRequest {
        base_url: url,
        api_key: None,
        headers: HashMap::new(),
        provider_name: "gw".into(),
    };
    let err = fetch_model_context_window(&req, "m").await.unwrap_err();
    assert!(err.to_string().contains("too large"), "{err}");
}

#[test]
fn catalog_request_falls_back_to_env_api_key() {
    use whycodes_config::Config;
    use whycodes_core::types::ProviderConfig;

    let name = format!("envkey{}", std::process::id());
    let env_name = format!("{}_API_KEY", name.to_uppercase());
    unsafe { std::env::set_var(&env_name, " sk-from-env ") };
    let mut cfg = Config::default();
    cfg.providers.insert(
        name.clone(),
        ProviderConfig {
            name: name.clone(),
            api_key: None,
            api_base: None,
            base_url: Some("http://gateway.example/v1".into()),
            headers: None,
            models: vec![],
            tool_arguments: None,
            extra: Default::default(),
        },
    );
    let req = catalog_request_from_config(&cfg, &name, None).unwrap();
    assert_eq!(req.api_key.as_deref(), Some("sk-from-env"));
    unsafe { std::env::remove_var(&env_name) };
}

#[test]
fn as_u32_accepts_positive_i64() {
    let m = json!({"context_length": 4096_i64});
    assert_eq!(context_window_from_model_value(&m), Some(4096));
}
