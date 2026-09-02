//! Leaf sandbox policy types used by [`crate::ToolContext`] and the sandbox runtime.
//!
//! Kept in `whycodes-core` (not `whycodes-config`) so tool execution can depend on
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_fallback_parse_and_from_raw() {
        for (raw, mode) in [
            ("off", SandboxMode::Off),
            ("none", SandboxMode::Off),
            ("false", SandboxMode::Off),
            ("0", SandboxMode::Off),
            ("workspace", SandboxMode::Workspace),
            ("on", SandboxMode::Workspace),
            ("true", SandboxMode::Workspace),
            ("1", SandboxMode::Workspace),
        ] {
            assert_eq!(raw.parse::<SandboxMode>().unwrap(), mode);
        }
        assert!("nope".parse::<SandboxMode>().is_err());
        assert_eq!(SandboxMode::Off.as_str(), "off");
        assert_eq!(SandboxMode::Workspace.as_str(), "workspace");
        assert_eq!(SandboxMode::default(), SandboxMode::Workspace);

        for (raw, fb) in [
            ("allow", SandboxFallback::Allow),
            ("warn", SandboxFallback::Allow),
            ("host", SandboxFallback::Allow),
            ("deny", SandboxFallback::Deny),
            ("error", SandboxFallback::Deny),
            ("strict", SandboxFallback::Deny),
        ] {
            assert_eq!(raw.parse::<SandboxFallback>().unwrap(), fb);
        }
        assert!("nope".parse::<SandboxFallback>().is_err());

        let off = SandboxSettings::off();
        assert_eq!(off.mode, SandboxMode::Off);
        assert!(SandboxSettings::default().network);
        let parsed = SandboxSettings::from_raw("off", false, "deny");
        assert_eq!(parsed.mode, SandboxMode::Off);
        assert!(!parsed.network);
        assert_eq!(parsed.fallback, SandboxFallback::Deny);
        let fallback = SandboxSettings::from_raw("???", true, "???");
        assert_eq!(fallback.mode, SandboxMode::Workspace);
        assert_eq!(fallback.fallback, SandboxFallback::Allow);
    }
}
