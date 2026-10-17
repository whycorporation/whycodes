use super::*;
use crate::cmd::helpers::IsolatedHome;
use tokio::io::{AsyncWriteExt, BufReader, DuplexStream, Lines};
use whycodes_llm::{ProviderRegistry, ScriptedProvider, ScriptedStep};

/// Scripted agents: one step list per LLM call.
struct TestFactory {
    batches: Vec<Vec<ScriptedStep>>,
}

impl Factory for TestFactory {
    async fn build(&self, cwd: &Path, resume: Option<&str>) -> anyhow::Result<Built> {
        if resume.is_some() {
            anyhow::bail!("resume is not scripted");
        }
        let config = Config::default();
        let info = config.get_agent("build").cloned().expect("build agent");
        let mut agent = Agent::new(info).with_config(&config);
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(ScriptedProvider::batched(
            "script",
            self.batches.clone(),
        )));
        agent.set_provider_registry(registry);
        agent.set_route("script", "acp-test-model");
        Ok(Built {
            agent,
            session: Session::new(cwd.to_path_buf(), "sys".into()),
            route: Route {
                provider: "script".into(),
                model: "acp-test-model".into(),
                api_key: String::new(),
            },
        })
    }
}

struct Client {
    tx: DuplexStream,
    rx: Lines<BufReader<DuplexStream>>,
    server: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Client {
    fn start<F: Factory + Send + 'static>(factory: F) -> Self {
        let (tx, server_in) = tokio::io::duplex(1 << 16);
        let (server_out, rx) = tokio::io::duplex(1 << 16);
        let server = tokio::spawn(serve(BufReader::new(server_in), server_out, factory));
        Self {
            tx,
            rx: BufReader::new(rx).lines(),
            server,
        }
    }

    async fn send_raw(&mut self, line: &str) {
        self.tx.write_all(line.as_bytes()).await.unwrap();
        self.tx.write_all(b"\n").await.unwrap();
    }

    async fn send(&mut self, msg: Value) {
        self.send_raw(&msg.to_string()).await;
    }

    async fn call(&mut self, id: u64, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
    }

    async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(20), self.rx.next_line())
            .await
            .expect("timed out waiting for the server")
            .unwrap()
            .expect("server closed");
        serde_json::from_str(&line).unwrap()
    }

    /// Messages until the response to `id` (returned last).
    async fn until(&mut self, id: u64) -> (Vec<Value>, Value) {
        let mut before = Vec::new();
        loop {
            let msg = self.recv().await;
            if msg.get("id") == Some(&json!(id)) && msg.get("method").is_none() {
                return (before, msg);
            }
            before.push(msg);
        }
    }

    async fn close(self) {
        drop(self.tx);
        self.server.await.unwrap().unwrap();
    }
}

fn kinds(updates: &[Value]) -> Vec<String> {
    updates
        .iter()
        .filter_map(|m| m["params"]["update"]["sessionUpdate"].as_str())
        .map(str::to_string)
        .collect()
}

