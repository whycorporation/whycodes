// Re-export the Tool trait and ToolContext from whycodes-core,
// where they live to avoid circular dependencies with other crates.
pub use whycodes_core::{Tool, ToolContext};

#[cfg(test)]
mod tests {
    #[test]
    fn tool_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
