use super::*;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use whycodes_core::types::{AGENT_NOTE_NAME, MessageContent};

/// Answers every connection with `body`; counts the requests it saw.
pub(super) fn serve(body: String) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&hits);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = vec![0u8; 16384];
            let _ = stream.read(&mut buf);
            seen.fetch_add(1, Ordering::SeqCst);
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (format!("http://{addr}/v1"), hits)
}

/// A `/systemone` reply with `outside` probability and `fits`.
pub(super) fn verdict(outside: f64, fits: f64) -> String {
    json!({
        "answers": {
            "blast": {"type": "choice", "probabilities": {"read_only": 1.0 - outside, "outside_project": outside}},
            "fits": {"type": "noul", "noul": fits},
        }
    })
    .to_string()
}

pub(super) fn gate(base: &str, threshold: f64) -> DecisionGate {
    DecisionGate::from_config(&DecisionConfig {
        enabled: true,
        base_url: base.to_string(),
        threshold,
        timeout_ms: 2000,
        ..DecisionConfig::default()
    })
    .unwrap()
}

fn msg(role: Role, text: &str, name: Option<&str>) -> Message {
    Message {
        role,
        content: MessageContent::Text(text.into()),
        tool_call_id: None,
        name: name.map(str::to_string),
        created_at: None,
    }
}

#[test]
fn from_config_is_none_when_disabled_and_reads_the_key_env() {
    assert!(DecisionGate::from_config(&DecisionConfig::default()).is_none());
    let on = DecisionConfig {
        enabled: true,
        ..DecisionConfig::default()
    };
    assert!(DecisionGate::from_config(&on).is_some());
    // `PATH` is set on every host; an unset name resolves to no key.
    for env in ["PATH", "WHYCODES_TEST_DECISION_KEY_THAT_IS_NEVER_SET"] {
        let cfg = DecisionConfig {
            api_key_env: env.into(),
            ..on.clone()
        };
        assert!(DecisionGate::from_config(&cfg).is_some());
    }
}

#[tokio::test]
async fn escalation_follows_threshold_and_keeps_quiet_on_failure() {
    let (base, _) = serve(verdict(0.9, 0.1));
    let reason = gate(&base, 1.5).escalation("./x.sh", "fix the test").await;
    assert_eq!(
        reason.as_deref(),
        Some("decision model (clef-flash) · outside project (0.90) · does not match your request")
    );
    let (base, _) = serve(verdict(0.1, 0.9));
    assert!(gate(&base, 1.5).escalation("ls", "list").await.is_none());
    let (base, _) = serve("not json".into());
    assert!(gate(&base, 1.5).escalation("ls", "list").await.is_none());
}

#[test]
fn last_human_request_skips_notes_notices_and_assistant_turns() {
    let messages = vec![
        msg(Role::User, "  deploy the docs site  ", None),
        msg(Role::Assistant, "ok", None),
        msg(
            Role::User,
            "Background job `bg-1` finished (done). Output:\nok",
            None,
        ),
        msg(
            Role::User,
            "`edit` failed; its output is in the tool result above.",
            None,
        ),
        msg(Role::User, "The tool failed with error: boom", None),
        msg(Role::User, "a note", Some(AGENT_NOTE_NAME)),
        msg(Role::User, "   ", None),
    ];
    assert_eq!(last_human_request(&messages), "deploy the docs site");
    assert_eq!(last_human_request(&[]), "");
    assert!(!is_notice("`code` blocks are fine"));
}

mod gate {
    use super::*;
    use crate::permission::{PermissionAskFuture, PermissionPrompter};
    use whycodes_core::types::{AgentInfo, AgentMode, PermissionSet, ToolCall};

    struct Prompter {
        allow: bool,
        asks: AtomicUsize,
        detail: std::sync::Mutex<String>,
    }

