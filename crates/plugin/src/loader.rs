use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// On-disk `plugin.json` / `manifest.json`.
///
/// Extra fields are ignored so a future marketplace schema can grow without
/// breaking discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Plugin-level shell command. Used when `tools` is empty, or as the
    /// fallback for a tool that omits its own `command`.
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub tools: Vec<PluginToolDef>,
    /// Reserved. Config `[hooks]` stay the hook path; marketplace hooks are out.
    #[serde(default)]
    pub hooks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToolDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
}

/// One discovered plugin directory.
#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    pub dir: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: PluginManifest,
}

/// A shell command the agent can register as `plugin_<name>`.
#[derive(Debug, Clone)]
pub struct ShellPluginSpec {
    pub name: String,
    pub command: String,
    pub description: String,
    pub parameters: Option<serde_json::Value>,
    /// Absolute plugin directory; the child process starts here.
    pub working_dir: PathBuf,
    /// Path shown by `whycode plugins` (the manifest file).
    pub origin: String,
}

/// Plugin loader — discovers `plugin.json` / `manifest.json` trees.
pub struct PluginManager {
    plugins: Vec<LoadedPlugin>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Register a plugin from its manifest (no directory — command must be absolute / on PATH).
    pub fn register(&mut self, manifest: PluginManifest) {
        self.upsert(LoadedPlugin {
            dir: PathBuf::new(),
            manifest_path: PathBuf::new(),
            manifest,
        });
    }

    pub fn list(&self) -> &[LoadedPlugin] {
        &self.plugins
    }

    pub fn find(&self, name: &str) -> Option<&LoadedPlugin> {
        self.plugins.iter().find(|p| p.manifest.name == name)
    }

    /// Discover `plugin.json` / `manifest.json` under a directory (one level).
    ///
    /// Same plugin `name` replaces an earlier entry (later directory wins).
    pub fn discover_dir(&mut self, dir: &Path) -> usize {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut n = 0;
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Some(loaded) = load_plugin_dir(&path) {
                self.upsert(loaded);
                n += 1;
            }
        }
        n
    }

    /// Global `$CONFIG/plugins/` then `<project>/.whycode/plugins/`.
    /// Project entries override global ones with the same `name`.
    pub fn discover_standard(&mut self, project_dir: Option<&Path>) -> usize {
        let mut n = 0;
        if let Some(global) = global_plugins_dir() {
            n += self.discover_dir(&global);
        }
        if let Some(project) = project_dir {
            n += self.discover_dir(&project_plugins_dir(project));
        }
        n
    }

    /// Expand every loaded plugin into shell tool specs (skips those with no command).
    pub fn shell_specs(&self) -> Vec<ShellPluginSpec> {
        self.plugins
            .iter()
            .flat_map(LoadedPlugin::shell_specs)
            .collect()
    }

    fn upsert(&mut self, loaded: LoadedPlugin) {
        if let Some(existing) = self
            .plugins
            .iter_mut()
            .find(|p| p.manifest.name == loaded.manifest.name)
        {
            *existing = loaded;
        } else {
            self.plugins.push(loaded);
        }
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadedPlugin {
    pub fn shell_specs(&self) -> Vec<ShellPluginSpec> {
        let fallback = self
            .manifest
            .command
            .clone()
            .or_else(|| inferred_command(&self.dir));
        let origin = if self.manifest_path.as_os_str().is_empty() {
            self.manifest.name.clone()
        } else {
            self.manifest_path.display().to_string()
        };
        let working_dir = if self.dir.as_os_str().is_empty() {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        } else {
            self.dir.clone()
        };

        if self.manifest.tools.is_empty() {
            let Some(cmd) = fallback else {
                return Vec::new();
            };
            return vec![ShellPluginSpec {
                name: self.manifest.name.clone(),
                command: resolve_command(&self.dir, &cmd),
                description: nonempty(&self.manifest.description, &self.manifest.name),
                parameters: None,
                working_dir,
                origin,
            }];
        }

        let mut out = Vec::new();
        for tool in &self.manifest.tools {
            let Some(cmd) = tool.command.clone().or_else(|| fallback.clone()) else {
                tracing::debug!(
                    plugin = %self.manifest.name,
                    tool = %tool.name,
                    "plugin tool skipped: no command"
                );
                continue;
            };
            let name = if self.manifest.tools.len() == 1 {
                if tool.name.trim().is_empty() {
                    self.manifest.name.clone()
                } else {
                    tool.name.clone()
                }
            } else {
                format!("{}_{}", self.manifest.name, tool.name)
            };
            let description = nonempty(
                &tool.description,
                &nonempty(&self.manifest.description, &name),
            );
            out.push(ShellPluginSpec {
                name,
                command: resolve_command(&self.dir, &cmd),
                description,
                parameters: tool.parameters.clone(),
                working_dir: working_dir.clone(),
                origin: origin.clone(),
            });
        }
        out
    }
}

/// `$CONFIG_DIR/plugins` (same tree as `plugins.toml`).
pub fn global_plugins_dir() -> Option<PathBuf> {
    whycode_config::Config::default_path()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("plugins")))
}

/// `<project>/.whycode/plugins`.
pub fn project_plugins_dir(project: &Path) -> PathBuf {
    project.join(".whycode").join("plugins")
}

