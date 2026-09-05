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
use whycodes_agent::notify::{NotifyHandle, spawn_need_input_wait};
use whycodes_agent::permission::PermissionPrompter;
use whycodes_agent::question::{AutoAnswerPrompter, QuestionError, QuestionPrompter};
use whycodes_protocol::sdk::{PermissionDecision, QuestionAnswerWire};
use whycodes_tools::question::{QuestionAnswer, QuestionSpec, validate_answers};

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

struct PendingQuestion {
    questions: Vec<QuestionSpec>,
    reply: QuestionReply,
}

pub struct PermHub {
    seq: AtomicU64,
    pending: Mutex<HashMap<(String, String), Pending>>,
    pending_q: Mutex<HashMap<(String, String), PendingQuestion>>,
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
                keys.into_iter()
                    .filter_map(|k| map.remove(&k).map(|p| p.reply))
                    .collect()
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
        let key = (session_id.to_string(), request_id.to_string());
        let mut map = self
            .pending_q
            .lock()
            .map_err(|_| "question map poisoned".to_string())?;
        if cancelled {
            let Some(pending) = map.remove(&key) else {
                return Err(format!("unknown question request {request_id}"));
            };
            drop(map);
            return pending
                .reply
                .send(Err(QuestionError::Cancelled))
                .map_err(|_| "question already timed out".to_string());
        }
        let Some(pending) = map.get(&key) else {
            return Err(format!("unknown question request {request_id}"));
        };
        let answers: Vec<QuestionAnswer> = answers
            .unwrap_or_default()
            .into_iter()
            .map(|a| QuestionAnswer {
                selected: a.selected,
                free_text: a.free_text,
                auto_picked: false,
            })
            .collect();
        validate_answers(&pending.questions, &answers)?;
        let pending = map
            .remove(&key)
            .ok_or_else(|| format!("unknown question request {request_id}"))?;
        drop(map);
        pending
            .reply
            .send(Ok(answers))
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
    pub timeout: Option<Duration>,
    pub notify: Option<NotifyHandle>,
}

impl ServeQuestionPrompter {
    pub fn new(hub: Arc<PermHub>) -> Self {
        Self {
            hub,
            timeout: Some(Duration::from_secs(
                whycodes_config::QuestionToolConfig::default()
                    .timeout_secs
                    .max(1),
            )),
            notify: None,
        }
    }
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

