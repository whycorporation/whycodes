use async_trait::async_trait;
use std::path::Path;

use crate::file_claims::{ClaimResult, FileClaimRegistry, FileStaleEvent};
use crate::network::NetworkPolicy;
use crate::panel::PanelSink;
use crate::sandbox::SandboxSettings;
use crate::swarm_hub::SwarmHub;
use crate::types::{PermissionSet, ToolDefinition, ToolResult};

/// Context passed to tool execution
#[derive(Clone)]
pub struct ToolContext {
    pub working_dir: String,
    pub session_id: Option<String>,
    /// OS sandbox policy for shell tools (ignored by pure file tools).
    pub sandbox: SandboxSettings,
    /// Domain allow/deny for HTTP tools (`webfetch`, `websearch`, GitHub API).
    pub network: NetworkPolicy,
    /// When set (swarm workers), file mutators claim paths before writing.
    pub file_claims: Option<FileClaimRegistry>,
    /// Stable agent id for file claims (e.g. `worker-0`). Required with `file_claims`.
    pub agent_id: Option<String>,
    /// Display label for conflict messages (defaults to `agent_id`).
    pub agent_label: Option<String>,
    /// Resident workspace file index (when the host started one). File tools
    /// use it as a warm fast path for enumeration and fall back to walking.
    pub file_index: Option<std::sync::Arc<whycode_index::WorkspaceIndex>>,
    /// Optional sink so the `panel` tool can pin a file / diff / mermaid.
    pub panel: Option<PanelSink>,
    /// Swarm mailbox (DM / broadcast).
    pub swarm_hub: Option<SwarmHub>,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("working_dir", &self.working_dir)
            .field("session_id", &self.session_id)
            .field("sandbox", &self.sandbox)
            .field("network", &self.network)
            .field("file_claims", &self.file_claims)
            .field("agent_id", &self.agent_id)
            .field("agent_label", &self.agent_label)
            .field("file_index", &self.file_index.as_ref().map(|_| "Some"))
            .field("panel", &self.panel.as_ref().map(|_| "Some"))
            .field("swarm_hub", &self.swarm_hub)
            .finish()
    }
}

impl ToolContext {
    pub fn new(working_dir: impl Into<String>) -> Self {
        Self {
            working_dir: working_dir.into(),
            session_id: None,
            sandbox: SandboxSettings::default(),
            network: NetworkPolicy::unrestricted(),
            file_claims: None,
            agent_id: None,
            agent_label: None,
            file_index: None,
            panel: None,
            swarm_hub: None,
        }
    }

    pub fn unsandboxed(working_dir: impl Into<String>) -> Self {
        Self {
            working_dir: working_dir.into(),
            session_id: None,
            sandbox: SandboxSettings::off(),
            network: NetworkPolicy::unrestricted(),
            file_claims: None,
            agent_id: None,
            agent_label: None,
            file_index: None,
            panel: None,
            swarm_hub: None,
        }
    }

    /// Gate a file write/edit against the shared claim registry.
    ///
    /// No-op when claims are not active (normal single-agent turns).
    /// On conflict returns an error string suitable for `ToolResult.content`.
    pub fn check_file_write(&self, path: &Path) -> Result<(), String> {
        let Some(reg) = self.file_claims.as_ref() else {
            return Ok(());
        };
        let Some(id) = self.agent_id.as_deref() else {
            return Ok(());
        };
        let label = self.agent_label.as_deref().unwrap_or(id);
        match reg.try_claim(id, label, path) {
            ClaimResult::Acquired | ClaimResult::Held => Ok(()),
            ClaimResult::Conflict {
                owner_label,
                owner_id: _,
            } => {
                let shown = path.display();
                Err(format!(
                    "File conflict: `{shown}` is claimed by swarm agent `{owner_label}`. \
                     Choose a different file, or wait for that agent to finish."
                ))
            }
        }
    }

    /// After a successful read, note the path. Returns a stale-read event when
    /// another agent wrote the file since this reader last saw it.
    pub fn check_file_read(&self, path: &Path) -> Option<FileStaleEvent> {
        let reg = self.file_claims.as_ref()?;
        let id = self.agent_id.as_deref()?;
        reg.note_read(id, path)
    }
}

/// A tool that can be invoked by the LLM
#[async_trait]
pub trait Tool: Send + Sync {
    /// Name of the tool
    fn name(&self) -> &str;

    /// Description for the LLM
    fn description(&self) -> &str;

    /// JSON Schema for the tool parameters
    fn parameters(&self) -> serde_json::Value;

    /// Execute the tool
    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult;

    /// Get the full tool definition for LLM requests
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters(),
        }
    }

    /// Whether this tool is allowed given a permission set.
    ///
    /// Uses OpenCode-style resolution: `rules` (allow/ask/deny), then legacy
    /// allowed/denied lists and category flags. `Ask` still returns true so the
    /// tool appears in the schema; the agent loop prompts before execution.
    fn is_allowed(&self, permissions: &PermissionSet) -> bool {
        permissions.is_tool_allowed(self.name())
    }
}
