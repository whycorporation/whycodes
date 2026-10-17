//! Which stream rules fired in this session, for `repeat` limits.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// User-turn counter plus the turn each stream rule last fired on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamRuleLedger {
    /// User turns started in this session (1-based once a turn runs).
    #[serde(default)]
    pub turn: u64,
    /// Rule name → `turn` of its last hit.
    #[serde(default)]
    pub fired: BTreeMap<String, u64>,
}

impl StreamRuleLedger {
    /// Nothing fired yet (the turn counter alone is not worth persisting).
    pub fn is_empty(&self) -> bool {
        self.fired.is_empty()
    }

    /// A new user turn starts.
    pub fn begin_turn(&mut self) {
        self.turn = self.turn.saturating_add(1);
    }

    /// Turn on which `name` last fired.
    pub fn last_fired(&self, name: &str) -> Option<u64> {
        self.fired.get(name).copied()
    }

    /// Record a hit of `name` on the current turn.
    pub fn record(&mut self, name: &str) {
        self.fired.insert(name.to_string(), self.turn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_tracks_turns_and_hits() {
        let mut l = StreamRuleLedger::default();
        assert!(l.is_empty());
        l.begin_turn();
        l.begin_turn();
        assert_eq!(l.last_fired("a"), None);
        l.record("a");
        assert!(!l.is_empty());
        assert_eq!(l.last_fired("a"), Some(2));
        let back: StreamRuleLedger = serde_json::from_str("{}").unwrap();
        assert_eq!(back, StreamRuleLedger::default());
    }
}