async fn new_session(c: &mut Client, cwd: &Path) -> String {
    c.call(1, "session/new", json!({"cwd": cwd, "mcpServers": []}))
        .await;
    let (_, resp) = c.until(1).await;
    resp["result"]["sessionId"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn handshake_and_protocol_errors() {
    let mut c = Client::start(TestFactory { batches: vec![] });
    c.call(0, "initialize", json!({"protocolVersion": 1})).await;
    let init = c.recv().await;
    assert_eq!(init["result"]["protocolVersion"], 1);
    assert_eq!(init["result"]["agentCapabilities"]["loadSession"], true);
    assert_eq!(init["result"]["agentInfo"]["name"], "whycodes");

    c.call(1, "authenticate", json!({"methodId": "x"})).await;
    assert_eq!(c.recv().await["result"], json!({}));
    c.send_raw("").await;
    c.send_raw("{not json").await;
    assert_eq!(c.recv().await["error"]["code"], PARSE_ERROR);
    // Unknown notification and a stray response are ignored.
    c.send(json!({"jsonrpc": "2.0", "method": "session/whatever"}))
        .await;
    c.send(json!({"jsonrpc": "2.0", "id": 5, "result": {}}))
        .await;
    c.call(2, "session/set_mode", json!({})).await;
    assert_eq!(c.recv().await["error"]["code"], METHOD_NOT_FOUND);
    c.call(3, "session/new", json!({})).await;
    assert_eq!(c.recv().await["error"]["code"], INVALID_PARAMS);
    c.call(4, "session/load", json!({"cwd": "/x"})).await;
    assert_eq!(c.recv().await["error"]["code"], INVALID_PARAMS);
    c.call(6, "session/load", json!({"cwd": "/x", "sessionId": "s"}))
        .await;
    let failed = c.recv().await;
    assert_eq!(failed["error"]["code"], INTERNAL_ERROR);
    assert!(
        failed["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not scripted")
    );
    c.call(
        7,
        "session/prompt",
        json!({"sessionId": "nope", "prompt": []}),
    )
    .await;
    assert_eq!(c.recv().await["error"]["code"], INVALID_PARAMS);
    c.close().await;
}

#[tokio::test]
async fn prompt_streams_text_and_tool_calls() {
    let _home = IsolatedHome::new();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
    let mut c = Client::start(TestFactory {
        batches: vec![
            vec![ScriptedStep::ToolCall {
                id: "t1".into(),
                name: "read".into(),
                input: json!({"path": "a.txt"}),
            }],
            vec![
                ScriptedStep::Thinking("reading".into()),
                ScriptedStep::Text("done".into()),
            ],
        ],
    });
    let sid = new_session(&mut c, dir.path()).await;

    c.call(2, "session/prompt", json!({"sessionId": sid, "prompt": []}))
        .await;
    assert_eq!(c.recv().await["error"]["message"], "empty prompt");

    c.call(
        3,
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type": "text", "text": "read a.txt"}]}),
    )
    .await;
    let (updates, resp) = c.until(3).await;
    assert_eq!(resp["result"]["stopReason"], "end_turn", "{resp}");
    let kinds = kinds(&updates);
    assert!(kinds.contains(&"tool_call".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"tool_call_update".to_string()), "{kinds:?}");
    assert!(
        kinds.contains(&"agent_message_chunk".to_string()),
        "{kinds:?}"
    );
    let done = updates
        .iter()
        .find(|m| m["params"]["update"]["sessionUpdate"] == "tool_call_update")
        .unwrap();
    assert_eq!(done["params"]["sessionId"], sid.as_str());
    assert_eq!(done["params"]["update"]["toolCallId"], "t1");
    c.close().await;
}

#[tokio::test]
async fn cancel_stops_a_running_prompt_and_busy_sessions_refuse() {
    let _home = IsolatedHome::new();
    let dir = tempfile::tempdir().unwrap();
    let mut c = Client::start(TestFactory {
        batches: vec![vec![ScriptedStep::Hang(Duration::from_secs(60))]],
    });
    let sid = new_session(&mut c, dir.path()).await;
    let prompt = json!({"sessionId": sid, "prompt": [{"type": "text", "text": "wait"}]});
    c.call(2, "session/prompt", prompt.clone()).await;
    c.call(3, "session/prompt", prompt).await;
    let (_, busy) = c.until(3).await;
    assert_eq!(busy["error"]["message"], "a prompt is already running");
    // Unknown session ids are ignored.
    c.send(json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": "x"}}))
        .await;
    c.send(json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": sid}}))
        .await;
    let (_, resp) = c.until(2).await;
    assert_eq!(resp["result"]["stopReason"], "cancelled", "{resp}");
    c.close().await;
}

#[tokio::test]
async fn closing_input_cancels_running_prompts() {
    let _home = IsolatedHome::new();
    let dir = tempfile::tempdir().unwrap();
    let mut c = Client::start(TestFactory {
        batches: vec![vec![ScriptedStep::Hang(Duration::from_secs(60))]],
    });
    let sid = new_session(&mut c, dir.path()).await;
    c.call(
        2,
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type": "text", "text": "wait"}]}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    tokio::time::timeout(Duration::from_secs(20), c.close())
        .await
        .expect("shutdown waits only for the cancelled turn");
}

#[tokio::test]
async fn failed_turns_answer_with_an_error() {
    let _home = IsolatedHome::new();
    let dir = tempfile::tempdir().unwrap();
    // A file where the data dir should be: the save after the turn fails
    // and is only logged.
    let blocked = dir.path().join("home-is-a-file");
    std::fs::write(&blocked, "").unwrap();
    unsafe { std::env::set_var("WHYCODES_HOME", &blocked) };
    let mut c = Client::start(TestFactory {
        batches: vec![vec![ScriptedStep::FailOpen("provider down".into())]],
    });
    let sid = new_session(&mut c, dir.path()).await;
    c.call(
        2,
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type": "text", "text": "hi"}]}),
    )
    .await;
    let (_, resp) = c.until(2).await;
    assert_eq!(resp["error"]["code"], INTERNAL_ERROR, "{resp}");
    c.close().await;
}

fn prompter(out: &Outbox) -> AcpPrompter {
    AcpPrompter {
        out: out.clone(),
        session_id: "s1".into(),
        open_tools: OpenTools::default(),
        always: Mutex::new(HashSet::new()),
    }
}

/// Run `ask` and answer its permission request with `reply`.
async fn answer(
    p: &Arc<AcpPrompter>,
    out: &Outbox,
    rx: &mut mpsc::UnboundedReceiver<Value>,
    tool: &'static str,
    reply: Value,
) -> (Value, bool) {
    let asking = Arc::clone(p);
    let task = tokio::spawn(async move { asking.ask(tool, "detail").await });
    let req = rx.recv().await.unwrap();
    out.resolve(&json!({"id": req["id"], "result": reply}));
    (req, task.await.unwrap())
}

#[tokio::test]
async fn permission_requests_follow_the_users_choice() {
    let (out, mut rx) = Outbox::new();
    let p = Arc::new(prompter(&out));
    lock(&p.open_tools).push(("bash".into(), "call-7".into()));

    let selected = |id: &str| json!({"outcome": {"outcome": "selected", "optionId": id}});
    let (req, allowed) = answer(&p, &out, &mut rx, "bash", selected("allow")).await;
    assert!(allowed);
    assert_eq!(req["method"], "session/request_permission");
    assert_eq!(req["params"]["sessionId"], "s1");
    assert_eq!(req["params"]["toolCall"]["toolCallId"], "call-7");
    assert_eq!(req["params"]["options"][1]["kind"], "allow_always");

    let (_, rejected) = answer(&p, &out, &mut rx, "bash", selected("reject")).await;
    assert!(!rejected);
    let (_, cancelled) = answer(
        &p,
        &out,
        &mut rx,
        "bash",
        json!({"outcome": {"outcome": "cancelled"}}),
    )
    .await;
    assert!(!cancelled);

    let (_, always) = answer(&p, &out, &mut rx, "bash", selected("always")).await;
    assert!(always);
    // Remembered: no request this time.
    assert!(p.ask("bash", "again").await);
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn permission_without_a_tool_call_uses_a_fresh_id() {
    let (out, _rx) = Outbox::new();
    let p = prompter(&out);
    assert_eq!(p.tool_call_id("write").await, "permission-write");
}

/// Restores the env the CLI factory test changes.
struct LlmEnv;

impl Drop for LlmEnv {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("WHYCODES_TEST_LLM");
            std::env::remove_var("ANTHROPIC_API_KEY");
        }
    }
}

fn test_cli() -> Cli {
    Cli {
        command: None,
        provider: Some("anthropic".into()),
        model: None,
        agent_flag: None,
        dir: None,
        plain: true,
        continue_session: false,
        resume: None,
        fork_session: false,
        debug: false,
        no_auto_update: true,
        prewalk: false,
        no_memory: true,
    }
}

#[tokio::test]
async fn cli_factory_builds_saves_and_loads_sessions() {
    let _home = IsolatedHome::new();
    let _env = LlmEnv;
    let dir = tempfile::tempdir().unwrap();
    let cli = test_cli();
    unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
    let missing = CliFactory { cli: &cli }.build(dir.path(), None).await;
    assert!(missing.is_err(), "no API key");
    let mut keyless = test_cli();
    keyless.provider = Some("ollama".into());
    let local = CliFactory { cli: &keyless }
        .build(dir.path(), None)
        .await
        .unwrap();
    assert!(local.route.api_key.is_empty());

    unsafe {
        std::env::set_var("WHYCODES_TEST_LLM", "acp-reply");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-acp-test");
    }
    let (client_tx, server_in) = tokio::io::duplex(1 << 16);
    let (server_out, client_rx) = tokio::io::duplex(1 << 16);
    let mut c = Client {
        tx: client_tx,
        rx: BufReader::new(client_rx).lines(),
        server: tokio::spawn(async { Ok(()) }),
    };
    let serving = serve(
        BufReader::new(server_in),
        server_out,
        CliFactory { cli: &cli },
    );
    let talk = async {
        let sid = new_session(&mut c, dir.path()).await;
        c.call(
            2,
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": "hello"}]}),
        )
        .await;
        let (updates, resp) = c.until(2).await;
        assert_eq!(resp["result"]["stopReason"], "end_turn", "{resp}");
        assert!(kinds(&updates).contains(&"agent_message_chunk".to_string()));

        c.call(
            3,
            "session/load",
            json!({"sessionId": sid, "cwd": dir.path(), "mcpServers": []}),
        )
        .await;
        let (replay, loaded) = c.until(3).await;
        assert_eq!(loaded["result"], Value::Null, "{loaded}");
        assert_eq!(
            kinds(&replay),
            ["user_message_chunk", "agent_message_chunk"]
        );
        c.call(
            4,
            "session/load",
            json!({"sessionId": "missing", "cwd": dir.path(), "mcpServers": []}),
        )
        .await;
        let (_, gone) = c.until(4).await;
        assert!(
            gone["error"]["message"]
                .as_str()
                .unwrap()
                .contains("no saved session")
        );
        drop(c.tx);
    };
    let (served, ()) = tokio::join!(serving, talk);
    served.unwrap();
}