            if let Some(cfg) = self.notify.as_deref() {
                let detail = questions
                    .iter()
                    .map(|q| q.prompt.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                spawn_need_input_wait(cfg, "Question", &detail);
            }

            let request_id = format!("q-{}", scope.hub.seq.fetch_add(1, Ordering::Relaxed));
            let (tx, rx) = oneshot::channel();
            if let Ok(mut map) = scope.hub.pending_q.lock() {
                map.insert(
                    (scope.session_id.clone(), request_id.clone()),
                    PendingQuestion {
                        questions: questions.clone(),
                        reply: tx,
                    },
                );
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
                            "preview": o.preview,
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

            match self.timeout {
                Some(dur) => match tokio::time::timeout(dur, rx).await {
                    Ok(Ok(r)) => r,
                    Ok(Err(_dropped)) => Err(QuestionError::Disconnected),
                    Err(_timeout) => {
                        if let Ok(mut map) = scope.hub.pending_q.lock() {
                            map.remove(&(scope.session_id.clone(), request_id));
                        }
                        Err(QuestionError::Timeout)
                    }
                },
                None => rx.await.unwrap_or(Err(QuestionError::Disconnected)),
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
        let prompter = ServeQuestionPrompter::new(Arc::clone(&hub));
        let s = scope("s1", false, Arc::clone(&hub));
        let questions = vec![QuestionSpec {
            prompt: "Pick a color".into(),
            options: vec![QuestionOption {
                label: "red".into(),
                description: "the reddest".into(),
                preview: Some("#f00".into()),
            }],
            multi_select: false,
            important: false,
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
        assert_eq!(questions[0]["options"][0]["preview"], "#f00");
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
        let prompter = ServeQuestionPrompter::new(Arc::clone(&hub));
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
    async fn answer_question_rejects_unknown_labels() {
        let hub = PermHub::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
        hub.register_run("s1", tx);
        let prompter = ServeQuestionPrompter::new(Arc::clone(&hub));
        let s = scope("s1", false, Arc::clone(&hub));
        let questions = vec![QuestionSpec {
            prompt: "Pick".into(),
            options: vec![QuestionOption {
                label: "A".into(),
                description: String::new(),
                preview: None,
            }],
            multi_select: false,
            important: false,
        }];
        let task =
            tokio::spawn(
                async move { RUN.scope(s, async { prompter.ask(questions).await }).await },
            );
        let ev = rx.recv().await.expect("no question ask");
        let TurnEvent::QuestionAsk { request_id, .. } = ev else {
            panic!("expected QuestionAsk");
        };
        let err = hub
            .answer_question(
                "s1",
                &request_id,
                Some(vec![QuestionAnswerWire {
                    selected: vec!["nope".into()],
                    free_text: None,
                }]),
                false,
            )
            .unwrap_err();
        assert!(err.contains("unknown option"), "{err}");
        hub.answer_question("s1", &request_id, None, true).unwrap();
        assert_eq!(
            task.await.expect("ask task panicked").unwrap_err(),
            QuestionError::Cancelled
        );
    }

    #[tokio::test]
    async fn question_ask_with_auto_approve_short_circuits() {
        let hub = PermHub::new();
        let prompter = ServeQuestionPrompter::new(Arc::clone(&hub));
        let s = scope("s1", true, Arc::clone(&hub));
        let questions = vec![QuestionSpec {
            prompt: "ok?".into(),
            options: vec![QuestionOption {
                label: "yes".into(),
                description: String::new(),
                preview: None,
            }],
            multi_select: false,
            important: false,
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
                auto_picked: true,
            }
        );
    }

    #[tokio::test]
    async fn question_ask_outside_a_run_scope_is_disconnected() {
        let hub = PermHub::new();
        let prompter = ServeQuestionPrompter::new(hub);
        assert_eq!(
            prompter.ask(Vec::new()).await.unwrap_err(),
            QuestionError::Disconnected
        );
    }

    fn poison_mutex<T>(m: &Mutex<T>) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = m.lock().unwrap();
            panic!("poison");
        }));
    }

    #[tokio::test]
    async fn finish_run_resolves_pending_questions() {
        let hub = PermHub::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
        hub.register_run("s1", tx);
        let prompter = ServeQuestionPrompter::new(Arc::clone(&hub));
        let s = scope("s1", false, Arc::clone(&hub));
        let task =
            tokio::spawn(
                async move { RUN.scope(s, async { prompter.ask(Vec::new()).await }).await },
            );
        let _ = rx.recv().await.expect("no question ask");
        hub.finish_run("s1");
        assert_eq!(
            task.await.expect("ask task panicked").unwrap_err(),
            QuestionError::Disconnected
        );
    }

    #[tokio::test]
    async fn dropped_reply_denies_permission_and_disconnects_question() {
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
        let pending = {
            let mut map = hub.pending.lock().unwrap();
            let key = map.keys().next().cloned().expect("pending key");
            map.remove(&key)
        };
        drop(pending);
        assert!(!task.await.expect("ask task panicked"));

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
        hub.register_run("s1", tx);
        let q = ServeQuestionPrompter::new(Arc::clone(&hub));
        let s = scope("s1", false, Arc::clone(&hub));
        let task =
            tokio::spawn(async move { RUN.scope(s, async { q.ask(Vec::new()).await }).await });
        let _ = rx.recv().await.expect("no question ask");
        let pending = {
            let mut map = hub.pending_q.lock().unwrap();
            let key = map.keys().next().cloned().expect("pending key");
            map.remove(&key)
        };
        drop(pending);
        assert_eq!(
            task.await.expect("ask task panicked").unwrap_err(),
            QuestionError::Disconnected
        );
    }

    #[test]
    fn decide_and_answer_fail_after_the_asker_is_gone() {
        let hub = PermHub::new();
        let (tx, rx) = oneshot::channel();
        drop(rx);
        hub.pending.lock().unwrap().insert(
            ("s1".into(), "perm-1".into()),
            Pending {
                tool_name: "bash".into(),
                reply: tx,
            },
        );
        let err = hub
            .decide("s1", "perm-1", PermissionDecision::Allow)
            .unwrap_err();
        assert!(err.contains("timed out"), "{err}");

        let (tx, rx) = oneshot::channel::<Result<Vec<QuestionAnswer>, QuestionError>>();
        drop(rx);
        hub.pending_q.lock().unwrap().insert(
            ("s1".into(), "q-1".into()),
            PendingQuestion {
                questions: Vec::new(),
                reply: tx,
            },
        );
        let err = hub.answer_question("s1", "q-1", None, false).unwrap_err();
        assert!(err.contains("timed out"), "{err}");
    }

    #[test]
    fn emit_ignores_closed_and_poisoned_event_maps() {
        let hub = PermHub::new();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        hub.register_run("s1", tx);
        drop(rx);
        hub.emit("s1", TurnEvent::Cancelled);

        poison_mutex(&hub.events);
        hub.register_run("s2", tokio::sync::mpsc::unbounded_channel().0);
        hub.finish_run("s2");
        hub.emit("s2", TurnEvent::Cancelled);
    }

    #[test]
    fn poisoned_maps_are_safe_noops_or_errors() {
        let hub = PermHub::new();
        poison_mutex(&hub.pending);
        poison_mutex(&hub.pending_q);
        poison_mutex(&hub.allow_always);
        hub.finish_run("s1");
        assert!(!hub.is_always_allowed("s1", "bash"));
        assert!(
            hub.decide("s1", "x", PermissionDecision::Allow)
                .unwrap_err()
                .contains("poisoned")
        );
        assert!(
            hub.answer_question("s1", "x", None, false)
                .unwrap_err()
                .contains("poisoned")
        );
    }

    #[tokio::test]
    async fn poisoned_pending_refuses_new_asks() {
        let hub = PermHub::new();
        poison_mutex(&hub.pending);
        poison_mutex(&hub.pending_q);
        let p = ServePrompter {
            hub: Arc::clone(&hub),
        };
        let q = ServeQuestionPrompter::new(Arc::clone(&hub));
        let s = scope("s1", false, Arc::clone(&hub));
        assert!(
            !RUN.scope(s.clone(), async { p.ask("bash", "x").await })
                .await
        );
        assert_eq!(
            RUN.scope(s, async { q.ask(Vec::new()).await })
                .await
                .unwrap_err(),
            QuestionError::Disconnected
        );
    }

    #[tokio::test]
    async fn allow_always_survives_poisoned_set_and_still_allows() {
        let hub = PermHub::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
        hub.register_run("s1", tx);
        poison_mutex(&hub.allow_always);
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
        hub.decide("s1", &request_id, PermissionDecision::AllowAlways)
            .unwrap();
        assert!(task.await.expect("ask task panicked"));
    }

    #[test]
    fn leftover_send_is_ignored_when_asker_already_returned() {
        let hub = PermHub::new();
        let (tx, rx) = oneshot::channel();
        drop(rx);
        hub.pending.lock().unwrap().insert(
            ("s1".into(), "perm-gone".into()),
            Pending {
                tool_name: "bash".into(),
                reply: tx,
            },
        );
        let (qtx, qrx) = oneshot::channel();
        drop(qrx);
        hub.pending_q.lock().unwrap().insert(
            ("s1".into(), "q-gone".into()),
            PendingQuestion {
                questions: Vec::new(),
                reply: qtx,
            },
        );
        hub.finish_run("s1");
    }

    #[tokio::test]
    async fn permission_and_question_asks_time_out() {
        tokio::time::pause();
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
        tokio::time::advance(Duration::from_secs(301)).await;
        assert!(!task.await.expect("ask task panicked"));

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
        hub.register_run("s1", tx);
        let mut q = ServeQuestionPrompter::new(Arc::clone(&hub));
        q.timeout = Some(Duration::from_secs(300));
        let s = scope("s1", false, Arc::clone(&hub));
        let task =
            tokio::spawn(async move { RUN.scope(s, async { q.ask(Vec::new()).await }).await });
        let _ = rx.recv().await.expect("no question ask");
        tokio::time::advance(Duration::from_secs(301)).await;
        assert_eq!(
            task.await.expect("ask task panicked").unwrap_err(),
            QuestionError::Timeout
        );
    }

    #[test]
    fn is_always_allowed_is_false_without_an_entry() {
        let hub = PermHub::new();
        assert!(!hub.is_always_allowed("missing", "bash"));
    }

    #[tokio::test]
    async fn timeout_cleanup_tolerates_poisoned_pending_maps() {
        tokio::time::pause();
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
        poison_mutex(&hub.pending);
        tokio::time::advance(Duration::from_secs(301)).await;
        assert!(!task.await.expect("ask task panicked"));

        let hub = PermHub::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
        hub.register_run("s1", tx);
        let mut q = ServeQuestionPrompter::new(Arc::clone(&hub));
        q.timeout = Some(Duration::from_secs(300));
        let s = scope("s1", false, Arc::clone(&hub));
        let task =
            tokio::spawn(async move { RUN.scope(s, async { q.ask(Vec::new()).await }).await });
        let _ = rx.recv().await.expect("no question ask");
        poison_mutex(&hub.pending_q);
        tokio::time::advance(Duration::from_secs(301)).await;
        assert_eq!(
            task.await.expect("ask task panicked").unwrap_err(),
            QuestionError::Timeout
        );
    }
}