fn load_plugin_dir(dir: &Path) -> Option<LoadedPlugin> {
    for name in ["plugin.json", "manifest.json"] {
        let mf = dir.join(name);
        if !mf.is_file() {
            continue;
        }
        let text = match std::fs::read_to_string(&mf) {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(path = %mf.display(), error = %e, "plugin manifest unreadable");
                continue;
            }
        };
        match serde_json::from_str::<PluginManifest>(&text) {
            Ok(manifest) if !manifest.name.trim().is_empty() => {
                return Some(LoadedPlugin {
                    dir: dir.to_path_buf(),
                    manifest_path: mf,
                    manifest,
                });
            }
            Ok(_) => {
                tracing::debug!(path = %mf.display(), "plugin manifest missing name");
            }
            Err(e) => {
                tracing::debug!(path = %mf.display(), error = %e, "plugin manifest invalid");
            }
        }
    }
    None
}

fn inferred_command(dir: &Path) -> Option<String> {
    if dir.as_os_str().is_empty() {
        return None;
    }
    let names = if cfg!(windows) {
        &[
            "run.cmd",
            "run.ps1",
            "run.bat",
            "plugin.cmd",
            "run.sh",
            "run",
            "plugin.sh",
        ][..]
    } else {
        &["run", "run.sh", "plugin.sh"][..]
    };
    for name in names {
        let p = dir.join(name);
        if p.is_file() {
            return Some(name.to_string());
        }
    }
    None
}

fn resolve_command(dir: &Path, command: &str) -> String {
    let cmd = command.trim();
    if cmd.is_empty() || dir.as_os_str().is_empty() {
        return cmd.to_string();
    }
    let p = Path::new(cmd);
    if p.is_absolute() {
        return cmd.to_string();
    }
    // Path-like (./run.sh, bin/tool) — resolve against the plugin dir when present.
    if cmd.contains('/') || cmd.contains('\\') || cmd.starts_with('.') {
        let candidate = dir.join(cmd);
        if candidate.exists() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    let candidate = dir.join(cmd);
    if candidate.is_file() {
        return candidate.to_string_lossy().into_owned();
    }
    cmd.to_string()
}

fn nonempty(value: &str, fallback: &str) -> String {
    let t = value.trim();
    if t.is_empty() {
        fallback.to_string()
    } else {
        t.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_plugin(root: &Path, dir_name: &str, body: &str) -> PathBuf {
        let dir = root.join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.json"), body).unwrap();
        dir
    }

    #[test]
    fn discover_dir_loads_plugin_json() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(
            tmp.path(),
            "hello",
            r#"{"name":"hello","command":"echo hi","description":"d"}"#,
        );
        let mut mgr = PluginManager::new();
        assert_eq!(mgr.discover_dir(tmp.path()), 1);
        assert_eq!(mgr.list()[0].manifest.name, "hello");
        let specs = mgr.shell_specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "hello");
        assert_eq!(specs[0].command, "echo hi");
    }

    #[test]
    fn later_dir_overrides_same_name() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        write_plugin(&a, "p", r#"{"name":"dup","command":"echo a"}"#);
        write_plugin(&b, "p", r#"{"name":"dup","command":"echo b"}"#);
        let mut mgr = PluginManager::new();
        mgr.discover_dir(&a);
        mgr.discover_dir(&b);
        assert_eq!(mgr.list().len(), 1);
        assert_eq!(mgr.shell_specs()[0].command, "echo b");
    }

    #[test]
    fn relative_command_resolves_against_plugin_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = write_plugin(
            tmp.path(),
            "rel",
            r#"{"name":"rel","command":"./greet.sh"}"#,
        );
        std::fs::write(dir.join("greet.sh"), "#!/bin/sh\necho ok\n").unwrap();
        let mut mgr = PluginManager::new();
        mgr.discover_dir(tmp.path());
        let spec = &mgr.shell_specs()[0];
        assert!(
            spec.command.ends_with("greet.sh"),
            "resolved: {}",
            spec.command
        );
        assert_eq!(spec.working_dir, dir);
    }

    #[test]
    fn infers_run_sh_when_command_omitted() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = write_plugin(tmp.path(), "imp", r#"{"name":"imp","description":"x"}"#);
        std::fs::write(dir.join("run.sh"), "#!/bin/sh\necho implied\n").unwrap();
        let mut mgr = PluginManager::new();
        mgr.discover_dir(tmp.path());
        let specs = mgr.shell_specs();
        assert_eq!(specs.len(), 1);
        assert!(specs[0].command.ends_with("run.sh"));
    }

    #[test]
    fn multi_tool_names_are_prefixed() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(
            tmp.path(),
            "pack",
            &json!({
                "name": "pack",
                "command": "echo default",
                "tools": [
                    {"name": "one", "description": "first"},
                    {"name": "two", "command": "echo two"}
                ]
            })
            .to_string(),
        );
        let mut mgr = PluginManager::new();
        mgr.discover_dir(tmp.path());
        let mut names: Vec<_> = mgr.shell_specs().into_iter().map(|s| s.name).collect();
        names.sort();
        assert_eq!(names, vec!["pack_one", "pack_two"]);
    }

    #[test]
    fn defaults_allow_minimal_manifest() {
        let m: PluginManifest = serde_json::from_str(r#"{"name":"n"}"#).unwrap();
        assert!(m.version.is_empty());
        assert!(m.tools.is_empty());
        assert!(m.command.is_none());
    }

    #[test]
    fn nameless_manifest_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(tmp.path(), "bad", r#"{"name":"","command":"echo x"}"#);
        let mut mgr = PluginManager::new();
        assert_eq!(mgr.discover_dir(tmp.path()), 0);
    }

    #[test]
    fn project_plugins_dir_is_under_whycode() {
        assert_eq!(
            project_plugins_dir(Path::new("/repo")),
            PathBuf::from("/repo/.whycode/plugins")
        );
    }
}
