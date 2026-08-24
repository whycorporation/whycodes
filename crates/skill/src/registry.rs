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

    /// Load skills from the process cwd (`.skills/`) and the global user
    /// config directory. Prefer [`Self::load_for_project`] when a project
    /// root is known — this path exists for callers without a working dir.
    pub fn load() -> anyhow::Result<Self> {
        Self::load_for_project(Path::new("."))
    }

    /// Load all `*.skill.md` files from a directory.
    pub fn load_from_dir(&mut self, dir: &Path) -> anyhow::Result<()> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) => {
                debug!(path = %dir.display(), %error, "skill dir unreadable");
                return Ok(());
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with(".skill.md") || !path.is_file() {
                continue;
            }
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

        Ok(())
    }

    /// Find a skill by name.
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// Case-insensitive name lookup (`skill://Rust-Dev` → `rust-dev`).
    pub fn get_ignore_ascii_case(&self, name: &str) -> Option<&Skill> {
        self.skills
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Project-local skills only (no user-global dir). Safe for tests and
    /// for the compact system-prompt catalog.
    pub fn load_project(project: &Path) -> anyhow::Result<Self> {
        let mut registry = Self::new();
        for dir in [
            project.join(".skills"),
            project.join("skills"),
            project.join(".whycode").join("skills"),
        ] {
            if dir.is_dir() {
                registry.load_from_dir(&dir)?;
                load_skill_md_tree(&mut registry, &dir);
            }
        }
        load_skill_md_tree(&mut registry, &project.join(".claude").join("skills"));
        Ok(registry)
    }

    /// Project skills plus the user-global `$CONFIG/skills` tree.
    pub fn load_for_project(project: &Path) -> anyhow::Result<Self> {
        let mut registry = Self::load_project(project)?;
        let global = global_skills_dir();
        if global.is_dir() {
            registry.load_from_dir(&global)?;
            load_skill_md_tree(&mut registry, &global);
        }
        Ok(registry)
    }

    /// Compact catalog for the system prompt: names and descriptions only.
    pub fn catalog_markdown(&self) -> String {
        if self.skills.is_empty() {
            return String::new();
        }
        let mut out = String::from(
            "# Skills\n\n\
             Available skills (name + description). Load the body with \
             `read skill://<name>` or the `skill` tool (`action=load`). \
             Do not guess a skill's contents.\n\n",
        );
        for skill in &self.skills {
            let desc = if skill.description.is_empty() {
                "(no description)"
            } else {
                skill.description.as_str()
            };
            out.push_str("- `");
            out.push_str(&skill.name);
            out.push_str("`: ");
            out.push_str(desc);
            out.push('\n');
        }
        out
    }
}

/// Load Claude-style `<dir>/<name>/SKILL.md` trees. Duplicate names are skipped.
fn load_skill_md_tree(registry: &mut SkillRegistry, root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        match Skill::from_file(&skill_md) {
            Ok(skill) => {
                if registry.get(&skill.name).is_none() {
                    registry.skills.push(skill);
                }
            }
            Err(e) => {
                warn!("Failed to load skill {:?}: {}", skill_md, e);
            }
        }
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
        let path = global_plugins_path();

        if !path.exists() {
            debug!(
                "No plugins config found at {:?}, returning empty registry",
                path
            );
            return Ok(Self::new());
        }

        let content = std::fs::read_to_string(&path)?;
        Self::parse_toml(&content)
    }

    /// Merge project-level `.whycode/plugins.toml` (later entries override by name).
    pub fn load_layered(project_dir: &std::path::Path) -> anyhow::Result<Self> {
        let mut reg = Self::load_from_config().unwrap_or_default();
        let path = project_dir.join(".whycode").join("plugins.toml");
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let project = Self::parse_toml(&content)?;
            for p in project.plugins {
                if let Some(existing) = reg.plugins.iter_mut().find(|x| x.name == p.name) {
                    *existing = p;
                } else {
                    reg.plugins.push(p);
                }
            }
        }
        Ok(reg)
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
fn whycode_config_dir() -> PathBuf {
    whycode_core::paths::config_dir()
}

