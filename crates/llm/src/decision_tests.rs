use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

/// One HTTP/1.1 reply on a fresh loopback port. Returns the `/v1` base and a
/// receiver for the raw request text.
fn serve_once(status: &str, body: &str) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let payload = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = vec![0u8; 16384];
            let n = stream.read(&mut buf).unwrap_or(0);
            tx.send(String::from_utf8_lossy(&buf[..n]).into_owned())
                .unwrap();
            stream.write_all(payload.as_bytes()).unwrap();
        }
    });
    (format!("http://{addr}/v1"), rx)
}

fn config(base: &str) -> DecisionConfig {
    DecisionConfig {
        enabled: true,
        base_url: format!("{base}/"),
        timeout_ms: 2000,
        ..DecisionConfig::default()
    }
}

fn answer(probabilities: Value, fits: f64) -> Value {
    json!({
        "model": "clef-flash",
        "answers": {
            "blast": {"type": "choice", "choice": "x", "probabilities": probabilities},
            "fits": {"type": "noul", "noul": fits},
        },
        "usage": {"input_tokens": 300, "output_tokens": 0},
    })
}

#[test]
fn parse_folds_risk_and_picks_the_likeliest_effect() {
    let v = parse_verdict(&answer(
        json!({"read_only": 0.1, "project_recoverable": 0.1, "project_unrecoverable": 0.1,
               "outside_project": 0.6, "remote_or_irreversible": 0.1}),
        0.2,
    ))
    .unwrap();
    assert!((v.risk - 1.6).abs() < 1e-9, "{v:?}");
    assert_eq!(v.effect, "outside_project");
    assert!((v.effect_probability - 0.6).abs() < 1e-9);
    assert_eq!(v.fits, 0.2);
}

#[test]
fn parse_treats_missing_options_as_zero() {
    let v = parse_verdict(&answer(json!({"read_only": 1.0}), 1.0)).unwrap();
    assert_eq!(v.risk, 0.0);
    assert_eq!(v.effect, "read_only");
}

#[test]
fn parse_rejects_refusals_and_other_shapes() {
    let refused =
        json!({"answers": {"blast": {"type": "refusal"}, "fits": {"type": "noul", "noul": 0.5}}});
    assert_eq!(
        parse_verdict(&refused).unwrap_err(),
        "no blast choice in response"
    );
    let no_fits = json!({"answers": {
        "blast": {"type": "choice", "probabilities": {"read_only": 1.0}},
        "fits": {"type": "refusal"},
    }});
    assert_eq!(
        parse_verdict(&no_fits).unwrap_err(),
        "no fits answer in response"
    );
    let wrong_type = json!({"answers": {
        "blast": {"type": "choice", "probabilities": {"read_only": 1.0}},
        "fits": {"type": "score", "noul": 0.5},
    }});
    assert!(parse_verdict(&wrong_type).is_err());
    assert!(parse_verdict(&json!({"error": "x"})).is_err());
}

#[test]
fn reason_names_effect_and_mismatch() {
    let v = ShellVerdict {
        risk: 1.9,
        effect: "remote_or_irreversible".into(),
        effect_probability: 0.712,
        fits: 0.1,
    };
    assert_eq!(
        v.reason("clef"),
        "decision model (clef) · remote or irreversible (0.71) · does not match your request"
    );
    let fits = ShellVerdict { fits: 0.9, ..v };
    assert_eq!(
        fits.reason("clef"),
        "decision model (clef) · remote or irreversible (0.71)"
    );
}

