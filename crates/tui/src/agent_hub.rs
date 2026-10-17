//! Agent hub (`Alt+A` / `/agents`): the session's `task` and `swarm` workers.
//!
//! One row per worker: id prefix, type, status, elapsed time, tokens when the
//! provider reported them, and the last activity line. Running workers come
//! first, then finished ones, newest first. Keys on the selected row:
//!
//! - `Enter` opens the read-only transcript (live log while running, the
//!   saved `.whycodes/agents/<id>.md` once finished).
//! - `t` prefills `/tell <id> ` in the prompt.
//! - `k` asks a running worker to stop after its current step.
//! - `r` queues "Re-run worker <id>: <goal>" as the next user turn, so the
//!   parent starts a new `task` call; the UI never calls tools itself.
//!
//! Attached to `whycodes serve`, `k` and `r` are hidden: the daemon API has
//! no per-worker cancel, and the hub only sees the workers it was told about.

use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{ChatRole, DialogKind, SubagentUi, TuiApp};

/// Characters of the worker id shown in a row (`/tell` matches a prefix).
const ID_PREFIX_CHARS: usize = 14;

/// Rows in display order: running workers oldest first, then finished ones
/// newest first.
pub fn rows(app: &TuiApp) -> Vec<&SubagentUi> {
    let mut out: Vec<&SubagentUi> = app.subagents.iter().filter(|s| s.is_running()).collect();
    out.extend(app.subagents.iter().rev().filter(|s| !s.is_running()));
    out
}

/// Id of the row under the cursor.
pub fn selected_id(app: &TuiApp) -> Option<String> {
    rows(app).get(app.agents_cursor).map(|row| row.id.clone())
}

/// `running` / `done` / `failed` / `cancelled`.
pub fn status_label(status: &str) -> &'static str {
    match status {
        "running" => "running",
        "completed" | "done" => "done",
        "cancelled" | "killed" => "cancelled",
        _ => "failed",
    }
}

/// `950 tok`, `12.3k tok`.
pub fn format_tokens(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k tok", n as f64 / 1000.0)
    } else {
        format!("{n} tok")
    }
}

fn elapsed_ms(row: &SubagentUi) -> u64 {
    if row.is_running() {
        row.started_at.elapsed().as_millis() as u64
    } else {
        row.elapsed_ms
    }
}

/// The running step's label, or the first line of a finished worker's output.
fn last_activity(row: &SubagentUi) -> String {
    if !row.activity.is_empty() {
        return row.activity.clone();
    }
    let first = row.output.lines().map(str::trim).find(|l| !l.is_empty());
    first.unwrap_or("").chars().take(80).collect()
}

/// Label and detail text for one row.
pub fn row_text(row: &SubagentUi) -> (String, String) {
    let id: String = row.id.chars().take(ID_PREFIX_CHARS).collect();
    let label = format!("{id} · {} · {}", row.kind, status_label(&row.status));
    let mut detail = format!("{:.1}s", elapsed_ms(row) as f64 / 1000.0);
    if row.tokens > 0 {
        detail.push_str(&format!(" · {}", format_tokens(row.tokens)));
    }
    let activity = last_activity(row);
    if !activity.is_empty() {
        detail.push_str(&format!(" · {activity}"));
    }
    (label, detail)
}

/// Dialog title listing the keys that apply.
pub fn title(app: &TuiApp) -> &'static str {
    if app.attached {
        " Agents  ·  Enter view  ·  t tell "
    } else {
        " Agents  ·  Enter view  ·  t tell  ·  k cancel  ·  r re-run "
    }
}

/// Open the hub with the first row selected.
pub fn open(app: &mut TuiApp) {
    app.agents_cursor = 0;
    crate::input::open_dialog(app, DialogKind::Agents);
    app.mark_dirty();
}

/// The user turn a re-run sends.
pub fn rerun_prompt(id: &str, kind: &str, goal: &str) -> String {
    format!("Re-run worker {id}: {goal}\n\nStart it as a new `task` call (subagent_type `{kind}`).")
}

/// `t` / `k` / `r` on the selected row. `false` leaves the key to the
/// dialog keymap (arrows, Enter, Esc).
pub fn handle_key(app: &mut TuiApp, key: &KeyEvent) -> bool {
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return false;
    }
    let Some(row) = rows(app).get(app.agents_cursor).copied() else {
        return false;
    };
    let (id, running) = (row.id.clone(), row.is_running());
    let rerun = rerun_prompt(&row.id, &row.kind, &row.description);
    match key.code {
        KeyCode::Char('t') => {
            crate::input::dismiss_dialog(app);
            if running {
                app.input_buffer = format!("/tell {id} ");
                app.input_cursor = app.input_buffer.len();
                app.focus_prompt();
            } else {
                notice(app, "That worker finished — /tell reaches running workers");
            }
        }
        KeyCode::Char('k') if !app.attached => {
            if running {
                app.pending_worker_cancel = Some(id.clone());
                app.status_message = format!("Cancelling {id} after its current step…");
            } else {
                notice(app, "That worker already finished");
            }
        }
        KeyCode::Char('r') if !app.attached => {
            if running {
                notice(app, "Still running — cancel it (k) before a re-run");
            } else {
                app.pending_reruns.push_back(rerun);
                crate::input::dismiss_dialog(app);
                notice(app, "Re-run queued for the next turn");
            }
        }
        _ => return false,
    }
    app.mark_dirty();
    true
}

fn notice(app: &mut TuiApp, text: &str) {
    app.toasts
        .push(crate::toast::ToastKind::Info, text.to_string());
}

/// Send the hub's cancel request to the running turn's registry.
pub fn send_pending_cancel(
    app: &mut TuiApp,
    cancels: &whycodes_agent::worker_cancel::WorkerCancels,
) {
    let Some(id) = app.pending_worker_cancel.take() else {
        return;
    };
    if !cancels.cancel(&id) {
        notice(app, "That worker already finished");
    }
    app.mark_dirty();
}

/// Start a queued re-run as a user turn once nothing else runs or waits.
pub fn send_pending_rerun(app: &mut TuiApp, agent_busy: bool) {
    if agent_busy || app.has_pending_turn() {
        return;
    }
    if let Some(text) = app.pending_reruns.pop_front() {
        app.add_message(ChatRole::User, text.clone());
        app.enqueue_prompt_text(text);
    }
}

/// A finished worker's saved output, `.whycodes/agents/<id>.md` (the id is
/// cleaned the way the agent cleans it when saving).
pub fn read_artifact(project: &Path, id: &str) -> Option<String> {
    let clean: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if clean.is_empty() {
        return None;
    }
    let path = whycodes_core::project_dir(project)
        .join("agents")
        .join(format!("{clean}.md"));
    std::fs::read_to_string(path).ok()
}

#[cfg(test)]
#[path = "agent_hub_tests.rs"]
mod tests;
