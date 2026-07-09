// Re-export the Tool trait and ToolContext from whycode-core,
// where they live to avoid circular dependencies with other crates.
pub use whycode_core::{Tool, ToolContext};
