// ── widgets/mod.rs: Widget module root ─────────────────────────────────
// Reusable rendering widgets.

pub mod diff;
pub mod message;
pub mod tool_call;
pub mod wrap;

#[cfg(test)]
mod tests {
    #[test]
    fn diff_parse_is_reachable_from_widgets() {
        assert!(super::diff::parse_unified_diff("").is_empty());
        let lines = super::diff::parse_unified_diff("+added\n-removed");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].kind, super::diff::DiffLineKind::Add);
        assert_eq!(lines[1].kind, super::diff::DiffLineKind::Remove);
    }
}
