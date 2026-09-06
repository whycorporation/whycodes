// ── widgets/mod.rs: Widget module root ─────────────────────────────────
// Reusable rendering widgets.

pub mod diff;
pub mod message;
pub mod tool_call;
pub mod wrap;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
