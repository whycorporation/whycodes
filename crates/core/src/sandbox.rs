//! Leaf sandbox policy types used by [`crate::ToolContext`] and the sandbox runtime.
//!
//! Kept in `whycode-core` (not `whycode-config`) so tool execution can depend on
//! policy without pulling config load/merge.

use serde::{Deserialize, Serialize};

/// How aggressively shell commands are OS-sandboxed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    /// Host `bash -c` with no namespace isolation.
    Off,
    /// Project directory RW, host root RO (bubblewrap on Linux).
    #[default]
    Workspace,
}

impl std::str::FromStr for SandboxMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "off" | "none" | "false" | "0" => Ok(Self::Off),
            "workspace" | "on" | "true" | "1" => Ok(Self::Workspace),
            other => Err(format!(
                "unknown sandbox mode '{other}' (expected off or workspace)"
            )),
        }
    }
}

impl SandboxMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Workspace => "workspace",
        }
    }
}

/// What to do when the requested sandbox backend is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxFallback {
    /// Log a warning and run the command on the host.
    #[default]
    Allow,
    /// Fail the tool call; do not run unsandboxed.
    Deny,
}

impl std::str::FromStr for SandboxFallback {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "allow" | "warn" | "host" => Ok(Self::Allow),
            "deny" | "error" | "strict" => Ok(Self::Deny),
            other => Err(format!(
                "unknown sandbox_fallback '{other}' (expected allow or deny)"
            )),
        }
    }
}

/// Resolved sandbox policy carried on [`crate::ToolContext`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxSettings {
    pub mode: SandboxMode,
    pub network: bool,
    pub fallback: SandboxFallback,
}

impl Default for SandboxSettings {
    fn default() -> Self {
        Self {
            mode: SandboxMode::Workspace,
            network: true,
            fallback: SandboxFallback::Allow,
        }
    }
}

impl SandboxSettings {
    pub fn off() -> Self {
        Self {
            mode: SandboxMode::Off,
            network: true,
            fallback: SandboxFallback::Allow,
        }
    }

    /// Build settings from raw security strings (config layer values).
    pub fn from_raw(mode: &str, network: bool, fallback: &str) -> Self {
        let mode = mode.parse().unwrap_or_else(|e| {
            tracing::warn!("{e}; falling back to workspace");
            SandboxMode::Workspace
        });
        let fallback = fallback.parse().unwrap_or_else(|e| {
            tracing::warn!("{e}; falling back to allow");
            SandboxFallback::Allow
        });
        Self {
            mode,
            network,
            fallback,
        }
    }
}
