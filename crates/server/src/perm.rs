//! Session-scoped permission asks for protocol v1.
//!
//! The daemon uses one [`ServePrompter`] for the shared [`Agent`]. Each `/v1`
//! run enters [`RUN`] so `ask` knows which session and whether to auto-approve.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::oneshot;
use whycodes_agent::events::{EventSink, TurnEvent};
use whycodes_agent::permission::PermissionPrompter;
use whycodes_agent::question::{AutoAnswerPrompter, QuestionError, QuestionPrompter};
use whycodes_protocol::sdk::{PermissionDecision, QuestionAnswerWire};
use whycodes_tools::question::{QuestionAnswer, QuestionSpec};

tokio::task_local! {
    pub static RUN: RunScope;
}

#[derive(Clone)]
pub struct RunScope {
    pub session_id: String,
    pub auto_approve: bool,
    pub hub: Arc<PermHub>,
}

type QuestionReply = oneshot::Sender<Result<Vec<QuestionAnswer>, QuestionError>>;

pub struct PermHub {
    seq: AtomicU64,
    pending: Mutex<HashMap<(String, String), Pending>>,
    pending_q: Mutex<HashMap<(String, String), QuestionReply>>,
    allow_always: Mutex<HashMap<String, HashSet<String>>>,
    events: Mutex<HashMap<String, EventSink>>,
}

struct Pending {
    tool_name: String,
    reply: oneshot::Sender<bool>,
}

impl PermHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            seq: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            pending_q: Mutex::new(HashMap::new()),
            allow_always: Mutex::new(HashMap::new()),
            events: Mutex::new(HashMap::new()),
        })
    }

    pub fn register_run(&self, session_id: &str, tx: EventSink) {
        if let Ok(mut map) = self.events.lock() {
            map.insert(session_id.to_string(), tx);
        }
    }

    pub fn finish_run(&self, session_id: &str) {
        if let Ok(mut ev) = self.events.lock() {
            ev.remove(session_id);
        }
        let leftover: Vec<oneshot::Sender<bool>> = match self.pending.lock() {
            Ok(mut map) => {
                let keys: Vec<_> = map
                    .keys()
                    .filter(|(sid, _)| sid == session_id)
                    .cloned()
                    .collect();
                keys.into_iter()
                    .filter_map(|k| map.remove(&k).map(|p| p.reply))
                    .collect()
            }
            Err(_poisoned) => Vec::new(),
        };
        for reply in leftover {
            if let Err(_gone) = reply.send(false) {
                // Ask already returned.
            }
        }
        let leftover_q: Vec<_> = match self.pending_q.lock() {
            Ok(mut map) => {
                let keys: Vec<_> = map
                    .keys()
                    .filter(|(sid, _)| sid == session_id)
                    .cloned()
                    .collect();
                keys.into_iter().filter_map(|k| map.remove(&k)).collect()
            }
            Err(_poisoned) => Vec::new(),
        };
        for reply in leftover_q {
            if let Err(_gone) = reply.send(Err(QuestionError::Disconnected)) {
                // Ask already returned.
            }
        }
    }

    pub fn answer_question(
        &self,
        session_id: &str,
        request_id: &str,
        answers: Option<Vec<QuestionAnswerWire>>,
        cancelled: bool,
    ) -> Result<(), String> {
        let tx = {
            let mut map = self
                .pending_q
                .lock()
                .map_err(|_| "question map poisoned".to_string())?;
            map.remove(&(session_id.to_string(), request_id.to_string()))
        };
        let Some(tx) = tx else {
            return Err(format!("unknown question request {request_id}"));
        };
        let payload = if cancelled {
            Err(QuestionError::Cancelled)
        } else {
            let wires = answers.unwrap_or_default();
            Ok(wires
                .into_iter()
                .map(|a| QuestionAnswer {
                    selected: a.selected,
                    free_text: a.free_text,
                })
                .collect())
        };
        tx.send(payload)
            .map_err(|_| "question already timed out".to_string())
    }

    pub fn decide(
        &self,
        session_id: &str,
        request_id: &str,
        decision: PermissionDecision,
    ) -> Result<(), String> {
        let pending = {
            let mut map = self
                .pending
                .lock()
                .map_err(|_| "permission map poisoned".to_string())?;
            map.remove(&(session_id.to_string(), request_id.to_string()))
        };
        let Some(pending) = pending else {
            return Err(format!("unknown permission request {request_id}"));
        };
        let allow = match decision {
            PermissionDecision::Deny => false,
            PermissionDecision::Allow => true,
            PermissionDecision::AllowAlways => {
                if let Ok(mut always) = self.allow_always.lock() {
                    always
                        .entry(session_id.to_string())
                        .or_default()
                        .insert(pending.tool_name.clone());
                }
                true
            }
        };
        pending
            .reply
            .send(allow)
            .map_err(|_| "ask already timed out".to_string())
    }

    fn is_always_allowed(&self, session_id: &str, tool: &str) -> bool {
        self.allow_always
            .lock()
            .ok()
            .and_then(|m| m.get(session_id).map(|s| s.contains(tool)))
            .unwrap_or(false)
    }

    fn emit(&self, session_id: &str, ev: TurnEvent) {
        if let Ok(map) = self.events.lock()
            && let Some(tx) = map.get(session_id)
            && let Err(e) = tx.send(ev)
        {
            tracing::debug!(error = %e, "perm: event channel closed");
        }
    }
}

