// ── ui/mod.rs: UI module root ──────────────────────────────────────────
// Declares sub-modules and re-exports the render entry point.

pub mod chat;
pub mod dialogs;
pub mod file_suggest;
pub mod header;
pub mod layout;
pub mod markdown;
pub mod progress_bar;
pub mod prompt;
pub mod render;
pub mod scrollbar;
pub mod sidebar;
pub mod slash_suggest;
pub mod status;
pub mod status_bar;
pub mod subagents;
pub mod timefmt;
pub mod toast;
pub mod todos;

pub use render::render;

#[cfg(test)]
mod tests {
    #[test]
    fn mod_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
