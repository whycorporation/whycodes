//! Session-scoped permission asks for protocol v1.
//!
//! The daemon uses one [`ServePrompter`] for the shared [`Agent`]. Each `/v1`
//! run enters [`RUN`] so `ask` knows which session and whether to auto-approve.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::oneshot;
use whycode_agent::events::{EventSink, TurnEvent};
use whycode_agent::permission::PermissionPrompter;
use whycode_agent::question::{AutoAnswerPrompter, QuestionError, QuestionPrompter};
use whycode_protocol::sdk::{PermissionDecision, QuestionAnswerWire};
use whycode_tools::question::{QuestionAnswer, QuestionSpec};

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

pub struct ServePrompter {
    pub hub: Arc<PermHub>,
}

#[async_trait]
impl PermissionPrompter for ServePrompter {
    async fn ask(&self, tool_name: &str, detail: &str) -> bool {
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
    }
}

pub struct ServeQuestionPrompter {
    pub hub: Arc<PermHub>,
}

#[async_trait]
impl QuestionPrompter for ServeQuestionPrompter {
    async fn ask(
        &self,
        questions: Vec<QuestionSpec>,
    ) -> Result<Vec<QuestionAnswer>, QuestionError> {
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
    }
}