#[derive(Clone)]
pub struct ServePrompter {
    pub hub: Arc<PermHub>,
}

impl PermissionPrompter for ServePrompter {
    fn ask<'a>(
        &'a self,
        tool_name: &'a str,
        detail: &'a str,
    ) -> whycodes_agent::permission::PermissionAskFuture<'a> {
        Box::pin(async move {
            let scope = match RUN.try_with(Clone::clone) {
                Ok(s) => s,
                Err(_not_in_run) => {
                    // No `/v1` / wrapped `/api` context — refuse rather than
                    // silently approve.
                    return false;
                }
            };
            if scope.auto_approve || scope.hub.is_always_allowed(&scope.session_id, tool_name) {
                return true;
            }

            let request_id = format!("perm-{}", scope.hub.seq.fetch_add(1, Ordering::Relaxed));
            let (tx, rx) = oneshot::channel();
            if let Ok(mut map) = scope.hub.pending.lock() {
                map.insert(
                    (scope.session_id.clone(), request_id.clone()),
                    Pending {
                        tool_name: tool_name.to_string(),
                        reply: tx,
                    },
                );
            } else {
                return false;
            }

            scope.hub.emit(
                &scope.session_id,
                TurnEvent::PermissionAsk {
                    request_id: request_id.clone(),
                    tool_name: tool_name.to_string(),
                    detail: detail.to_string(),
                },
            );

            match tokio::time::timeout(Duration::from_secs(300), rx).await {
                Ok(Ok(allow)) => allow,
                Ok(Err(_dropped)) => false,
                Err(_timeout) => {
                    if let Ok(mut map) = scope.hub.pending.lock() {
                        map.remove(&(scope.session_id.clone(), request_id));
                    }
                    false
                }
            }
        })
    }
}

#[derive(Clone)]
pub struct ServeQuestionPrompter {
    pub hub: Arc<PermHub>,
}

