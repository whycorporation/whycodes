// ── widgets/mod.rs: Widget module root ─────────────────────────────────
// Reusable rendering widgets.

pub mod diff;
pub mod message;
pub mod tool_call;
pub mod wrap;

#[cfg(test)]
mod tests {
    #[test]
    fn mod_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
