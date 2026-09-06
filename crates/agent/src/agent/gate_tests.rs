use super::*;
use crate::permission::PermissionPrompter;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use whycodes_core::Tool;
use whycodes_core::types::{
    AgentInfo, AgentMode, ApprovalMode, PermissionAction, PermissionSet, ToolCall,
};

fn info(name: &str) -> AgentInfo {
    AgentInfo {
        name: name.to_string(),
        description: format!("Test agent: {name}"),
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
        system_prompt: Some("You are a test agent.".to_string()),
        temperature: Some(0.5),
        top_p: None,
    }
}

fn agent() -> Agent {
    Agent::new(info("build"))
}

fn tc(name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "t1".into(),
        name: name.into(),
        arguments: args,
    }
}

fn session_ctx(agent: &Agent) -> (Session, ToolContext) {
    let session = Session::new(std::path::PathBuf::from("/work/proj"), "test".into());
    let ctx = agent.tool_context(&session);
    (session, ctx)
}

struct CountingDenyPrompter {
    asks: AtomicUsize,
}

impl PermissionPrompter for CountingDenyPrompter {
    fn ask<'a>(
        &'a self,
        _tool_name: &'a str,
        _detail: &'a str,
    ) -> crate::permission::PermissionAskFuture<'a> {
        Box::pin(async move {
            self.asks.fetch_add(1, Ordering::SeqCst);
            false
        })
    }
}

struct FlakyRead {
    hits: Arc<AtomicUsize>,
}

impl whycodes_core::Tool for FlakyRead {
    fn name(&self) -> &str {
        "read"
    }
    fn description(&self) -> &str {
        "flaky"
    }
    fn parameters(&self) -> serde_json::Value {
        json!({"type": "object"})
    }
    fn execute<'a>(
        &'a self,
        _args: serde_json::Value,
        _ctx: &'a whycodes_core::ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        let hits = Arc::clone(&self.hits);
        Box::pin(async move {
            let n = hits.fetch_add(1, Ordering::SeqCst) + 1;
            if n < 3 {
                ToolResult {
                    tool_call_id: String::new(),
                    content: format!("transient fail {n}"),
                    is_error: true,
                }
            } else {
                ToolResult {
                    tool_call_id: String::new(),
                    content: "ok".into(),
                    is_error: false,
                }
            }
        })
    }
}

#[test]
fn retryable_skips_policy_and_question() {
    let err = |c: &str| ToolResult {
        tool_call_id: "t".into(),
        content: c.into(),
        is_error: true,
    };
    assert!(!tool_error_is_retryable(
        "read",
        &ToolResult {
            tool_call_id: "t".into(),
            content: "ok".into(),
            is_error: false,
        }
    ));
    assert!(tool_error_is_retryable("read", &err("transient fail 1")));
    assert!(!tool_error_is_retryable("question", &err("do not ask")));
    assert!(!tool_error_is_retryable("task", &err("subagent failed")));
    assert!(!tool_error_is_retryable("swarm", &err("x")));
    assert!(!tool_error_is_retryable("bg", &err("x")));
    assert!(!tool_error_is_retryable("schedule", &err("x")));
    assert!(!tool_error_is_retryable("worktree", &err("x")));
    assert!(!tool_error_is_retryable("todowrite", &err("x")));
    assert!(!tool_error_is_retryable("todo", &err("x")));
    assert!(!tool_error_is_retryable("todoread", &err("x")));
    assert!(!tool_error_is_retryable("checkpoint", &err("x")));
    assert!(!tool_error_is_retryable("rewind", &err("x")));
    assert!(!tool_error_is_retryable(
        "read",
        &err("Permission denied for tool 'read'.")
    ));
    assert!(!tool_error_is_retryable("bash", &err("user denied")));
    assert!(!tool_error_is_retryable("bash", &err("doom loop")));
    assert!(!tool_error_is_retryable("bash", &err("cannot be approved")));
    assert!(!tool_error_is_retryable("bash", &err("catastrophic")));
    assert!(!tool_error_is_retryable(
        "bash",
        &err("Refused: catastrophic")
    ));
    assert_eq!(AUTO_TOOL_RETRY_LIMIT, 2);
}