impl QuestionPrompter for ServeQuestionPrompter {
    fn ask(&self, questions: Vec<QuestionSpec>) -> whycodes_agent::question::QuestionAskFuture<'_> {
        Box::pin(async move {
            let scope = match RUN.try_with(Clone::clone) {
                Ok(s) => s,
                Err(_not_in_run) => return Err(QuestionError::Disconnected),
            };
            if scope.auto_approve {
                return AutoAnswerPrompter.ask(questions).await;
            }

            let request_id = format!("q-{}", scope.hub.seq.fetch_add(1, Ordering::Relaxed));
            let (tx, rx) = oneshot::channel();
            if let Ok(mut map) = scope.hub.pending_q.lock() {
                map.insert((scope.session_id.clone(), request_id.clone()), tx);
            } else {
                return Err(QuestionError::Disconnected);
            }

            let payload = serde_json::json!(
                questions
                    .iter()
                    .map(|q| serde_json::json!({
                        "prompt": q.prompt,
                        "multi_select": q.multi_select,
                        "options": q.options.iter().map(|o| serde_json::json!({
                            "label": o.label,
                            "description": o.description,
                        })).collect::<Vec<_>>(),
                    }))
                    .collect::<Vec<_>>()
            );

            scope.hub.emit(
                &scope.session_id,
                TurnEvent::QuestionAsk {
                    request_id: request_id.clone(),
                    questions: payload,
                },
            );

            match tokio::time::timeout(Duration::from_secs(300), rx).await {
                Ok(Ok(r)) => r,
                Ok(Err(_dropped)) => Err(QuestionError::Disconnected),
                Err(_timeout) => {
                    if let Ok(mut map) = scope.hub.pending_q.lock() {
                        map.remove(&(scope.session_id.clone(), request_id));
                    }
                    Err(QuestionError::Timeout)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whycodes_agent::events::TurnEvent;
    use whycodes_tools::question::{QuestionAnswer, QuestionOption, QuestionSpec};

    fn scope(session_id: &str, auto_approve: bool, hub: Arc<PermHub>) -> RunScope {
        RunScope {
            session_id: session_id.into(),
            auto_approve,
            hub,
        }
    }

    #[tokio::test]
    async fn permission_ask_outside_a_run_scope_is_refused() {
        let hub = PermHub::new();
        let prompter = ServePrompter { hub };
        assert!(!prompter.ask("bash", "do a thing").await);
    }

    #[tokio::test]
    async fn permission_ask_with_auto_approve_returns_true_immediately() {
        let hub = PermHub::new();
        let prompter = ServePrompter {
            hub: Arc::clone(&hub),
        };
        let s = scope("s1", true, hub);
        let ok = RUN
            .scope(s, async { prompter.ask("bash", "x").await })
            .await;
        assert!(ok);
    }

    #[tokio::test]
    async fn allow_decision_unblocks_the_ask() {
        let hub = PermHub::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
        hub.register_run("s1", tx);
        let prompter = ServePrompter {
            hub: Arc::clone(&hub),
        };
        let s = scope("s1", false, Arc::clone(&hub));
        let task = tokio::spawn(async move {
            RUN.scope(s, async { prompter.ask("bash", "run ls").await })
                .await
        });
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for permission ask")
            .expect("event channel closed");
        let TurnEvent::PermissionAsk {
            request_id,
            tool_name,
            detail,
        } = ev
        else {
            panic!("expected PermissionAsk, got {ev:?}");
        };
        assert_eq!(tool_name, "bash");
        assert_eq!(detail, "run ls");
        hub.decide("s1", &request_id, PermissionDecision::Allow)
            .unwrap();
        assert!(task.await.expect("ask task panicked"));
    }

    #[tokio::test]
    async fn deny_decision_unblocks_the_ask_with_false() {
        let hub = PermHub::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
        hub.register_run("s1", tx);
        let prompter = ServePrompter {
            hub: Arc::clone(&hub),
        };
        let s = scope("s1", false, Arc::clone(&hub));
        let task = tokio::spawn(async move {
            RUN.scope(s, async { prompter.ask("bash", "x").await })
                .await
        });
        let ev = rx.recv().await.expect("no permission ask");
        let TurnEvent::PermissionAsk { request_id, .. } = ev else {
            panic!("expected PermissionAsk");
        };
        hub.decide("s1", &request_id, PermissionDecision::Deny)
            .unwrap();
        assert!(!task.await.expect("ask task panicked"));
    }

    #[tokio::test]
    async fn allow_always_skips_future_asks() {
        let hub = PermHub::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
        hub.register_run("s1", tx);
        let prompter = ServePrompter {
            hub: Arc::clone(&hub),
        };
        let s = scope("s1", false, Arc::clone(&hub));
        let prompter_for_task = prompter.clone();
        let task = tokio::spawn(async move {
            RUN.scope(s, async { prompter_for_task.ask("bash", "first").await })
                .await
        });
        let ev = rx.recv().await.expect("no permission ask");
        let TurnEvent::PermissionAsk { request_id, .. } = ev else {
            panic!("expected PermissionAsk");
        };
        hub.decide("s1", &request_id, PermissionDecision::AllowAlways)
            .unwrap();
        assert!(task.await.expect("ask task panicked"));

        // Second ask for the same tool auto-approves without a new event.
        let s2 = scope("s1", false, Arc::clone(&hub));
        let ok = RUN
            .scope(s2, async { prompter.ask("bash", "second").await })
            .await;
        assert!(ok);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn deciding_an_unknown_request_is_an_error() {
        let hub = PermHub::new();
        let err = hub
            .decide("s1", "perm-999", PermissionDecision::Allow)
            .unwrap_err();
        assert!(err.contains("unknown permission request"));
    }

    #[tokio::test]
    async fn finish_run_resolves_pending_asks_as_denied() {
        let hub = PermHub::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
        hub.register_run("s1", tx);
        let prompter = ServePrompter {
            hub: Arc::clone(&hub),
        };
        let s = scope("s1", false, Arc::clone(&hub));
        let task = tokio::spawn(async move {
            RUN.scope(s, async { prompter.ask("bash", "x").await })
                .await
        });
        let _ = rx.recv().await.expect("no permission ask");
        hub.finish_run("s1");
        assert!(!task.await.expect("ask task panicked"));
        // A second finish for the same session is a no-op.
        hub.finish_run("s1");
    }

    #[tokio::test]
    async fn answer_question_resolves_the_ask() {
        let hub = PermHub::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
        hub.register_run("s1", tx);
        let prompter = ServeQuestionPrompter {
            hub: Arc::clone(&hub),
        };
        let s = scope("s1", false, Arc::clone(&hub));
        let questions = vec![QuestionSpec {
            prompt: "Pick a color".into(),
            options: vec![QuestionOption {
                label: "red".into(),
                description: "the reddest".into(),
                preview: None,
            }],
            multi_select: false,
        }];
        let task =
            tokio::spawn(
                async move { RUN.scope(s, async { prompter.ask(questions).await }).await },
            );
        let ev = rx.recv().await.expect("no question ask");
        let TurnEvent::QuestionAsk {
            request_id,
            questions,
        } = ev
        else {
            panic!("expected QuestionAsk");
        };
        assert_eq!(questions[0]["prompt"], "Pick a color");
        assert_eq!(questions[0]["options"][0]["label"], "red");
        hub.answer_question(
            "s1",
            &request_id,
            Some(vec![QuestionAnswerWire {
                selected: vec!["red".into()],
                free_text: None,
            }]),
            false,
        )
        .unwrap();
        let answers = task.await.expect("ask task panicked").unwrap();
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].selected, vec!["red".to_string()]);
    }

    #[tokio::test]
    async fn cancelled_answer_maps_to_a_cancelled_error() {
        let hub = PermHub::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
        hub.register_run("s1", tx);
        let prompter = ServeQuestionPrompter {
            hub: Arc::clone(&hub),
        };
        let s = scope("s1", false, Arc::clone(&hub));
        let task =
            tokio::spawn(
                async move { RUN.scope(s, async { prompter.ask(Vec::new()).await }).await },
            );
        let ev = rx.recv().await.expect("no question ask");
        let TurnEvent::QuestionAsk { request_id, .. } = ev else {
            panic!("expected QuestionAsk");
        };
        hub.answer_question("s1", &request_id, None, true).unwrap();
        assert_eq!(
            task.await.expect("ask task panicked").unwrap_err(),
            QuestionError::Cancelled
        );
    }

    #[tokio::test]
    async fn answering_an_unknown_question_is_an_error() {
        let hub = PermHub::new();
        let err = hub.answer_question("s1", "q-999", None, false).unwrap_err();
        assert!(err.contains("unknown question request"));
    }

    #[tokio::test]
    async fn question_ask_with_auto_approve_short_circuits() {
        let hub = PermHub::new();
        let prompter = ServeQuestionPrompter {
            hub: Arc::clone(&hub),
        };
        let s = scope("s1", true, Arc::clone(&hub));
        let questions = vec![QuestionSpec {
            prompt: "ok?".into(),
            options: vec![QuestionOption {
                label: "yes".into(),
                description: String::new(),
                preview: None,
            }],
            multi_select: false,
        }];
        let answers = RUN
            .scope(s, async { prompter.ask(questions).await })
            .await
            .unwrap();
        assert_eq!(answers.len(), 1);
        // AutoAnswerPrompter selects the first option by default.
        assert_eq!(
            answers[0],
            QuestionAnswer {
                selected: vec!["yes".to_string()],
                free_text: None,
            }
        );
    }

    #[tokio::test]
    async fn question_ask_outside_a_run_scope_is_disconnected() {
        let hub = PermHub::new();
        let prompter = ServeQuestionPrompter { hub };
        assert_eq!(
            prompter.ask(Vec::new()).await.unwrap_err(),
            QuestionError::Disconnected
        );
    }
}
