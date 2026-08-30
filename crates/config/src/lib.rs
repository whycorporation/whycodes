//! WhyCodes configuration: load, merge, validate.
//!
//! Leaf types (`Message`, `Tool`, sandbox policy) live in `whycodes-core`.
//! This crate owns the user-facing `Config` tree and I/O.

mod load;
mod merge;
mod types;
mod validate;

// Re-export leaf sandbox types so callers that already import `whycodes_config`
// can resolve sandbox policy without a second crate path.
pub use whycodes_core::sandbox::{SandboxFallback, SandboxMode, SandboxSettings};

pub use types::{
    AutomationConfig, CONFIG_SCHEMA_VERSION, CommandConfig, Config, CustomCommandConfig,
    CustomToolConfig, GeneralConfig, HookConfig, HookEvent, MagicKeywordsConfig, McpServerConfig,
    McpTransportKind, MemoryConfig, NotifyConfig, NotifyEvent, QuestionToolConfig, SecurityConfig,
    SessionConfig, StreamRuleConfig, SwarmConfig, ToolsConfig, TuiConfig, is_discord_webhook_url,
};

#[cfg(test)]
pub(crate) use load::{
    encode_toml, ensure_parent_dir, map_toml_ser, parse_command_markdown, toml_err,
};

#[cfg(test)]
mod tests;