#[tokio::test]
async fn dispatch_tool_special_names_via_permission() {
    let mut a = agent();
    a.swarm_enabled = false;
    let (session, ctx) = session_ctx(&a);

    let bg = a
        .execute_with_permission(
            &tc("bg", json!({"action": "list"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert!(bg.content.contains("No background jobs"), "{bg:?}");

    let search = a
        .execute_with_permission(
            &tc("tool_search", json!({"action": "list"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert!(!search.is_error, "{search:?}");

    let wt = a
        .execute_with_permission(
            &tc("worktree", json!({"action": "nope"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert!(wt.is_error, "{wt:?}");

    let sched = a
        .execute_with_permission(
            &tc("schedule", json!({})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert!(sched.is_error, "{sched:?}");

    let task = a
        .execute_with_permission(
            &tc("task", json!({})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert!(task.is_error, "{task:?}");

    let swarm = a
        .execute_with_permission(
            &tc("swarm", json!({"tasks": [{"goal": "x"}]})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert!(swarm.is_error, "{swarm:?}");
    assert!(
        swarm.content.to_lowercase().contains("disabled"),
        "{swarm:?}"
    );

    let bg_shell = a
        .execute_with_permission(
            &tc("bash", json!({"command": "", "background": true})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert!(bg_shell.is_error, "{bg_shell:?}");
    assert!(
        bg_shell.content.to_lowercase().contains("command"),
        "{bg_shell:?}"
    );
}

#[tokio::test]
async fn confirm_class_bash_denied_by_prompter() {
    let prompter = Arc::new(CountingDenyPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut a = Agent::new(info("build")).with_permission_prompter(prompter.clone());
    a.set_approval_mode(ApprovalMode::Manual);
    let (session, ctx) = session_ctx(&a);
    let result = a
        .execute_with_permission(
            &tc("bash", json!({"command": "rm -rf /tmp/scratch"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert_eq!(prompter.asks.load(Ordering::SeqCst), 1);
    assert!(
        result.content.contains("User denied permission"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn shell_rule_deny_allow_and_ask() {
    let mut deny_info = info("build");
    deny_info
        .permission
        .rules
        .insert("bash(git *)".into(), PermissionAction::Deny);
    let deny = Agent::new(deny_info);
    let (session, ctx) = session_ctx(&deny);
    let denied = deny
        .execute_with_permission(
            &tc("bash", json!({"command": "git status"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert!(denied.is_error, "{denied:?}");
    assert!(
        denied.content.contains("Permission denied for shell"),
        "{}",
        denied.content
    );

    let asks = Arc::new(CountingDenyPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut allow_info = info("build");
    allow_info
        .permission
        .rules
        .insert("bash(git *)".into(), PermissionAction::Allow);
    let mut allow = Agent::new(allow_info).with_permission_prompter(asks.clone());
    allow.set_approval_mode(ApprovalMode::Manual);
    let (session, ctx) = session_ctx(&allow);
    let _ = allow
        .execute_with_permission(
            &tc("bash", json!({"command": "git status"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert_eq!(asks.asks.load(Ordering::SeqCst), 0);

    let ask_p = Arc::new(CountingDenyPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut ask_info = info("build");
    ask_info
        .permission
        .rules
        .insert("bash(git status)".into(), PermissionAction::Ask);
    let mut ask_agent = Agent::new(ask_info).with_permission_prompter(ask_p.clone());
    ask_agent.set_approval_mode(ApprovalMode::Manual);
    let (session, ctx) = session_ctx(&ask_agent);
    let asked = ask_agent
        .execute_with_permission(
            &tc("bash", json!({"command": "git status"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert_eq!(ask_p.asks.load(Ordering::SeqCst), 1);
    assert!(
        asked.content.contains("User denied permission"),
        "{}",
        asked.content
    );
}

#[tokio::test]
async fn intent_refuse_and_confirm_deny() {
    let ask = Agent::new(info("ask"));
    let (session, ctx) = session_ctx(&ask);
    let intent = crate::intent::classify_user_intent("please write a file");
    let refused = ask
        .execute_with_permission(
            &tc("write", json!({"path": "a.md", "content": "x"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            Some(&intent),
            None,
        )
        .await;
    assert!(refused.is_error, "{refused:?}");
    assert!(
        refused.content.contains("intent") || refused.content.contains("read-only"),
        "{}",
        refused.content
    );

    let deny = Arc::new(CountingDenyPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut build = Agent::new(info("build")).with_permission_prompter(deny.clone());
    build.set_approval_mode(ApprovalMode::Manual);
    let (session, ctx) = session_ctx(&build);
    let q = crate::intent::classify_user_intent("how does auth work?");
    let confirmed = build
        .execute_with_permission(
            &tc("write", json!({"path": "a.md", "content": "x"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            Some(&q),
            None,
        )
        .await;
    assert_eq!(deny.asks.load(Ordering::SeqCst), 1);
    assert!(
        confirmed.content.contains("intent gate"),
        "{}",
        confirmed.content
    );
}

#[tokio::test]
async fn path_rule_deny_allow_and_ask() {
    let mut deny_info = info("build");
    deny_info
        .permission
        .rules
        .insert("edit(src/**)".into(), PermissionAction::Deny);
    let deny = Agent::new(deny_info);
    let (session, ctx) = session_ctx(&deny);
    let denied = deny
        .execute_with_permission(
            &tc(
                "edit",
                json!({"path": "src/lib.rs", "old_string": "a", "new_string": "b"}),
            ),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert!(denied.is_error, "{denied:?}");
    assert!(denied.content.contains("path rule"), "{}", denied.content);

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    let asks = Arc::new(CountingDenyPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut allow_info = info("build");
    allow_info
        .permission
        .rules
        .insert("write(docs/**)".into(), PermissionAction::Allow);
    let mut allow = Agent::new(allow_info).with_permission_prompter(asks.clone());
    allow.set_approval_mode(ApprovalMode::Manual);
    let session = Session::new(dir.path().to_path_buf(), "test".into());
    let ctx = allow.tool_context(&session);
    let _ = allow
        .execute_with_permission(
            &tc("write", json!({"path": "docs/a.md", "content": "ok"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert_eq!(asks.asks.load(Ordering::SeqCst), 0);

    let ask_p = Arc::new(CountingDenyPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut ask_info = info("build");
    ask_info
        .permission
        .rules
        .insert("write(docs/**)".into(), PermissionAction::Ask);
    let mut ask_agent = Agent::new(ask_info).with_permission_prompter(ask_p.clone());
    ask_agent.set_approval_mode(ApprovalMode::Manual);
    let (session, ctx) = session_ctx(&ask_agent);
    let asked = ask_agent
        .execute_with_permission(
            &tc("write", json!({"path": "docs/a.md", "content": "ok"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert_eq!(ask_p.asks.load(Ordering::SeqCst), 1);
    assert!(
        asked.content.contains("User denied permission"),
        "{}",
        asked.content
    );
}

#[tokio::test]
async fn invalid_question_and_background_open_work() {
    let mut a = agent();
    a.set_approval_mode(ApprovalMode::Auto);
    let (session, ctx) = session_ctx(&a);
    let invalid = a
        .execute_with_permission(
            &tc("question", json!({})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert!(invalid.is_error, "{invalid:?}");
    assert!(
        invalid.content.contains("Invalid questionnaire"),
        "{}",
        invalid.content
    );

    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path().to_path_buf(), "test".into());
    let ctx = a.tool_context(&session);
    let started = a.execute_background_shell(
        &tc(
            "bash",
            json!({"command": "sleep 30", "description": "hold"}),
        ),
        &ctx,
        None,
    );
    assert!(!started.is_error, "{started:?}");
    let q = a
        .execute_with_permission(
            &tc(
                "question",
                json!({"question": "Pick", "choices": ["A", "B"]}),
            ),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    a.background.kill_all();
    assert!(q.is_error, "{q:?}");
    assert!(q.content.contains("background tasks"), "{}", q.content);
}

#[tokio::test]
async fn auto_question_refuses_when_session_todos_are_open() {
    let mut a = agent();
    a.set_approval_mode(ApprovalMode::Auto);
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(dir.path().to_path_buf(), "test".into());
    session.id = "todo-sess".into();
    whycodes_core::todo::save_todos(
        dir.path(),
        Some(&session.id),
        &[whycodes_core::todo::TodoItem::new(
            "1",
            "keep going",
            whycodes_core::todo::TodoStatus::Pending,
        )],
    )
    .expect("save todos");
    let ctx = a.tool_context(&session);
    let q = a
        .execute_with_permission(
            &tc(
                "question",
                json!({"question": "Pick", "choices": ["A", "B"]}),
            ),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert!(q.is_error, "{q:?}");
    assert!(q.content.contains("todos"), "{}", q.content);
}

#[tokio::test]
async fn auto_retry_emits_status() {
    let hits = Arc::new(AtomicUsize::new(0));
    let mut exec = whycodes_tools::executor::ToolExecutor::new();
    exec.register(Box::new(FlakyRead {
        hits: Arc::clone(&hits),
    }));
    let mut a = Agent::new(info("build")).with_tool_executor(exec);
    a.set_approval_mode(ApprovalMode::Auto);
    let (session, ctx) = session_ctx(&a);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let result = a
        .execute_with_permission(
            &tc("read", json!({"path": "x"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            Some(&tx),
        )
        .await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(hits.load(Ordering::SeqCst), 3);
    let mut saw = false;
    while let Ok(ev) = rx.try_recv() {
        if let TurnEvent::Status(s) = ev
            && s.contains("Auto: retrying")
        {
            saw = true;
        }
    }
    assert!(saw, "expected Auto: retrying status");
}

#[tokio::test]
async fn execute_tool_calls_empty_and_cancel() {
    let a = agent();
    let (session, ctx) = session_ctx(&a);
    let empty = a
        .execute_tool_calls(
            &[],
            &session,
            &ctx,
            "script",
            "m",
            "k",
            &None,
            &None,
            None,
            &mut Vec::new(),
        )
        .await
        .expect("empty");
    assert!(empty.is_empty());

    let cancel = crate::events::new_cancel_flag();
    crate::events::request_cancel(&cancel);
    let single = a
        .execute_tool_calls(
            &[tc("read", json!({"path": "x"}))],
            &session,
            &ctx,
            "script",
            "m",
            "k",
            &None,
            &Some(cancel.clone()),
            None,
            &mut Vec::new(),
        )
        .await;
    assert!(single.is_err());

    let parallel = a
        .execute_tool_calls(
            &[
                ToolCall {
                    id: "a".into(),
                    name: "read".into(),
                    arguments: json!({"path": "a"}),
                },
                ToolCall {
                    id: "b".into(),
                    name: "read".into(),
                    arguments: json!({"path": "b"}),
                },
            ],
            &session,
            &ctx,
            "script",
            "m",
            "k",
            &None,
            &Some(cancel.clone()),
            None,
            &mut Vec::new(),
        )
        .await;
    assert!(parallel.is_err());

    let sequential = a
        .execute_tool_calls(
            &[
                tc("read", json!({"path": "a"})),
                tc("bash", json!({"command": "echo hi"})),
            ],
            &session,
            &ctx,
            "script",
            "m",
            "k",
            &None,
            &Some(cancel),
            None,
            &mut Vec::new(),
        )
        .await;
    assert!(sequential.is_err());
}

#[tokio::test]
async fn take_speculative_read_hits_matching_call() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "payload").unwrap();
    let a = agent();
    let session = Session::new(dir.path().to_path_buf(), "test".into());
    let ctx = a.tool_context(&session);
    let mut jobs = Vec::new();
    crate::speculative_read::maybe_start(&mut jobs, "t1", "read", r#"{"path":"n.txt"}"#, &ctx);
    assert_eq!(jobs.len(), 1);
    let out = a
        .execute_tool_calls(
            &[tc("read", json!({"path": "n.txt"}))],
            &session,
            &ctx,
            "script",
            "m",
            "k",
            &None,
            &None,
            None,
            &mut jobs,
        )
        .await
        .expect("read");
    assert_eq!(out.len(), 1);
    assert!(out[0].content.contains("payload"), "{:?}", out[0]);
    assert!(jobs.is_empty());
}

#[tokio::test]
async fn important_prompts_high_risk_schedule() {
    let asks = Arc::new(CountingDenyPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut info = info("build");
    info.permission
        .rules
        .insert("schedule".into(), PermissionAction::Ask);
    let mut a = Agent::new(info).with_permission_prompter(asks.clone());
    a.set_approval_mode(ApprovalMode::Important);
    let (session, ctx) = session_ctx(&a);
    let result = a
        .execute_with_permission(
            &tc(
                "schedule",
                json!({"command": "rm -rf /tmp/scratch", "after_secs": 0}),
            ),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert_eq!(asks.asks.load(Ordering::SeqCst), 1);
    assert!(
        result.content.contains("User denied permission"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn parallel_speculative_and_sequential_speculative() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
    std::fs::write(dir.path().join("b.txt"), "beta").unwrap();
    let a = agent();
    let session = Session::new(dir.path().to_path_buf(), "test".into());
    let ctx = a.tool_context(&session);
    let mut jobs = Vec::new();
    crate::speculative_read::maybe_start(&mut jobs, "a", "read", r#"{"path":"a.txt"}"#, &ctx);
    crate::speculative_read::maybe_start(&mut jobs, "b", "read", r#"{"path":"b.txt"}"#, &ctx);
    let parallel = a
        .execute_tool_calls(
            &[
                ToolCall {
                    id: "a".into(),
                    name: "read".into(),
                    arguments: json!({"path": "a.txt"}),
                },
                ToolCall {
                    id: "b".into(),
                    name: "read".into(),
                    arguments: json!({"path": "b.txt"}),
                },
            ],
            &session,
            &ctx,
            "script",
            "m",
            "k",
            &None,
            &None,
            None,
            &mut jobs,
        )
        .await
        .expect("parallel");
    assert_eq!(parallel.len(), 2);
    assert!(parallel[0].content.contains("alpha"), "{:?}", parallel[0]);
    assert!(parallel[1].content.contains("beta"), "{:?}", parallel[1]);

    let mut jobs = Vec::new();
    crate::speculative_read::maybe_start(&mut jobs, "t1", "read", r#"{"path":"a.txt"}"#, &ctx);
    let mixed = a
        .execute_tool_calls(
            &[
                tc("read", json!({"path": "a.txt"})),
                tc("bash", json!({"command": "echo hi"})),
            ],
            &session,
            &ctx,
            "script",
            "m",
            "k",
            &None,
            &None,
            None,
            &mut jobs,
        )
        .await
        .expect("mixed");
    assert_eq!(mixed.len(), 2);
    assert!(mixed[0].content.contains("alpha"), "{:?}", mixed[0]);
}

struct HangRead;

impl whycodes_core::Tool for HangRead {
    fn name(&self) -> &str {
        "read"
    }
    fn description(&self) -> &str {
        "hang"
    }
    fn parameters(&self) -> serde_json::Value {
        json!({"type": "object"})
    }
    fn execute<'a>(
        &'a self,
        _args: serde_json::Value,
        _ctx: &'a whycodes_core::ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            // Longer than the cancel-during-execute wait (30ms) so cancel
            // wins; short enough that hang_read_execute can await it.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            ToolResult {
                tool_call_id: String::new(),
                content: "never".into(),
                is_error: false,
            }
        })
    }
}

#[tokio::test]
async fn cancel_during_single_and_sequential_execute() {
    let mut exec = whycodes_tools::executor::ToolExecutor::new();
    exec.register(Box::new(HangRead));
    let a = Agent::new(info("build")).with_tool_executor(exec);
    let (session, ctx) = session_ctx(&a);
    let cancel = crate::events::new_cancel_flag();
    let handle = {
        let cancel = cancel.clone();
        let session = session.clone();
        let ctx = ctx.clone();
        tokio::spawn(async move {
            a.execute_tool_calls(
                &[tc("read", json!({"path": "x"}))],
                &session,
                &ctx,
                "script",
                "m",
                "k",
                &None,
                &Some(cancel),
                None,
                &mut Vec::new(),
            )
            .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    crate::events::request_cancel(&cancel);
    let err = handle.await.expect("join");
    assert!(err.is_err(), "{err:?}");

    let mut exec = whycodes_tools::executor::ToolExecutor::new();
    exec.register(Box::new(HangRead));
    let a = Agent::new(info("build")).with_tool_executor(exec);
    let (session, ctx) = session_ctx(&a);
    let cancel = crate::events::new_cancel_flag();
    let handle = {
        let cancel = cancel.clone();
        let session = session.clone();
        let ctx = ctx.clone();
        tokio::spawn(async move {
            a.execute_tool_calls(
                &[
                    tc("read", json!({"path": "x"})),
                    tc("bash", json!({"command": "echo hi"})),
                ],
                &session,
                &ctx,
                "script",
                "m",
                "k",
                &None,
                &Some(cancel),
                None,
                &mut Vec::new(),
            )
            .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    crate::events::request_cancel(&cancel);
    let err = handle.await.expect("join");
    assert!(err.is_err(), "{err:?}");
}

struct CountingAllowPrompter {
    asks: AtomicUsize,
}

impl PermissionPrompter for CountingAllowPrompter {
    fn ask<'a>(
        &'a self,
        _tool_name: &'a str,
        _detail: &'a str,
    ) -> crate::permission::PermissionAskFuture<'a> {
        Box::pin(async move {
            self.asks.fetch_add(1, Ordering::SeqCst);
            true
        })
    }
}

#[tokio::test]
async fn confirm_then_ask_rule_skips_second_prompt() {
    let asks = Arc::new(CountingAllowPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut info = info("build");
    info.permission
        .rules
        .insert("bash(rm *)".into(), PermissionAction::Ask);
    let mut a = Agent::new(info).with_permission_prompter(asks.clone());
    a.set_approval_mode(ApprovalMode::Manual);
    let (session, ctx) = session_ctx(&a);
    let result = a
        .execute_with_permission(
            &tc("bash", json!({"command": "rm -rf /tmp/scratch-cov"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert_eq!(asks.asks.load(Ordering::SeqCst), 1, "{result:?}");
    assert!(!result.content.contains("User denied"), "{result:?}");
}

#[test]
fn dummy_tool_trait_methods_are_callable() {
    let flaky = FlakyRead {
        hits: Arc::new(AtomicUsize::new(0)),
    };
    assert_eq!(flaky.description(), "flaky");
    assert_eq!(flaky.parameters()["type"], "object");
    let hang = HangRead;
    assert_eq!(hang.description(), "hang");
    assert_eq!(hang.parameters()["type"], "object");
}

#[tokio::test]
async fn hang_read_execute_returns_after_short_sleep() {
    // Cover the post-sleep ToolResult in HangRead without a 30s wait.
    struct QuickHang;
    impl whycodes_core::Tool for QuickHang {
        fn name(&self) -> &str {
            "read"
        }
        fn description(&self) -> &str {
            "hang"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({"type": "object"})
        }
        fn execute<'a>(
            &'a self,
            _args: serde_json::Value,
            _ctx: &'a whycodes_core::ToolContext,
        ) -> whycodes_core::ToolFuture<'a> {
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                ToolResult {
                    tool_call_id: String::new(),
                    content: "never".into(),
                    is_error: false,
                }
            })
        }
    }
    let tool = QuickHang;
    let ctx = whycodes_core::ToolContext::new("/tmp");
    let out = tool.execute(json!({}), &ctx).await;
    assert_eq!(out.content, "never");
    let hang = HangRead;
    let _ = hang.description();
    let _ = hang.parameters();
    let hang_out = hang.execute(json!({}), &ctx).await;
    assert_eq!(hang_out.content, "never");
}

#[tokio::test]
async fn intent_confirm_skipped_after_risk_confirmed() {
    let asks = Arc::new(CountingAllowPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut a = Agent::new(info("build")).with_permission_prompter(asks.clone());
    a.set_approval_mode(ApprovalMode::Manual);
    a.intent_guidance = crate::intent::IntentGuidanceMode::Always;
    let (session, ctx) = session_ctx(&a);
    let q = crate::intent::classify_user_intent("how does auth work?");
    let result = a
        .execute_with_permission(
            &tc("bash", json!({"command": "rm -rf /tmp/scratch-intent-cov"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            Some(&q),
            None,
        )
        .await;
    // Destructive Confirm already set risk_confirmed; intent Confirm must not ask again.
    assert_eq!(asks.asks.load(Ordering::SeqCst), 1, "{result:?}");
    assert!(!result.content.contains("User denied"), "{result:?}");
}

#[tokio::test]
async fn path_ask_skipped_after_intent_confirm() {
    let asks = Arc::new(CountingAllowPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut info = info("build");
    info.permission
        .rules
        .insert("write(docs/**)".into(), PermissionAction::Ask);
    let mut a = Agent::new(info).with_permission_prompter(asks.clone());
    a.set_approval_mode(ApprovalMode::Manual);
    a.intent_guidance = crate::intent::IntentGuidanceMode::Always;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    let session = Session::new(dir.path().to_path_buf(), "test".into());
    let ctx = a.tool_context(&session);
    let q = crate::intent::classify_user_intent("how does auth work?");
    let result = a
        .execute_with_permission(
            &tc("write", json!({"path": "docs/a.md", "content": "ok"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            Some(&q),
            None,
        )
        .await;
    assert_eq!(asks.asks.load(Ordering::SeqCst), 1, "{result:?}");
    assert!(!result.content.contains("User denied"), "{result:?}");
}

#[tokio::test]
async fn tool_ask_skipped_after_path_allow() {
    let asks = Arc::new(CountingAllowPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut info = info("build");
    info.permission
        .rules
        .insert("write(docs/**)".into(), PermissionAction::Allow);
    info.permission
        .rules
        .insert("write".into(), PermissionAction::Ask);
    let mut a = Agent::new(info).with_permission_prompter(asks.clone());
    a.set_approval_mode(ApprovalMode::Manual);
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    let session = Session::new(dir.path().to_path_buf(), "test".into());
    let ctx = a.tool_context(&session);
    let result = a
        .execute_with_permission(
            &tc("write", json!({"path": "docs/a.md", "content": "ok"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert_eq!(asks.asks.load(Ordering::SeqCst), 0, "{result:?}");
    assert!(!result.content.contains("User denied"), "{result:?}");
}

#[tokio::test]
async fn shell_ask_rule_allows_and_sets_risk_confirmed() {
    let asks = Arc::new(CountingAllowPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut info = info("build");
    info.permission
        .rules
        .insert("bash(echo *)".into(), PermissionAction::Ask);
    let mut a = Agent::new(info).with_permission_prompter(asks.clone());
    a.set_approval_mode(ApprovalMode::Manual);
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path().to_path_buf(), "test".into());
    let ctx = a.tool_context(&session);
    let result = a
        .execute_with_permission(
            &tc("bash", json!({"command": "echo hello-ask"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert_eq!(asks.asks.load(Ordering::SeqCst), 1, "{result:?}");
    assert!(!result.is_error, "{result:?}");
}

#[tokio::test]
async fn path_ask_allows_without_prior_confirm() {
    let asks = Arc::new(CountingAllowPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut info = info("build");
    info.permission
        .rules
        .insert("write(docs/**)".into(), PermissionAction::Ask);
    let mut a = Agent::new(info).with_permission_prompter(asks.clone());
    a.set_approval_mode(ApprovalMode::Manual);
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    let session = Session::new(dir.path().to_path_buf(), "test".into());
    let ctx = a.tool_context(&session);
    let result = a
        .execute_with_permission(
            &tc("write", json!({"path": "docs/a.md", "content": "ok"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert_eq!(asks.asks.load(Ordering::SeqCst), 1, "{result:?}");
    assert!(!result.content.contains("User denied"), "{result:?}");
}

#[tokio::test]
async fn path_ask_denied_without_prior_confirm() {
    let asks = Arc::new(CountingDenyPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut info = info("build");
    info.permission
        .rules
        .insert("write(docs/**)".into(), PermissionAction::Ask);
    let mut a = Agent::new(info).with_permission_prompter(asks.clone());
    a.set_approval_mode(ApprovalMode::Manual);
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    let session = Session::new(dir.path().to_path_buf(), "test".into());
    let ctx = a.tool_context(&session);
    let result = a
        .execute_with_permission(
            &tc("write", json!({"path": "docs/a.md", "content": "ok"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            None,
            None,
        )
        .await;
    assert_eq!(asks.asks.load(Ordering::SeqCst), 1, "{result:?}");
    assert!(result.is_error, "{result:?}");
    assert!(
        result.content.contains("User denied permission"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn intent_refuse_does_not_prompt() {
    let asks = Arc::new(CountingAllowPrompter {
        asks: AtomicUsize::new(0),
    });
    let mut a = Agent::new(info("ask")).with_permission_prompter(asks.clone());
    a.set_approval_mode(ApprovalMode::Manual);
    a.intent_guidance = crate::intent::IntentGuidanceMode::Always;
    let (session, ctx) = session_ctx(&a);
    let q = crate::intent::classify_user_intent("how does auth work?");
    let result = a
        .execute_with_permission(
            &tc("write", json!({"path": "docs/a.md", "content": "nope"})),
            &session,
            &ctx,
            "script",
            "m",
            "k",
            Some(&q),
            None,
        )
        .await;
    assert!(result.is_error, "{result:?}");
    assert!(
        result.content.to_lowercase().contains("intent")
            || result.content.to_lowercase().contains("refused"),
        "{}",
        result.content
    );
    assert_eq!(asks.asks.load(Ordering::SeqCst), 0, "{result:?}");
}
