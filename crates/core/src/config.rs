use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::types::{AgentInfo, ModelConfig, ProviderConfig};

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// API provider configurations
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,

    /// Model configurations
    #[serde(default)]
    pub models: HashMap<String, ModelConfig>,

    /// Agent definitions
    #[serde(default)]
    pub agents: Vec<AgentInfo>,

    /// Default agent name
    #[serde(default = "default_agent")]
    pub default_agent: String,

    /// Default model
    #[serde(default)]
    pub default_model: Option<ModelConfig>,

    /// Tools configuration
    #[serde(default)]
    pub tools: ToolsConfig,

    /// Session configuration
    #[serde(default)]
    pub session: SessionConfig,

    /// TUI configuration
    #[serde(default)]
    pub tui: TuiConfig,

    /// General settings
    #[serde(default)]
    pub general: GeneralConfig,
}

fn default_agent() -> String {
    "build".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsConfig {
    #[serde(default = "default_true")]
    pub enable_read: bool,
    #[serde(default = "default_true")]
    pub enable_write: bool,
    #[serde(default = "default_true")]
    pub enable_edit: bool,
    #[serde(default = "default_true")]
    pub enable_glob: bool,
    #[serde(default = "default_true")]
    pub enable_grep: bool,
    #[serde(default = "default_true")]
    pub enable_shell: bool,
    #[serde(default = "default_true")]
    pub enable_webfetch: bool,
    #[serde(default = "default_true")]
    pub enable_websearch: bool,
    pub disabled_tools: Vec<String>,
    pub custom_tools: HashMap<String, CustomToolConfig>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionConfig {
    #[serde(default = "default_max_tokens")]
    pub max_context_tokens: usize,
    #[serde(default = "default_compaction_threshold")]
    pub compaction_threshold: usize,
    pub store_path: Option<PathBuf>,
}

fn default_max_tokens() -> usize {
    200_000
}

fn default_compaction_threshold() -> usize {
    150_000
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TuiConfig {
    pub theme: Option<String>,
    pub key_bindings: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GeneralConfig {
    pub project_path: Option<PathBuf>,
    pub log_level: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CustomToolConfig {
    pub command: String,
    pub description: String,
    pub parameters: Option<serde_json::Value>,
}

impl Config {
    /// Load config from the default location
    pub fn load() -> crate::Result<Self> {
        let path = Self::default_path()?;
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            Ok(toml::from_str(&content).map_err(|e| crate::Error::Config(e.to_string()))?)
        } else {
            Ok(Self::default())
        }
    }

    /// Save config to the default location
    pub fn save(&self) -> crate::Result<()> {
        let path = Self::default_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content =
            toml::to_string_pretty(self).map_err(|e| crate::Error::Config(e.to_string()))?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Get default config path
    pub fn default_path() -> crate::Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("com", "whycorporation", "whycode")
            .ok_or_else(|| crate::Error::Config("Cannot find config directory".to_string()))?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Get data directory for sessions, caches, etc.
    pub fn data_dir() -> crate::Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("com", "whycorporation", "whycode")
            .ok_or_else(|| crate::Error::Config("Cannot find data directory".to_string()))?;
        Ok(dirs.data_local_dir().to_path_buf())
    }

    /// Get the configured project path or current directory
    pub fn project_path(&self) -> PathBuf {
        self.general
            .project_path
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// Get provider by name
    pub fn get_provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.get(name)
    }

    /// Get model config
    pub fn get_model(&self, provider: &str, model: &str) -> Option<&ModelConfig> {
        self.models
            .values()
            .find(|m| m.provider_id == provider && m.model_id == model)
    }

    /// Get agent by name
    pub fn get_agent(&self, name: &str) -> Option<&AgentInfo> {
        self.agents.iter().find(|a| a.name == name)
    }

    /// Get the default agent
    pub fn default_agent(&self) -> Option<&AgentInfo> {
        self.get_agent(&self.default_agent)
    }
}
