pub mod hooks;
pub mod loader;

pub use hooks::{
    HookContext, HookRunResult, PreHookDecision, matching_hooks, run_hook, run_post_hooks,
    run_pre_hooks, tool_matches, truncate_output,
};
pub use loader::{
    LoadedPlugin, PluginManager, PluginManifest, PluginToolDef, ShellPluginSpec,
    global_plugins_dir, project_plugins_dir,
};
