use serde::{Deserialize, Serialize};

/// Manifest for a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub tools: Vec<PluginToolDef>,
    pub hooks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Plugin loader — loads and manages plugins
pub struct PluginManager {
    plugins: Vec<PluginManifest>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self { plugins: Vec::new() }
    }

    /// Register a plugin from its manifest
    pub fn register(&mut self, manifest: PluginManifest) {
        self.plugins.push(manifest);
    }

    /// List all registered plugins
    pub fn list(&self) -> &[PluginManifest] {
        &self.plugins
    }

    /// Find a plugin by name
    pub fn find(&self, name: &str) -> Option<&PluginManifest> {
        self.plugins.iter().find(|p| p.name == name)
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}
