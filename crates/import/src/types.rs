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

/// Kind of a user-selectable import row (MCP / permission / hook).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportItemKind {
    Mcp,
    Permission,
    Hook,
}

/// One addable row shown in the TUI picker or CLI item prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportItem {
    pub kind: ImportItemKind,
    pub label: String,
    pub detail: String,
}

impl ImportPlan {
    pub fn is_empty(&self) -> bool {
        self.mcp_add.is_empty() && self.permission_add.is_empty() && self.hooks_add.is_empty()
    }

    /// Rows the user can check/uncheck, in apply order (MCP, permission, hooks).
    pub fn selectable_items(&self) -> Vec<ImportItem> {
        let mut items = Vec::with_capacity(
            self.mcp_add.len() + self.permission_add.len() + self.hooks_add.len(),
        );
        for (name, server) in &self.mcp_add {
            let detail = server
                .command
                .clone()
                .or_else(|| server.url.clone())
                .unwrap_or_default();
            items.push(ImportItem {
                kind: ImportItemKind::Mcp,
                label: format!("MCP `{name}`"),
                detail,
            });
        }
        for (tool, action) in &self.permission_add {
            let action = match action {
                PermissionAction::Allow => "allow",
                PermissionAction::Ask => "ask",
                PermissionAction::Deny => "deny",
            };
            items.push(ImportItem {
                kind: ImportItemKind::Permission,
                label: format!("permission `{tool}` = {action}"),
                detail: String::new(),
            });
        }
        for hook in &self.hooks_add {
            let event = match hook.event {
                whycodes_config::HookEvent::PreTool => "pre_tool",
                whycodes_config::HookEvent::PostTool => "post_tool",
            };
            items.push(ImportItem {
                kind: ImportItemKind::Hook,
                label: format!("hook {event} `{}`", hook.command),
                detail: hook.tool_match.clone(),
            });
        }
        items
    }

    /// Keep add-lists in lockstep with [`selectable_items`] booleans.
    /// A short `selected` slice drops the remaining rows.
    pub fn retain_selected(&mut self, selected: &[bool]) {
        let mut i = 0;
        let mut next = || {
            let keep = selected.get(i).copied().unwrap_or(false);
            i += 1;
            keep
        };
        self.mcp_add.retain(|_| next());
        self.permission_add.retain(|_| next());
        self.hooks_add.retain(|_| next());
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
        assert_eq!(Product::OpenCode.as_str(), "opencode");
        assert_eq!(Product::Grok.as_str(), "grok");
        assert_eq!(Product::Codex.as_str(), "codex");
        assert_eq!(Product::parse("claude"), Some(Product::Claude));
        assert_eq!(Product::parse("grok"), Some(Product::Grok));
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
        assert!(plan.selectable_items().is_empty());
    }

    #[test]
    fn counts_label_permission_or_hooks_only() {
        let mut only_perm = Extracted::default();
        only_perm
            .permission
            .insert("bash".into(), PermissionAction::Ask);
        assert_eq!(only_perm.counts_label(), "permission×1");
        let mut only_hooks = Extracted::default();
        only_hooks.hooks.push(HookConfig::default());
        assert_eq!(only_hooks.counts_label(), "hooks×1");
    }

    fn sample_plan() -> ImportPlan {
        let mut plan = ImportPlan::default();
        plan.mcp_add.push((
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
        plan.mcp_add.push((
            "remote".into(),
            McpServerConfig {
                transport: None,
                command: None,
                args: vec![],
                env: None,
                cwd: None,
                url: Some("https://example".into()),
                headers: None,
            },
        ));
        plan.permission_add
            .push(("bash".into(), PermissionAction::Ask));
        plan.permission_add
            .push(("read".into(), PermissionAction::Allow));
        plan.hooks_add.push(HookConfig {
            event: whycodes_config::HookEvent::PreTool,
            tool_match: "bash".into(),
            command: "echo hi".into(),
            block_on_failure: true,
            timeout_secs: 30,
        });
        plan
    }

    #[test]
    fn selectable_items_cover_mcp_permission_hook() {
        let plan = sample_plan();
        let items = plan.selectable_items();
        assert_eq!(items.len(), 5);
        assert_eq!(items[0].kind, ImportItemKind::Mcp);
        assert_eq!(items[0].label, "MCP `fs`");
        assert_eq!(items[0].detail, "npx");
        assert_eq!(items[1].kind, ImportItemKind::Mcp);
        assert_eq!(items[1].detail, "https://example");
        assert_eq!(items[2].kind, ImportItemKind::Permission);
        assert!(items[2].label.contains("ask"));
        assert_eq!(items[3].kind, ImportItemKind::Permission);
        assert!(items[3].label.contains("allow"));
        assert_eq!(items[4].kind, ImportItemKind::Hook);
        assert!(items[4].label.contains("pre_tool"));
        assert_eq!(items[4].detail, "bash");
    }

    #[test]
    fn retain_selected_keeps_checked_rows_in_order() {
        let mut plan = sample_plan();
        plan.retain_selected(&[true, false, false, true, true]);
        assert_eq!(plan.mcp_add.len(), 1);
        assert_eq!(plan.mcp_add[0].0, "fs");
        assert_eq!(plan.permission_add.len(), 1);
        assert_eq!(plan.permission_add[0].0, "read");
        assert_eq!(plan.hooks_add.len(), 1);
        let mut empty = sample_plan();
        empty.retain_selected(&[]);
        assert!(empty.is_empty());
        let mut deny = sample_plan();
        deny.permission_add[0].1 = PermissionAction::Deny;
        assert!(
            deny.selectable_items()[2]
                .label
                .contains("permission `bash` = deny")
        );
        let mut post = sample_plan();
        post.hooks_add[0].event = whycodes_config::HookEvent::PostTool;
        assert!(post.selectable_items()[4].label.contains("post_tool"));
    }

    #[test]
    fn selectable_item_falls_back_to_empty_detail() {
        let mut plan = ImportPlan::default();
        plan.mcp_add.push((
            "bare".into(),
            McpServerConfig {
                transport: None,
                command: None,
                args: vec![],
                env: None,
                cwd: None,
                url: None,
                headers: None,
            },
        ));
        let items = plan.selectable_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].detail, "");
        assert_eq!(items[0].kind, ImportItemKind::Mcp);
    }
}
