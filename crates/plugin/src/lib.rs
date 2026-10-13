pub mod hooks;
pub mod loader;
pub mod notify;

pub use hooks::{
    HookContext, HookOutcome, HookRunResult, PreHookDecision, append_feedback, matching_hooks,
    run_hook, run_hooks, run_post_hooks, run_pre_hooks, tool_matches, truncate_output,
};
pub use loader::{
    LoadedPlugin, PluginManager, PluginManifest, PluginToolDef, ShellPluginSpec,
    global_plugins_dir, project_plugins_dir,
};
pub use notify::{NotifyPayload, send_notify, spawn_notify};
