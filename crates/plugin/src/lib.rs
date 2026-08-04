pub mod hooks;
pub mod loader;

pub use hooks::{
    HookContext, HookRunResult, PreHookDecision, matching_hooks, run_post_hooks, run_pre_hooks,
    tool_matches, truncate_output,
};
pub use loader::{PluginManager, PluginManifest, PluginToolDef};