    impl PermissionPrompter for Prompter {
        fn ask<'a>(&'a self, _tool: &'a str, detail: &'a str) -> PermissionAskFuture<'a> {
            Box::pin(async move {
                self.asks.fetch_add(1, Ordering::SeqCst);
                *self.detail.lock().unwrap() = detail.to_string();
                self.allow
            })
        }
    }

    fn prompter(allow: bool) -> Arc<Prompter> {
        Arc::new(Prompter {
            allow,
            asks: AtomicUsize::new(0),
            detail: std::sync::Mutex::new(String::new()),
        })
    }

    fn agent(prompter: Arc<Prompter>, mode: ApprovalMode, gate: DecisionGate) -> Agent {
        let info = AgentInfo {
            name: "build".into(),
            description: "test".into(),
            mode: AgentMode::Primary,
            permission: PermissionSet {
                allowed_tools: None,
                denied_tools: None,
                allow_file_writes: true,
                allow_network: true,
                allow_shell: true,
                allowed_paths: None,
                rules: Default::default(),
            },
            model: None,
            system_prompt: None,
            temperature: None,
            top_p: None,
        };
        let mut a = Agent::new(info).with_permission_prompter(prompter);
        a.set_approval_mode(mode);
        a.decision = Some(gate);
        a
    }

    async fn run(a: &Agent, command: &str) -> whycodes_core::types::ToolResult {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::new(dir.path().to_path_buf(), "test".into());
        session
            .messages
            .push(msg(Role::User, "explain the config loader", None));
        let ctx = a.tool_context(&session);
        let call = ToolCall {
            id: "t1".into(),
            name: "bash".into(),
            arguments: json!({"command": command}),
        };
        a.execute_with_permission(&call, &session, &ctx, "script", "m", "k", None, None)
            .await
    }

    #[tokio::test]
    async fn escalated_safe_command_prompts_with_reason_and_denial_blocks() {
        let (base, hits) = serve(verdict(0.95, 0.05));
        let p = prompter(false);
        let a = agent(p.clone(), ApprovalMode::Important, super::gate(&base, 1.5));
        let result = run(&a, "echo deploy-now").await;
        assert!(result.is_error, "{result:?}");
        assert!(result.content.contains("User denied"), "{result:?}");
        assert_eq!(p.asks.load(Ordering::SeqCst), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let detail = p.detail.lock().unwrap().clone();
        assert!(detail.contains("Command:\necho deploy-now"), "{detail}");
        assert!(detail.contains("decision model (clef-flash)"), "{detail}");
    }

    #[tokio::test]
    async fn approved_escalation_asks_once_and_runs() {
        let (base, _) = serve(verdict(0.95, 0.05));
        let p = prompter(true);
        let mut a = agent(p.clone(), ApprovalMode::Important, super::gate(&base, 1.5));
        a.secrets = Some(Arc::new(whycodes_llm::Redactor::with_key([7u8; 32])));
        let result = run(&a, "echo decision-approved").await;
        assert!(!result.is_error, "{result:?}");
        assert_eq!(
            p.asks.load(Ordering::SeqCst),
            1,
            "no second ask after approval"
        );
    }

    #[tokio::test]
    async fn below_threshold_runs_without_asking() {
        let (base, hits) = serve(verdict(0.05, 0.95));
        let p = prompter(false);
        let mut a = agent(p.clone(), ApprovalMode::Important, super::gate(&base, 1.5));
        a.secrets = None;
        let result = run(&a, "echo quiet").await;
        assert!(!result.is_error, "{result:?}");
        assert_eq!(p.asks.load(Ordering::SeqCst), 0);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn auto_mode_and_allow_rules_skip_the_model() {
        let (base, hits) = serve(verdict(0.95, 0.05));
        let p = prompter(false);
        let a = agent(p.clone(), ApprovalMode::Auto, super::gate(&base, 1.5));
        assert!(!run(&a, "echo auto").await.is_error);

        let mut a = agent(p.clone(), ApprovalMode::Important, super::gate(&base, 1.5));
        a.info
            .permission
            .rules
            .insert("bash(echo *)".into(), PermissionAction::Allow);
        assert!(!run(&a, "echo allowed").await.is_error);

        assert_eq!(hits.load(Ordering::SeqCst), 0, "model never called");
        assert_eq!(p.asks.load(Ordering::SeqCst), 0);
    }
}
