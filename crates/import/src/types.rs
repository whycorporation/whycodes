//! Shared types for settings import.

use std::collections::HashMap;
use std::path::PathBuf;

use whycodes_config::{HookConfig, McpServerConfig};
use whycodes_core::types::PermissionAction;

/// A competing coding agent whose user-level settings we know how to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Product {
    Claude,
    OpenCode,
    Grok,
    Codex,
}

impl Product {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Some(Self::Claude),
            "opencode" => Some(Self::OpenCode),
            "grok" | "grok-build" => Some(Self::Grok),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::OpenCode => "opencode",
            Self::Grok => "grok",
            Self::Codex => "codex",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::OpenCode => "OpenCode",
            Self::Grok => "Grok Build",
            Self::Codex => "Codex CLI",
        }
    }
}

/// Consent state of one discovered file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceState {
    /// Present; never approved nor denied — contents not yet read.
    New,
    /// The user approved this exact path; it may be read and imported.
    Approved,
    /// The user declined this exact path; leave it alone.
    Denied,
    /// The path is a symlink — refused regardless of consent.
    Symlink,
}

/// One located settings file.
#[derive(Debug, Clone)]
pub struct FoundSource {
    pub product: Product,
    pub rel_path: &'static str,
    pub path: PathBuf,
    pub state: SourceState,
}

/// Parsed payload from one source file. No secrets; MCP env values are copied
/// as written (same as a user pasting config.toml).
#[derive(Debug, Clone)]
pub struct Extracted {
    pub product: Product,
    pub path: PathBuf,
    pub mcp: Vec<(String, McpServerConfig)>,
    pub permission: HashMap<String, PermissionAction>,
    pub hooks: Vec<HookConfig>,
    pub skipped: Vec<String>,
}

impl Default for Extracted {
    fn default() -> Self {
        Self {
            product: Product::Claude,
            path: PathBuf::new(),
            mcp: Vec::new(),
            permission: HashMap::new(),
            hooks: Vec::new(),
            skipped: Vec::new(),
        }
    }
}

impl Extracted {
    pub fn is_empty(&self) -> bool {
        self.mcp.is_empty() && self.permission.is_empty() && self.hooks.is_empty()
    }

    pub fn counts_label(&self) -> String {
        let mut parts = Vec::new();
        if !self.mcp.is_empty() {
            parts.push(format!("MCP×{}", self.mcp.len()));
        }
        if !self.permission.is_empty() {
            parts.push(format!("permission×{}", self.permission.len()));
        }
        if !self.hooks.is_empty() {
            parts.push(format!("hooks×{}", self.hooks.len()));
        }
        if parts.is_empty() {
            "nothing mapped".into()
        } else {
            parts.join("  ")
        }
    }
}

/// What would be written into WhyCodes config.
#[derive(Debug, Clone, Default)]
pub struct ImportPlan {
    pub mcp_add: Vec<(String, McpServerConfig)>,
    pub mcp_skip: Vec<(String, String)>,
    pub permission_add: Vec<(String, PermissionAction)>,
    pub permission_skip: Vec<(String, String)>,
    pub hooks_add: Vec<HookConfig>,
    pub hooks_skip: Vec<String>,
    pub warnings: Vec<String>,
}

impl ImportPlan {
    pub fn is_empty(&self) -> bool {
        self.mcp_add.is_empty() && self.permission_add.is_empty() && self.hooks_add.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "MCP +{} (skip {}) · permission +{} (skip {}) · hooks +{} (skip {})",
            self.mcp_add.len(),
            self.mcp_skip.len(),
            self.permission_add.len(),
            self.permission_skip.len(),
            self.hooks_add.len(),
            self.hooks_skip.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_parse_and_labels() {
        assert_eq!(Product::parse("claude-code"), Some(Product::Claude));
        assert_eq!(Product::parse("OpenCode"), Some(Product::OpenCode));
        assert_eq!(Product::parse("grok-build"), Some(Product::Grok));
        assert_eq!(Product::parse("codex"), Some(Product::Codex));
        assert_eq!(Product::parse("cursor"), None);
        assert_eq!(Product::Claude.as_str(), "claude");
        assert_eq!(Product::OpenCode.label(), "OpenCode");
        assert_eq!(Product::Grok.label(), "Grok Build");
        assert_eq!(Product::Codex.label(), "Codex CLI");
        assert_eq!(Product::Claude.label(), "Claude Code");
    }

    #[test]
    fn extracted_and_plan_summaries() {
        let empty = Extracted {
            product: Product::Claude,
            path: PathBuf::from("/x"),
            ..Default::default()
        };
        assert!(empty.is_empty());
        assert_eq!(empty.counts_label(), "nothing mapped");
        let mut full = empty.clone();
        full.mcp.push((
            "fs".into(),
            McpServerConfig {
                transport: None,
                command: Some("npx".into()),
                args: vec![],
                env: None,
                cwd: None,
                url: None,
                headers: None,
            },
        ));
        full.permission.insert("bash".into(), PermissionAction::Ask);
        full.hooks.push(HookConfig::default());
        assert!(!full.is_empty());
        assert!(full.counts_label().contains("MCP×1"));
        let plan = ImportPlan::default();
        assert!(plan.is_empty());
        assert!(plan.summary().contains("MCP +0"));
    }
}