/// Return the global skills directory:  `$CONFIG_DIR/skills/`
fn global_skills_dir() -> PathBuf {
    whycode_config_dir().join("skills")
}

/// Return the global plugins config file:  `$CONFIG_DIR/plugins.toml`
fn global_plugins_path() -> PathBuf {
    whycode_config_dir().join("plugins.toml")
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
    fn registries_default() {
        assert!(SkillRegistry::default().skills.is_empty());
        assert!(PluginRegistry::default().plugins.is_empty());
        let _ = SkillRegistry::new();
        let _ = PluginRegistry::new();
    }

    #[test]
    fn plugin_registry_empty_toml() {
        let reg = PluginRegistry::parse_toml("").unwrap();
        assert!(reg.plugins.is_empty());
    }

    #[test]
    fn plugin_registry_missing_plugins_table() {
        let reg = PluginRegistry::parse_toml("name = \"x\"\n").unwrap();
        assert!(reg.plugins.is_empty());
    }

    #[test]
    fn plugin_registry_rejects_bad_toml() {
        assert!(PluginRegistry::parse_toml("[[plugins]]\nname = [").is_err());
    }

    #[test]
    fn plugin_registry_get_finds_and_misses() {
        let reg = PluginRegistry::parse_toml(
            r#"
[[plugins]]
name = "hello"
command = "echo"
description = "d"
"#,
        )
        .unwrap();
        assert!(reg.get("hello").is_some());
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn skill_registry_get_and_skips_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ok.skill.md"), "---\nname: ok\n---\nbody\n").unwrap();
        std::fs::write(dir.path().join("bad.skill.md"), "no frontmatter at all\n").unwrap();
        std::fs::write(dir.path().join("readme.md"), "ignored\n").unwrap();
        std::fs::create_dir(dir.path().join("nested.skill.md")).unwrap();

        let mut reg = SkillRegistry::new();
        reg.load_from_dir(dir.path()).unwrap();
        assert_eq!(reg.skills.len(), 1);
        assert!(reg.get("ok").is_some());
        assert!(reg.get("nope").is_none());
    }

    #[test]
    fn load_from_missing_dir_is_ok() {
        let mut reg = SkillRegistry::new();
        let missing = tempfile::tempdir().unwrap().path().join("does-not-exist");
        reg.load_from_dir(&missing).unwrap();
        assert!(reg.skills.is_empty());
    }

    #[test]
    fn skill_registry_load_uses_project_and_global() {
        let _guard = env_lock();
        let root = tempfile::tempdir().unwrap();
        let project_skills = root.path().join(".skills");
        std::fs::create_dir_all(&project_skills).unwrap();
        std::fs::write(
            project_skills.join("local.skill.md"),
            "---\nname: local\n---\nL\n",
        )
        .unwrap();

        let home = root.path().join("home");
        let global = home.join("skills");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(
            global.join("global.skill.md"),
            "---\nname: global\n---\nG\n",
        )
        .unwrap();

        let prev_home = std::env::var_os("WHYCODE_HOME");
        let prev_cwd = std::env::current_dir().unwrap();
        unsafe { std::env::set_var("WHYCODE_HOME", &home) };
        std::env::set_current_dir(root.path()).unwrap();
        let loaded = SkillRegistry::load();
        std::env::set_current_dir(prev_cwd).unwrap();
        restore_home(prev_home);
        let reg = loaded.unwrap();
        assert!(reg.get("local").is_some());
        assert!(reg.get("global").is_some());
    }

    #[test]
    fn skill_registry_load_empty_when_dirs_missing() {
        let _guard = env_lock();
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("empty-home");
        std::fs::create_dir_all(&home).unwrap();
        let prev_home = std::env::var_os("WHYCODE_HOME");
        let prev_cwd = std::env::current_dir().unwrap();
        unsafe { std::env::set_var("WHYCODE_HOME", &home) };
        std::env::set_current_dir(root.path()).unwrap();
        let loaded = SkillRegistry::load();
        std::env::set_current_dir(prev_cwd).unwrap();
        restore_home(prev_home);
        assert!(loaded.unwrap().skills.is_empty());
    }

    #[test]
    fn catalog_and_case_insensitive_lookup() {
        let mut reg = SkillRegistry::new();
        assert!(reg.catalog_markdown().is_empty());
        assert!(reg.get_ignore_ascii_case("x").is_none());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("demo.skill.md"),
            "---\nname: Demo\ndescription: does a thing\n---\n\nbody here\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("bare.skill.md"),
            "---\nname: bare\n---\n\njust a body\n",
        )
        .unwrap();
        reg.load_from_dir(dir.path()).unwrap();
        assert!(reg.get_ignore_ascii_case("demo").is_some());
        let catalog = reg.catalog_markdown();
        assert!(catalog.contains("`Demo`"));
        assert!(catalog.contains("does a thing"));
        assert!(catalog.contains("`bare`"));
        assert!(catalog.contains("(no description)"));
        assert!(catalog.contains("skill://"));
    }

    #[test]
    fn load_project_reads_skill_md_and_dot_skills() {
        let root = tempfile::tempdir().unwrap();
        let skills = root.path().join(".skills");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(
            skills.join("local.skill.md"),
            "---\nname: local\ndescription: from dot-skills\n---\n\nL\n",
        )
        .unwrap();
        let alt = root.path().join("skills");
        std::fs::create_dir_all(&alt).unwrap();
        std::fs::write(
            alt.join("extra.skill.md"),
            "---\nname: extra\ndescription: from skills/\n---\n\nE\n",
        )
        .unwrap();
        let nested = root.path().join(".whycode").join("skills").join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            nested.join("SKILL.md"),
            "---\nname: nested\ndescription: skill md\n---\n\nN\n",
        )
        .unwrap();
        let claude = root.path().join(".claude").join("skills").join("fmt");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join("SKILL.md"),
            "---\nname: fmt\ndescription: formatter\n---\n\nF\n",
        )
        .unwrap();
        // Duplicate name in SKILL.md tree is skipped.
        let dup = root.path().join(".whycode").join("skills").join("dup");
        std::fs::create_dir_all(&dup).unwrap();
        std::fs::write(
            dup.join("SKILL.md"),
            "---\nname: local\ndescription: shadowed\n---\n\nX\n",
        )
        .unwrap();
        // Broken SKILL.md is ignored.
        let bad = root.path().join(".whycode").join("skills").join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("SKILL.md"), "not a skill\n").unwrap();

        let loaded = SkillRegistry::load_project(root.path()).unwrap();
        assert!(loaded.get("local").is_some());
        assert!(loaded.get("extra").is_some());
        assert!(loaded.get("nested").is_some());
        assert!(loaded.get("fmt").is_some());
        assert_eq!(loaded.get("local").unwrap().description, "from dot-skills");
    }

    #[test]
    fn load_for_project_includes_global_skills() {
        let _guard = env_lock();
        let home = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("WHYCODE_HOME");
        unsafe { std::env::set_var("WHYCODE_HOME", home.path()) };
        let global = home.path().join("skills");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(
            global.join("g.skill.md"),
            "---\nname: global-skill\ndescription: g\n---\n\nG\n",
        )
        .unwrap();
        let project = tempfile::tempdir().unwrap();
        let loaded = SkillRegistry::load_for_project(project.path()).unwrap();
        restore_home(prev);
        assert!(loaded.get("global-skill").is_some());
    }

    #[test]
    fn plugin_load_from_config_missing_and_present() {
        let _guard = env_lock();
        let home = tempfile::tempdir().unwrap();
        // Ensure the restore-Some branch runs even when the process had no home set.
        unsafe { std::env::set_var("WHYCODE_HOME", "/tmp/whycode-cov-sentinel") };
        let prev_home = std::env::var_os("WHYCODE_HOME");
        unsafe { std::env::set_var("WHYCODE_HOME", home.path()) };

        let empty = PluginRegistry::load_from_config().unwrap();
        assert!(empty.plugins.is_empty());

        std::fs::write(
            home.path().join("plugins.toml"),
            r#"
[[plugins]]
name = "from-home"
command = "true"
description = "d"
"#,
        )
        .unwrap();
        let present = PluginRegistry::load_from_config().unwrap();

        restore_home(prev_home);
        assert_eq!(present.plugins.len(), 1);
        assert_eq!(present.plugins[0].name, "from-home");
    }

    #[test]
    fn plugin_load_from_config_unreadable_path_is_err() {
        let _guard = env_lock();
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("plugins.toml")).unwrap();
        let prev_home = std::env::var_os("WHYCODE_HOME");
        unsafe { std::env::set_var("WHYCODE_HOME", home.path()) };
        let err = PluginRegistry::load_from_config();
        restore_home(prev_home);
        assert!(err.is_err());
    }

    #[test]
    fn plugin_load_from_config_bad_file_is_err() {
        let _guard = env_lock();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("plugins.toml"), "[[plugins]]\nname = [").unwrap();
        let prev_home = std::env::var_os("WHYCODE_HOME");
        unsafe { std::env::set_var("WHYCODE_HOME", home.path()) };
        let err = PluginRegistry::load_from_config();
        restore_home(prev_home);
        assert!(err.is_err());
    }

    #[test]
    fn load_layered_overrides_by_name_and_appends() {
        let _guard = env_lock();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("plugins.toml"),
            r#"
[[plugins]]
name = "shared"
command = "old"
description = "global"
[[plugins]]
name = "only-global"
command = "g"
description = "g"
"#,
        )
        .unwrap();
        let prev_home = std::env::var_os("WHYCODE_HOME");
        unsafe { std::env::set_var("WHYCODE_HOME", home.path()) };

        let project = tempfile::tempdir().unwrap();
        let why = project.path().join(".whycode");
        std::fs::create_dir_all(&why).unwrap();
        std::fs::write(
            why.join("plugins.toml"),
            r#"
[[plugins]]
name = "shared"
command = "new"
description = "project"
[[plugins]]
name = "only-project"
command = "p"
description = "p"
"#,
        )
        .unwrap();
        let layered = PluginRegistry::load_layered(project.path());

        let no_project = tempfile::tempdir().unwrap();
        let unchanged = PluginRegistry::load_layered(no_project.path());

        restore_home(prev_home);

        let layered = layered.unwrap();
        assert_eq!(layered.get("shared").unwrap().command, "new");
        assert!(layered.get("only-global").is_some());
        assert!(layered.get("only-project").is_some());
        assert_eq!(unchanged.unwrap().plugins.len(), 2);
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn restore_home(prev: Option<std::ffi::OsString>) {
        match prev {
            Some(v) => unsafe { std::env::set_var("WHYCODE_HOME", v) },
            None => unsafe { std::env::remove_var("WHYCODE_HOME") },
        }
    }

    #[test]
    fn load_layered_uses_default_when_global_config_is_invalid() {
        let _guard = env_lock();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("plugins.toml"), "[[plugins]]\nname = [").unwrap();
        let prev_home = std::env::var_os("WHYCODE_HOME");
        unsafe { std::env::set_var("WHYCODE_HOME", home.path()) };
        let project = tempfile::tempdir().unwrap();
        let layered = PluginRegistry::load_layered(project.path());
        restore_home(prev_home);
        assert!(layered.unwrap().plugins.is_empty());
    }

    #[test]
    fn restore_home_both_branches() {
        let _guard = env_lock();
        let prev = std::env::var_os("WHYCODE_HOME");
        restore_home(Some(std::ffi::OsString::from("/tmp")));
        restore_home(None);
        restore_home(prev);
    }

    #[test]
    fn env_lock_recovers_from_poison() {
        let _ = std::thread::spawn(|| {
            let _guard = env_lock();
            panic!("poison the skill env lock");
        })
        .join();
        let _guard = env_lock();
    }
}
