//! JSON-RPC 2.0 framing for ACP: one JSON object per line on stdio.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

/// JSON-RPC error codes used here.
pub(crate) const PARSE_ERROR: i64 = -32700;
pub(crate) const INVALID_PARAMS: i64 = -32602;
pub(crate) const METHOD_NOT_FOUND: i64 = -32601;
pub(crate) const INTERNAL_ERROR: i64 = -32603;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Option<Value>>>>>;

/// Outgoing side: responses, notifications, and agent → client requests.
#[derive(Clone)]
pub(crate) struct Outbox {
    tx: mpsc::UnboundedSender<Value>,
    next_id: Arc<AtomicU64>,
    pending: Pending,
}

impl Outbox {
    /// An outbox and the receiver that [`write_loop`] drains.
    pub(crate) fn new() -> (Self, mpsc::UnboundedReceiver<Value>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let outbox = Self {
            tx,
            next_id: Arc::new(AtomicU64::new(0)),
            pending: Arc::new(Mutex::new(HashMap::new())),
        };
        (outbox, rx)
    }

    fn send(&self, msg: Value) {
        // The writer is gone only after stdout closed; nothing left to tell.
        if self.tx.send(msg).is_err() {
            tracing::debug!("acp: writer closed, message dropped");
        }
    }

    pub(crate) fn respond(&self, id: Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    pub(crate) fn respond_error(&self, id: Value, code: i64, message: &str) {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message},
        }));
    }

    pub(crate) fn notify(&self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// Send a request to the client; `None` when it answers with an error
    /// or the connection closes first.
    pub(crate) async fn request(&self, method: &str, params: Value) -> Option<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        rx.await.ok().flatten()
    }

    /// Route a client response to the waiting [`Self::request`].
    pub(crate) fn resolve(&self, msg: &Value) {
        let Some(id) = msg.get("id").and_then(Value::as_u64) else {
            return;
        };
        if let Some(tx) = lock(&self.pending).remove(&id) {
            // The requester may have given up (cancelled turn); fine.
            let _unused = tx.send(msg.get("result").cloned());
        }
    }

    /// Fail every outstanding request (connection closed).
    pub(crate) fn close_pending(&self) {
        lock(&self.pending).clear();
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Write each message as one line until every [`Outbox`] is dropped.
pub(crate) async fn write_loop<W: AsyncWrite + Unpin>(
    mut out: W,
    mut rx: mpsc::UnboundedReceiver<Value>,
) -> std::io::Result<()> {
    while let Some(msg) = rx.recv().await {
        let mut line = msg.to_string();
        line.push('\n');
        out.write_all(line.as_bytes()).await?;
        out.flush().await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
