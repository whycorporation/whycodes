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
        Self {
            plugins: Vec::new(),
        }
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

    /// Discover `plugin.json` / `manifest.json` under a directory (one level).
    pub fn discover_dir(&mut self, dir: &std::path::Path) -> usize {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut n = 0;
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            for name in ["plugin.json", "manifest.json"] {
                let mf = path.join(name);
                if !mf.is_file() {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(&mf)
                    && let Ok(manifest) = serde_json::from_str::<PluginManifest>(&text)
                {
                    self.register(manifest);
                    n += 1;
                    break;
                }
            }
        }
        n
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}
