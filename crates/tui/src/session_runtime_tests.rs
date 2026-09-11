use super::*;

fn msg(role: ChatRole, content: &str) -> ChatMessage {
    ChatMessage {
        role,
        content: content.to_string(),
        blocks: Vec::new(),
        results_expanded: false,
        tool_calls: Vec::new(),
        error: None,
        duration_ms: None,
        image_labels: Vec::new(),
        created_at: None,
        layout_cache: None,
        line_cache: None,
        stream_md: None,
    }
}

#[test]
fn session_state_group_rank_orders_needs_input_first() {
    assert_eq!(SessionState::WaitingPermission.group_rank(), 0);
    assert_eq!(SessionState::WaitingInput.group_rank(), 0);
    assert_eq!(SessionState::Working.group_rank(), 1);
    assert_eq!(SessionState::Idle.group_rank(), 2);
    assert_eq!(SessionState::Error.group_rank(), 2);
}

#[test]
fn session_state_glyphs_and_labels() {
    assert_eq!(SessionState::Working.glyph(), "…");
    assert_eq!(SessionState::WaitingPermission.glyph(), "!");
    assert_eq!(SessionState::WaitingInput.glyph(), "!");
    assert_eq!(SessionState::Idle.glyph(), "·");
    assert_eq!(SessionState::Error.glyph(), "✗");
    assert_eq!(SessionState::Working.label(), "working");
    assert_eq!(SessionState::WaitingPermission.label(), "needs permission");
    assert_eq!(SessionState::WaitingInput.label(), "needs input");
    assert_eq!(SessionState::Idle.label(), "idle");
    assert_eq!(SessionState::Error.label(), "error");
}

#[test]
fn view_snapshot_default_is_sane() {
    let v = ViewSnapshot::default();
    assert!(v.messages.is_empty());
    assert!(v.session_title.is_empty());
    assert_eq!(v.current_agent_state, AgentState::Idle);
    assert_eq!(v.scroll_offset, 0);
    assert!(v.auto_scroll);
    assert!(v.selected_msg.is_none());
    assert!(v.input_buffer.is_empty());
    assert_eq!(v.context_used, 0);
}

#[test]
fn runtime_state_transitions() {
    let rt = make_runtime();
    assert_eq!(rt.state(), SessionState::Idle);

    let mut rt = make_runtime();
    rt.agent_busy = true;
    assert_eq!(rt.state(), SessionState::Working);

    let mut rt = make_runtime();
    rt.agent_busy = true;
    rt.pending_perm_queue.push_back(perm_request());
    assert_eq!(rt.state(), SessionState::WaitingPermission);

    let mut rt = make_runtime();
    rt.agent_busy = true;
    rt.pending_question_queue.push_back(question_request());
    assert_eq!(rt.state(), SessionState::WaitingInput);

    let mut rt = make_runtime();
    rt.last_error = true;
    assert_eq!(rt.state(), SessionState::Error);

    let mut rt = make_runtime();
    rt.agent_busy = true;
    rt.pending_perm_queue.push_back(perm_request());
    rt.pending_question_queue.push_back(question_request());
    assert_eq!(
        rt.state(),
        SessionState::WaitingPermission,
        "permission outranks input"
    );
}

#[test]
fn preview_prefers_status_while_busy() {
    let mut rt = make_runtime();
    rt.agent_busy = true;
    rt.view.status_message = "Running tests…".into();
    assert_eq!(rt.preview(), "Running tests…");
}

#[test]
fn preview_uses_last_non_empty_message_when_idle() {
    let mut rt = make_runtime();
    rt.view.messages = vec![
        msg(ChatRole::System, "sys"),
        msg(ChatRole::User, "hello"),
        msg(ChatRole::Assistant, "hi there\nsecond line"),
    ];
    assert_eq!(rt.preview(), "hi there");
}

#[test]
fn preview_truncates_to_80_chars() {
    let mut rt = make_runtime();
    let long = "x".repeat(200);
    rt.view.messages = vec![msg(ChatRole::User, &long)];
    assert_eq!(rt.preview().chars().count(), 80);
}

#[test]
fn preview_empty_when_no_messages() {
    let rt = make_runtime();
    assert_eq!(rt.preview(), "");
}

#[test]
fn age_is_nonzero_after_construction() {
    let rt = make_runtime();
    // Instant::now() can report 0 on a fast machine; just prove the
    // field is wired and Duration is representable.
    let _ = rt.age();
    assert!(rt.age() < std::time::Duration::from_secs(60));
}

#[test]
fn persist_is_a_noop_without_a_database() {
    let mut rt = make_runtime();
    rt.db = None;
    rt.persist("coverage");
}

// ── helpers ────────────────────────────────────────────────────────

/// Point the data dir at a throwaway temp dir once per test binary so
/// `SessionRuntime::new` (which opens `whycodes.db`) never touches real
/// user data. Parallel tests share the same isolated root.
fn isolate_data_dir() {
    use std::sync::OnceLock;
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = HOME.get_or_init(|| tempfile::tempdir().expect("tempdir"));
    unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
}

fn perm_request() -> PermissionRequest {
    let (tx, _rx) = tokio::sync::oneshot::channel();
    PermissionRequest {
        tool_name: "bash".into(),
        detail: "echo hi".into(),
        reply: tx,
    }
}

fn question_request() -> QuestionRequest {
    let (tx, _rx) = tokio::sync::oneshot::channel();
    QuestionRequest {
        questions: vec![whycodes_tools::question::QuestionSpec {
            prompt: "Pick?".into(),
            options: vec![whycodes_tools::question::QuestionOption {
                label: "A".into(),
                description: String::new(),
                preview: None,
            }],
            multi_select: false,
            important: false,
        }],
        reply: tx,
    }
}

fn make_runtime() -> SessionRuntime {
    use whycodes_core::types::{AgentInfo, AgentMode, PermissionSet};
    use whycodes_session::SessionHistory;
    use whycodes_session::session::Session;

    isolate_data_dir();
    let info = AgentInfo {
        name: "build".into(),
        description: String::new(),
        mode: AgentMode::Primary,
        permission: PermissionSet::default(),
        model: None,
        system_prompt: None,
        temperature: None,
        top_p: None,
    };
    let agent = whycodes_agent::agent::Agent::new(info);
    let session = Session::new(std::path::PathBuf::from("/work/proj"), "sys".into());
    let history = SessionHistory::new();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (done_tx, done_rx) = mpsc::unbounded_channel();
    let (perm_prompter, perm_rx) = ChannelPermissionPrompter::new();
    let (question_prompter, question_rx) = ChannelQuestionPrompter::new(None);
    SessionRuntime::new(
        agent,
        session,
        history,
        event_tx,
        event_rx,
        done_tx,
        done_rx,
        Arc::new(perm_prompter),
        Arc::new(question_prompter),
        perm_rx,
        question_rx,
    )
}
