//! Transient notices that do not overwrite the status line.
//!
//! The status line holds standing context — the agent, the model, the current
//! keybindings. Writing "Copied" into it destroys that until something else
//! rewrites it, so every transient message costs the user the information that
//! was there. Toasts expire on their own and leave the status line alone.

use std::time::{Duration, Instant};

/// How long a toast stays up. Long enough to read a short line; short enough
/// that a burst of them does not stack into a wall.
pub const DEFAULT_TTL: Duration = Duration::from_secs(4);

/// Mode / intent mismatches need a longer glance (user may switch agent).
pub const WARNING_TTL: Duration = Duration::from_secs(8);

/// Errors stay up longer — they are the ones worth not missing.
pub const ERROR_TTL: Duration = Duration::from_secs(8);

/// Most toasts on screen at once. Beyond this the oldest is dropped rather
/// than the newest refused: the newest is the one the user just caused.
pub const MAX_VISIBLE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

impl ToastKind {
    /// Leading glyph, so kind is readable without relying on colour alone.
    /// Matches `system_callout` glyphs in `ui/chat.rs` for a single visual language.
    pub fn glyph(&self) -> &'static str {
        match self {
            Self::Info => "i",
            Self::Success => "✓",
            Self::Warning => "!",
            Self::Error => "✕",
        }
    }

    fn ttl(&self) -> Duration {
        match self {
            Self::Error => ERROR_TTL,
            Self::Warning => WARNING_TTL,
            _ => DEFAULT_TTL,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub kind: ToastKind,
    expires_at: Instant,
}

impl Toast {
    pub fn new(kind: ToastKind, message: impl Into<String>) -> Self {
        Self::at(Instant::now(), kind, message)
    }

    /// [`Toast::new`] with the creation time supplied, so expiry can be tested
    /// without sleeping.
    pub fn at(now: Instant, kind: ToastKind, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind,
            expires_at: now + kind.ttl(),
        }
    }

    pub fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at
    }
}

/// The toasts currently on screen.
#[derive(Debug, Clone, Default)]
pub struct Toasts {
    items: Vec<Toast>,
}

impl Toasts {
    pub fn push(&mut self, kind: ToastKind, message: impl Into<String>) {
        self.push_toast(Toast::new(kind, message));
    }

    pub fn push_toast(&mut self, toast: Toast) {
        self.items.push(toast);
        while self.items.len() > MAX_VISIBLE {
            self.items.remove(0);
        }
    }

    /// Drop the ones whose time is up. Called once per frame.
    ///
    /// Returns `true` when at least one toast was removed (caller should
    /// request a redraw so the stack does not linger a frame past expiry).
    pub fn prune(&mut self, now: Instant) -> bool {
        let before = self.items.len();
        self.items.retain(|t| !t.is_expired(now));
        self.items.len() != before
    }

    pub fn visible(&self) -> &[Toast] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_toast_expires_after_its_ttl() {
        let now = Instant::now();
        let toast = Toast::at(now, ToastKind::Info, "hello");
        assert!(!toast.is_expired(now));
        assert!(!toast.is_expired(now + DEFAULT_TTL - Duration::from_millis(1)));
        assert!(toast.is_expired(now + DEFAULT_TTL));
    }

    #[test]
    fn errors_stay_up_longer_than_everything_else() {
        let now = Instant::now();
        let info = Toast::at(now, ToastKind::Info, "x");
        let error = Toast::at(now, ToastKind::Error, "x");
        assert!(info.is_expired(now + DEFAULT_TTL));
        assert!(!error.is_expired(now + DEFAULT_TTL));
        assert!(error.is_expired(now + ERROR_TTL));
    }

    #[test]
    fn warnings_outlast_info() {
        let now = Instant::now();
        let info = Toast::at(now, ToastKind::Info, "x");
        let warn = Toast::at(now, ToastKind::Warning, "x");
        assert!(info.is_expired(now + DEFAULT_TTL));
        assert!(!warn.is_expired(now + DEFAULT_TTL));
        assert!(warn.is_expired(now + WARNING_TTL));
    }

    #[test]
    fn pruning_removes_only_the_expired_ones() {
        let now = Instant::now();
        let mut toasts = Toasts::default();
        toasts.push_toast(Toast::at(now, ToastKind::Info, "old"));
        toasts.push_toast(Toast::at(now + DEFAULT_TTL, ToastKind::Info, "new"));

        toasts.prune(now + DEFAULT_TTL);
        assert_eq!(toasts.visible().len(), 1);
        assert_eq!(toasts.visible()[0].message, "new");
    }

    #[test]
    fn a_burst_drops_the_oldest_not_the_newest() {
        let mut toasts = Toasts::default();
        for i in 0..MAX_VISIBLE + 2 {
            toasts.push(ToastKind::Info, format!("m{i}"));
        }
        assert_eq!(toasts.visible().len(), MAX_VISIBLE);
        // The two oldest are gone; the most recent is kept, because it is the
        // one the user just caused.
        assert_eq!(toasts.visible()[0].message, "m2");
        assert_eq!(
            toasts.visible().last().unwrap().message,
            format!("m{}", MAX_VISIBLE + 1)
        );
    }

    #[test]
    fn every_kind_has_a_distinct_glyph() {
        let glyphs: Vec<&str> = [
            ToastKind::Info,
            ToastKind::Success,
            ToastKind::Warning,
            ToastKind::Error,
        ]
        .iter()
        .map(|k| k.glyph())
        .collect();
        let mut unique = glyphs.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            glyphs.len(),
            "kinds must be distinguishable without colour"
        );
    }

    #[test]
    fn an_empty_stack_reports_itself_empty() {
        let mut toasts = Toasts::default();
        assert!(toasts.is_empty());
        toasts.push(ToastKind::Info, "x");
        assert!(!toasts.is_empty());
    }
}
