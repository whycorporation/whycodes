use crate::plugin::PluginConfig;
use crate::skill::Skill;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// A registry that holds all loaded skills.
#[derive(Debug, Default)]
pub struct SkillRegistry {
    pub skills: Vec<Skill>,
}

impl SkillRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { skills: Vec::new() }
    }

    /// Load skills from the project `.skills/` directory and the global
    /// user config directory, globbing for `*.skill.md` files.
    pub fn load() -> anyhow::Result<Self> {
        let mut registry = Self::new();

        // 1. Project-local skills directory:  .skills/
        let project_dir = Path::new(".skills");
        if project_dir.is_dir() {
            registry.load_from_dir(project_dir)?;
        }

        // 2. Global user config directory
        if let Ok(global_dir) = global_skills_dir()
            && global_dir.is_dir() {
                registry.load_from_dir(&global_dir)?;
            }

        Ok(registry)
    }

    /// Load all `*.skill.md` files from a directory.
    pub fn load_from_dir(&mut self, dir: &Path) -> anyhow::Result<()> {
        let pattern = dir.join("*.skill.md");
        let pattern_str = pattern.to_string_lossy();

        let entries = glob::glob(&pattern_str).map_err(|e| {
            anyhow::anyhow!("Invalid glob pattern '{}': {}", pattern_str, e)
        })?;

        for entry in entries {
            match entry {
                Ok(path) => {
                    if path.is_file() {
                        debug!("Loading skill from {:?}", path);
                        match Skill::from_file(&path) {
                            Ok(skill) => {
                                debug!("Loaded skill '{}'", skill.name);
                                self.skills.push(skill);
                            }
                            Err(e) => {
                                warn!("Failed to load skill {:?}: {}", path, e);
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Glob error: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Find a skill by name.
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }
}

/// A registry that holds all configured plugins.
#[derive(Debug, Default)]
pub struct PluginRegistry {
    pub plugins: Vec<PluginConfig>,
}

impl PluginRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Load plugins from the global config directory's `plugins.toml` file.
    ///
    /// Expected format:
    /// ```toml
    /// [[plugins]]
    /// name = "my-tool"
    /// command = "my-tool --arg"
    /// description = "Does something useful"
    /// parameters = { type = "object", properties = {} }
    /// ```
    pub fn load_from_config() -> anyhow::Result<Self> {
        let path = global_plugins_path()?;

        if !path.exists() {
            debug!("No plugins config found at {:?}, returning empty registry", path);
            return Ok(Self::new());
        }

        let content = std::fs::read_to_string(&path)?;
        Self::parse_toml(&content)
    }

    /// Parse plugins from a TOML string.
    pub fn parse_toml(content: &str) -> anyhow::Result<Self> {
        #[derive(Deserialize)]
        struct PluginsFile {
            plugins: Option<Vec<PluginConfig>>,
        }

        let file: PluginsFile = toml::from_str(content)
            .map_err(|e| anyhow::anyhow!("Failed to parse plugins TOML: {}", e))?;

        Ok(Self {
            plugins: file.plugins.unwrap_or_default(),
        })
    }

    /// Find a plugin by name.
    pub fn get(&self, name: &str) -> Option<&PluginConfig> {
        self.plugins.iter().find(|p| p.name == name)
    }
}

/// Return the global user config directory for whycode.
fn whycode_config_dir() -> anyhow::Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("com", "whycorporation", "whycode")
        .ok_or_else(|| anyhow::anyhow!("Cannot determine config directory"))?;
    Ok(dirs.config_dir().to_path_buf())
}

/// Return the global skills directory:  `$CONFIG_DIR/skills/`
fn global_skills_dir() -> anyhow::Result<PathBuf> {
    Ok(whycode_config_dir()?.join("skills"))
}

/// Return the global plugins config file:  `$CONFIG_DIR/plugins.toml`
fn global_plugins_path() -> anyhow::Result<PathBuf> {
    Ok(whycode_config_dir()?.join("plugins.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skill_registry_loads_from_dir() {
        let dir = tempfile::tempdir().unwrap();

        let content = "---\nname: test-skill\ndescription: a test\n---\n\n# Prompt\n";
        std::fs::write(dir.path().join("test.skill.md"), content).unwrap();

        let mut reg = SkillRegistry::new();
        reg.load_from_dir(dir.path()).unwrap();
        assert_eq!(reg.skills.len(), 1);
        assert_eq!(reg.skills[0].name, "test-skill");
    }

    #[test]
    fn plugin_registry_parses_toml() {
        let toml_content = r#"
[[plugins]]
name = "hello"
command = "echo hello"
description = "Says hello"
parameters = { type = "object", properties = { name = { type = "string" } } }
"#;
        let reg = PluginRegistry::parse_toml(toml_content).unwrap();
        assert_eq!(reg.plugins.len(), 1);
        assert_eq!(reg.plugins[0].name, "hello");
        assert_eq!(reg.plugins[0].command, "echo hello");
        assert!(reg.plugins[0].parameters.is_some());
    }

    #[test]
    fn plugin_registry_empty_toml() {
        let reg = PluginRegistry::parse_toml("").unwrap();
        assert!(reg.plugins.is_empty());
    }
}
