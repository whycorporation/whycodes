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

/// Permission-layer filesystem ladder (issue #122). Independent of OS
/// [`SandboxMode`]: bubblewrap still uses Off/Workspace; this decides when
/// writes and deletes need an extra ask (or a headless deny).
///
/// Default [`WorkspaceWrite`] matches today's workspace-on behaviour so
/// existing configs do not tighten overnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesystemMode {
    /// Today's "sandbox off" for files: no extra write/delete gate.
    FullAccess,
    /// Writes confined to the project dir (current workspace sandbox).
    #[default]
    WorkspaceWrite,
    /// Workspace writes allowed; unlink / rmdir / `git clean -fd` of
    /// tracked or project files require `ask` (TUI) or deny (headless).
    DeleteGuard,
    /// No writes, no deletes (`plan` / `ask` / `verifier` posture).
    ReadOnly,
}

impl std::str::FromStr for FilesystemMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "full_access" | "full-access" | "fullaccess" | "full" => Ok(Self::FullAccess),
            "workspace_write" | "workspace-write" | "workspacewrite" | "workspace" => {
                Ok(Self::WorkspaceWrite)
            }
            "delete_guard" | "delete-guard" | "deleteguard" => Ok(Self::DeleteGuard),
            "read_only" | "read-only" | "readonly" | "ro" => Ok(Self::ReadOnly),
            other => Err(format!(
                "unknown filesystem mode '{other}' (expected full_access, workspace_write, delete_guard, or read_only)"
            )),
        }
    }
}

impl FilesystemMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FullAccess => "full_access",
            Self::WorkspaceWrite => "workspace_write",
            Self::DeleteGuard => "delete_guard",
            Self::ReadOnly => "read_only",
        }
    }

    /// `delete_guard` and `read_only` both intercept unlink / rmdir / `git clean -f`.
    pub fn guards_deletes(self) -> bool {
        matches!(self, Self::DeleteGuard | Self::ReadOnly)
    }

    /// `read_only` refuses file writes (`write` / `edit` / `apply_patch`).
    pub fn allows_writes(self) -> bool {
        !matches!(self, Self::ReadOnly)
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
    pub filesystem: FilesystemMode,
    pub network: bool,
    pub fallback: SandboxFallback,
}

impl Default for SandboxSettings {
    fn default() -> Self {
        Self {
            mode: SandboxMode::Workspace,
            filesystem: FilesystemMode::WorkspaceWrite,
            network: true,
            fallback: SandboxFallback::Allow,
        }
    }
}

impl SandboxSettings {
    pub fn off() -> Self {
        Self {
            mode: SandboxMode::Off,
            filesystem: FilesystemMode::FullAccess,
            network: true,
            fallback: SandboxFallback::Allow,
        }
    }

    /// Build settings from raw security strings (config layer values).
    pub fn from_raw(mode: &str, network: bool, fallback: &str) -> Self {
        Self::from_raw_fs(mode, network, fallback, "workspace_write")
    }

    /// [`from_raw`] plus an explicit filesystem ladder string.
    pub fn from_raw_fs(mode: &str, network: bool, fallback: &str, filesystem: &str) -> Self {
        let mode = mode.parse().unwrap_or_else(|e| {
            tracing::warn!("{e}; falling back to workspace");
            SandboxMode::Workspace
        });
        let fallback = fallback.parse().unwrap_or_else(|e| {
            tracing::warn!("{e}; falling back to allow");
            SandboxFallback::Allow
        });
        let filesystem = filesystem.parse().unwrap_or_else(|e| {
            tracing::warn!("{e}; falling back to workspace_write");
            FilesystemMode::WorkspaceWrite
        });
        Self {
            mode,
            filesystem,
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
        assert_eq!(off.filesystem, FilesystemMode::FullAccess);
        assert!(SandboxSettings::default().network);
        assert_eq!(
            SandboxSettings::default().filesystem,
            FilesystemMode::WorkspaceWrite
        );
        let parsed = SandboxSettings::from_raw("off", false, "deny");
        assert_eq!(parsed.mode, SandboxMode::Off);
        assert!(!parsed.network);
        assert_eq!(parsed.fallback, SandboxFallback::Deny);
        assert_eq!(parsed.filesystem, FilesystemMode::WorkspaceWrite);
        let fallback = SandboxSettings::from_raw("???", true, "???");
        assert_eq!(fallback.mode, SandboxMode::Workspace);
        assert_eq!(fallback.fallback, SandboxFallback::Allow);
        let fs = SandboxSettings::from_raw_fs("workspace", true, "allow", "delete_guard");
        assert_eq!(fs.filesystem, FilesystemMode::DeleteGuard);
        let fs_bad = SandboxSettings::from_raw_fs("workspace", true, "allow", "???");
        assert_eq!(fs_bad.filesystem, FilesystemMode::WorkspaceWrite);
    }

    #[test]
    fn filesystem_mode_parses() {
        for (raw, mode) in [
            ("full_access", FilesystemMode::FullAccess),
            ("full-access", FilesystemMode::FullAccess),
            ("full", FilesystemMode::FullAccess),
            ("workspace_write", FilesystemMode::WorkspaceWrite),
            ("workspace", FilesystemMode::WorkspaceWrite),
            ("delete_guard", FilesystemMode::DeleteGuard),
            ("delete-guard", FilesystemMode::DeleteGuard),
            ("read_only", FilesystemMode::ReadOnly),
            ("read-only", FilesystemMode::ReadOnly),
            ("ro", FilesystemMode::ReadOnly),
        ] {
            assert_eq!(raw.parse::<FilesystemMode>().unwrap(), mode);
        }
        assert!("nope".parse::<FilesystemMode>().is_err());
        assert_eq!(FilesystemMode::FullAccess.as_str(), "full_access");
        assert_eq!(FilesystemMode::WorkspaceWrite.as_str(), "workspace_write");
        assert_eq!(FilesystemMode::DeleteGuard.as_str(), "delete_guard");
        assert_eq!(FilesystemMode::ReadOnly.as_str(), "read_only");
        assert_eq!(FilesystemMode::default(), FilesystemMode::WorkspaceWrite);
        assert!(FilesystemMode::DeleteGuard.guards_deletes());
        assert!(FilesystemMode::ReadOnly.guards_deletes());
        assert!(!FilesystemMode::WorkspaceWrite.guards_deletes());
        assert!(!FilesystemMode::FullAccess.guards_deletes());
        assert!(FilesystemMode::WorkspaceWrite.allows_writes());
        assert!(FilesystemMode::DeleteGuard.allows_writes());
        assert!(FilesystemMode::FullAccess.allows_writes());
        assert!(!FilesystemMode::ReadOnly.allows_writes());
    }
}
