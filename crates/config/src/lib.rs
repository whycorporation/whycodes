//! WhyCodes configuration: load, merge, validate.
//!
//! Leaf types (`Message`, `Tool`, sandbox policy) live in `whycodes-core`.
//! This crate owns the user-facing `Config` tree and I/O.

pub mod dotted;
mod edit;
mod load;
mod merge;
mod stream_rules;
mod types;
mod validate;

// Re-export leaf sandbox types so callers that already import `whycodes_config`
// can resolve sandbox policy without a second crate path.
pub use whycodes_core::sandbox::{SandboxFallback, SandboxMode, SandboxSettings};

pub use types::{
    AutomationConfig, BashConfig, CONFIG_SCHEMA_VERSION, CommandConfig, Config,
    CustomCommandConfig, CustomToolConfig, DEFAULT_MODEL_ID, DEFAULT_PROVIDER, FormatConfig,
    GeneralConfig, HeadlessAskMode, HookConfig, HookEvent, LspConfig, LspServerConfig,
    MagicKeywordsConfig, McpServerConfig, McpTransportKind, MemoryConfig, NotifyConfig,
    NotifyEvent, PriceConfig, QuestionToolConfig, RuntimeLspServer, SecurityConfig, SessionConfig,
    SlopConfig, StreamRuleConfig, StreamRuleRepeat, StreamRuleScope, SwarmConfig,
    SystemPromptOverlays, ToolsConfig, TuiConfig, WebSearchConfig, WebSearchCustomConfig,
    is_discord_webhook_url,
};

pub use edit::ConfigTarget;
pub use stream_rules::{parse_rule_markdown, stream_rule_files};

#[cfg(test)]
pub(crate) use load::{
    encode_toml, ensure_parent_dir, map_toml_ser, parse_command_markdown,
    project_websearch_warning, toml_err, write_atomic,
};

/// `.whycodes/slop.toml` next to project config.
pub fn project_slop_path(project_dir: &std::path::Path) -> std::path::PathBuf {
    whycodes_core::project_dir(project_dir).join("slop.toml")
}

#[cfg(test)]
mod tests;