#[test]
fn mask_hides_long_query_values_and_percent_runs_and_caps_length() {
    let pasted = "query_id=AAG7T3YWAAAAALtPdhaXVgah&user=%7B%22id%22%3A3768524%2C%22first_name%22%3A%22A%22%7D&hash=0123456789abcdef0123456789abcdef";
    let masked = mask_for_decision(pasted, 4000);
    assert!(!masked.contains("3768524"), "{masked}");
    assert!(!masked.contains("0123456789abcdef"), "{masked}");
    assert!(masked.contains("&hash=<redacted>"), "{masked}");
    let link = "curl 'https://x.example/dl?sig=abcdefghijklmnopqrstuvwxyz0123&n=1'";
    assert_eq!(
        mask_for_decision(link, 4000),
        "curl 'https://x.example/dl?sig=<redacted>&n=1'"
    );
    assert_eq!(mask_for_decision("git status", 4000), "git status");
    assert_eq!(mask_for_decision("ğüşiöç", 3), "ğüş");
    // The body applies it.
    let body = DecisionClient::new(&DecisionConfig::default(), None).body("ls", pasted);
    assert!(
        !body["state"]["user_request"]
            .as_str()
            .unwrap()
            .contains("3768524")
    );
}

#[test]
fn body_pins_providers_only_when_configured() {
    let client = DecisionClient::new(&DecisionConfig::default(), None);
    let body = client.body("ls", "list files");
    assert_eq!(body["model"], "clef-flash");
    assert_eq!(body["state"]["command"], "ls");
    assert_eq!(body["state"]["user_request"], "list files");
    assert_eq!(
        body["questions"]["blast"]["criteria"]
            .as_object()
            .unwrap()
            .len(),
        5
    );
    assert!(body.get("provider").is_none());

    let pinned = DecisionConfig {
        provider_only: vec!["cloudflare".into()],
        ..DecisionConfig::default()
    };
    let body = DecisionClient::new(&pinned, None).body("ls", "x");
    assert_eq!(body["provider"]["only"], json!(["cloudflare"]));
}

#[tokio::test]
async fn verdict_posts_to_systemone_with_bearer() {
    let reply = answer(json!({"read_only": 0.9, "outside_project": 0.1}), 0.9).to_string();
    let (base, rx) = serve_once("200 OK", &reply);
    let client = DecisionClient::new(&config(&base), Some("secret-token".into()));
    assert_eq!(client.model(), "clef-flash");
    let v = client
        .shell_verdict("git status", "show changes")
        .await
        .unwrap();
    assert_eq!(v.effect, "read_only");
    let request = rx.recv().unwrap();
    assert!(request.starts_with("POST /v1/systemone "), "{request}");
    assert!(request.contains("Bearer secret-token"), "{request}");
    assert!(request.contains("git status"), "{request}");
}

#[tokio::test]
async fn verdict_without_key_sends_no_auth_header() {
    let reply = answer(json!({"read_only": 1.0}), 1.0).to_string();
    let (base, rx) = serve_once("200 OK", &reply);
    let client = DecisionClient::new(&config(&base), Some(String::new()));
    client.shell_verdict("ls", "x").await.unwrap();
    let request = rx.recv().unwrap().to_ascii_lowercase();
    assert!(!request.contains("authorization:"), "{request}");
}

#[tokio::test]
async fn verdict_errors_on_status_body_and_transport() {
    let (base, _rx) = serve_once("402 Payment Required", r#"{"error":"credits"}"#);
    let err = DecisionClient::new(&config(&base), None)
        .shell_verdict("ls", "x")
        .await
        .unwrap_err();
    assert!(err.starts_with("HTTP 402"), "{err}");

    let (base, _rx) = serve_once("200 OK", "not json");
    assert!(
        DecisionClient::new(&config(&base), None)
            .shell_verdict("ls", "x")
            .await
            .is_err()
    );

    // A bound port that never answers: the timeout fires.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let held = thread::spawn(move || {
        let conn = listener.accept();
        thread::sleep(Duration::from_millis(600));
        drop(conn);
    });
    let slow = DecisionConfig {
        timeout_ms: 100,
        ..config(&format!("http://{addr}/v1"))
    };
    assert!(
        DecisionClient::new(&slow, None)
            .shell_verdict("ls", "x")
            .await
            .is_err()
    );
    held.join().unwrap();
}
