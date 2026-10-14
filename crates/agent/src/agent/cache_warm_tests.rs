use super::*;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use whycodes_core::types::{
    AgentInfo, AgentMode, ContentBlock, LlmRequest, MessageContent, PermissionSet,
};
use whycodes_llm::{LlmProvider, ProviderRegistry, ScriptedProvider, ScriptedStep};

/// Records every request, then streams the scripted steps.
struct Capture {
    inner: ScriptedProvider,
    seen: Arc<Mutex<Vec<LlmRequest>>>,
}

impl LlmProvider for Capture {
    fn name(&self) -> &str {
        "script"
    }
    fn default_base_url(&self) -> &str {
        "http://script.invalid"
    }
    fn complete<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> whycodes_llm::provider::ProviderResponseFuture<'a> {
        self.inner.complete(request, api_key, model)
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> whycodes_llm::provider::ProviderStreamFuture<'a> {
        self.seen.lock().unwrap().push(request.clone());
        self.inner.stream(request, api_key, model)
    }
}

fn agent(steps: Vec<ScriptedStep>) -> (Agent, Arc<Mutex<Vec<LlmRequest>>>) {
    let seen = Arc::default();
    let mut registry = ProviderRegistry::new();
    registry.register(Box::new(Capture {
        inner: ScriptedProvider::new(steps),
        seen: Arc::clone(&seen),
    }));
    let info = AgentInfo {
        name: "build".into(),
        description: String::new(),
        mode: AgentMode::Primary,
        permission: PermissionSet::default(),
        model: None,
        system_prompt: Some("sys".into()),
        temperature: None,
        top_p: None,
    };
    (Agent::new(info).with_provider_registry(registry), seen)
}

fn chatted() -> Session {
    let mut s = Session::new(std::path::PathBuf::from("/work/proj"), "sys".into());
    s.add_user_message("hello");
    s.add_assistant_message(vec![ContentBlock::Text {
        text: "hi there".into(),
    }]);
    s
}

#[test]
fn due_every_interval_inside_the_idle_window() {
    let ended = Instant::now();
    let window = Duration::from_secs(30 * 60);
    let at = |secs| ended + Duration::from_secs(secs);
    assert!(!warm_due(at(60), ended, None, window));
    assert!(warm_due(at(270), ended, None, window));
    assert!(!warm_due(at(300), ended, Some(at(270)), window));
    assert!(warm_due(at(540), ended, Some(at(270)), window));
    assert!(
        !warm_due(at(1800), ended, Some(at(1500)), window),
        "window over"
    );
    assert!(!warm_due(at(270), ended, None, Duration::ZERO), "off");
    // A warm from before this turn ended does not count.
    assert!(!warm_due(
        at(100),
        ended,
        Some(ended - Duration::from_secs(600)),
        window
    ));
}

#[tokio::test]
async fn keep_alive_repeats_the_last_request_and_reports_usage() {
    let (a, seen) = agent(vec![
        ScriptedStep::Usage {
            input_tokens: 12,
            output_tokens: 0,
        },
        ScriptedStep::CacheUsage {
            creation_input_tokens: 0,
            read_input_tokens: 900,
        },
        ScriptedStep::Text("x".into()),
        ScriptedStep::Usage {
            input_tokens: 1000,
            output_tokens: 1000,
        },
    ]);
    let session = chatted();
    let job = a.cache_warm_job(&session, "script", "m", "k").expect("job");
    let usage = job.await.expect("usage");
    assert_eq!(usage.input_tokens, 12, "stops at the first output");
    assert_eq!(usage.cache_read_input_tokens, Some(900));
    assert_eq!(usage.cache_creation_input_tokens, Some(0));

    let request = seen.lock().unwrap()[0].clone();
    assert!(request.use_prompt_cache);
    let last = request.messages.last().unwrap();
    assert_eq!(last.role, Role::User);
    assert!(matches!(&last.content, MessageContent::Text(t) if t.starts_with("hello")));
}

#[tokio::test]
async fn no_keep_alive_without_cache_history_or_provider() {
    let (mut a, _) = agent(Vec::new());
    let empty = Session::new(std::path::PathBuf::from("/work/proj"), "sys".into());
    assert!(a.cache_warm_job(&empty, "script", "m", "k").is_none());

    let job = a.cache_warm_job(&chatted(), "nobody", "m", "k").unwrap();
    assert!(job.await.is_none(), "unknown provider");

    a.use_prompt_cache = false;
    assert!(a.cache_warm_job(&chatted(), "script", "m", "k").is_none());
}

#[tokio::test]
async fn failed_open_yields_nothing() {
    let (a, _) = agent(vec![ScriptedStep::FailOpen("401 invalid x-api-key".into())]);
    let job = a.cache_warm_job(&chatted(), "script", "m", "k").unwrap();
    assert!(job.await.is_none());
}

#[test]
fn recorded_keep_alive_adds_usage_and_cost() {
    let (a, _) = agent(Vec::new());
    let mut session = chatted();
    let usage = Usage {
        input_tokens: 5,
        output_tokens: 1,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: Some(100),
    };
    a.record_cache_warm(&mut session, "script", "unpriced-model", &usage);
    assert_eq!(session.cost.unpriced_steps, 1);
}
