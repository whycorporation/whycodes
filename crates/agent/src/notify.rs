//! Session notifications: Discord / Telegram, fire-and-forget from the agent loop.

use std::sync::Arc;

use whycodes_config::{NotifyConfig, NotifyEvent};
use whycodes_plugin::notify::{NotifyPayload, spawn_notify};

/// Shared notify config (cloned onto permission/question waiters).
pub type NotifyHandle = Arc<NotifyConfig>;

pub fn handle_from_config(cfg: &NotifyConfig) -> NotifyHandle {
    Arc::new(cfg.clone())
}

pub fn spawn_turn_done(cfg: &NotifyConfig, title: &str, body: &str, session_id: Option<&str>) {
    if !cfg.enabled_for(NotifyEvent::TurnDone) {
        return;
    }
    spawn_notify(
        cfg.clone(),
        NotifyPayload::turn_done(title, body, session_id.map(str::to_string)),
    );
}

pub fn spawn_need_input(cfg: &NotifyConfig, title: &str, body: &str, session_id: Option<&str>) {
    if !cfg.enabled_for(NotifyEvent::NeedInput) {
        return;
    }
    spawn_notify(
        cfg.clone(),
        NotifyPayload::need_input(title, body, session_id.map(str::to_string)),
    );
}

/// Permission / question waiters that actually block a human.
pub fn spawn_need_input_wait(cfg: &NotifyConfig, kind: &str, detail: &str) {
    let body = if detail.trim().is_empty() {
        kind.to_string()
    } else {
        let d = detail.trim();
        let short: String = d.chars().take(280).collect();
        if d.chars().count() > 280 {
            format!("{kind}\n{short}…")
        } else {
            format!("{kind}\n{short}")
        }
    };
    spawn_need_input(cfg, "Waiting for you", &body, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_is_noop_when_off() {
        let cfg = NotifyConfig::default();
        spawn_turn_done(&cfg, "t", "b", Some("abc"));
        spawn_need_input_wait(&cfg, "permission", "bash rm");
        let mut on = cfg;
        on.on = vec!["need_input".into()];
        spawn_need_input_wait(&on, "question", "");
        spawn_need_input_wait(&on, "permission", &"x".repeat(400));
    }
}
