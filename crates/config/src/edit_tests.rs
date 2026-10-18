use super::*;
use crate::encode_toml;

struct Fixture {
    _guard: std::sync::MutexGuard<'static, ()>,
    dir: tempfile::TempDir,
    global: PathBuf,
}

impl Fixture {
    /// A seeded global `config.toml` with a leading comment, no project file.
    fn new() -> Self {
        let guard = crate::tests::lock_env();
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("home").join("config.toml");
        std::fs::create_dir_all(global.parent().unwrap()).unwrap();
        let text = format!(
            "# my settings\n{}",
            encode_toml(&Config::seeded_openrouter()).unwrap()
        );
        std::fs::write(&global, text).unwrap();
        Self {
            _guard: guard,
            dir,
            global,
        }
    }

    fn project_dir(&self) -> PathBuf {
        self.dir.path().join("proj")
    }

    fn project_file(&self) -> PathBuf {
        Config::project_config_path(&self.project_dir())
    }

    fn set(&self, target: ConfigTarget, key: &str, raw: &str) -> Result<PathBuf> {
        set_dotted_in(&self.global, &self.project_dir(), target, key, raw)
    }

    fn global_text(&self) -> String {
        std::fs::read_to_string(&self.global).unwrap()
    }

    fn global_config(&self) -> Config {
        Config::layered(&parse_global_text(&self.global_text()).unwrap(), None)
    }
}

fn bak(file: &Path) -> PathBuf {
    PathBuf::from(format!("{}.bak", file.display()))
}

/// Refused: the error names the problem and the file is byte-for-byte unchanged.
fn assert_refused(fx: &Fixture, key: &str, raw: &str, needle: &str) {
    let before = fx.global_text();
    let err = fx
        .set(ConfigTarget::Global, key, raw)
        .unwrap_err()
        .to_string();
    assert!(err.contains(needle), "{key}={raw}: {err}");
    assert_eq!(fx.global_text(), before, "{key}={raw} changed the file");
    assert!(!bak(&fx.global).exists(), "{key}={raw} made a backup");
}

#[test]
fn global_set_changes_one_line_and_backs_up_once() {
    let fx = Fixture::new();
    let original = fx.global_text();
    let written = fx
        .set(ConfigTarget::Global, "session.tool_profile", "full")
        .unwrap();
    assert_eq!(written, fx.global);
    let after = fx.global_text();
    assert!(after.starts_with("# my settings\n"));
    let changed: Vec<(&str, &str)> = original
        .lines()
        .zip(after.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0].1, "tool_profile = \"full\"");
    assert_eq!(original.lines().count(), after.lines().count());
    assert_eq!(fx.global_config().session.tool_profile, "full");
    assert_eq!(std::fs::read_to_string(bak(&fx.global)).unwrap(), original);

    // A second write keeps the first backup.
    fx.set(ConfigTarget::Global, "memory.auto_inject", "false")
        .unwrap();
    assert!(!fx.global_config().memory.auto_inject);
    assert_eq!(std::fs::read_to_string(bak(&fx.global)).unwrap(), original);
}

#[test]
fn values_parse_as_toml_then_fall_back_to_a_string() {
    let fx = Fixture::new();
    fx.set(ConfigTarget::Global, "general.log_level", "5")
        .unwrap();
    assert_eq!(fx.global_config().general.log_level.as_deref(), Some("5"));
    fx.set(ConfigTarget::Global, "session.max_context_tokens", "64000")
        .unwrap();
    assert_eq!(fx.global_config().session.max_context_tokens, 64000);
    fx.set(ConfigTarget::Global, "general.log_level", "\"debug\"")
        .unwrap();
    assert_eq!(
        fx.global_config().general.log_level.as_deref(),
        Some("debug")
    );
}

#[test]
fn refusals_leave_the_file_unchanged() {
    let fx = Fixture::new();
    assert_refused(&fx, "memory.auto_inject", "maybe", "invalid type");
    // A typed value that does not fit reports the typed error.
    assert_refused(&fx, "memory.auto_inject", "5", "integer");
    assert_refused(
        &fx,
        "default_agent",
        "nope",
        "Default agent 'nope' not found",
    );
    assert_refused(&fx, "slop.verbosity", "2.0", "slop.verbosity is 2");
    assert_refused(&fx, "nope.x", "1", "unknown config key: nope.x");
    // The merge keeps a built-in agent's description from the defaults.
    assert_refused(&fx, "agents.build.description", "x", "would have no effect");
    assert_refused(
        &fx,
        "agents.nobody.model",
        "x",
        "no 'agents' entry named 'nobody'",
    );
    assert_refused(&fx, "a..b", "1", "invalid config key");
}

#[test]
fn agent_model_settings_set_in_a_higher_layer_take_effect() {
    let fx = Fixture::new();
    fx.set(
        ConfigTarget::Global,
        "agents.build.model",
        "{ provider_id = \"openrouter\", model_id = \"x/strong\" }",
    )
    .unwrap();
    fx.set(ConfigTarget::Global, "agents.build.temperature", "0.25")
        .unwrap();
    fx.set(ConfigTarget::Global, "agents.plan.top_p", "0.5")
        .unwrap();
    let config = fx.global_config();
    let build = config.get_agent("build").unwrap();
    assert_eq!(build.model.as_ref().unwrap().model_id, "x/strong");
    assert_eq!(build.temperature, Some(0.25));
    assert_eq!(config.get_agent("plan").unwrap().top_p, Some(0.5));
}

#[test]
fn project_set_writes_only_the_overlay() {
    let fx = Fixture::new();
    let global = fx.global_text();
    let written = fx
        .set(ConfigTarget::Project, "session.tool_profile", "full")
        .unwrap();
    assert_eq!(written, fx.project_file());
    assert_eq!(
        std::fs::read_to_string(&written).unwrap(),
        "[session]\ntool_profile = \"full\"\n"
    );
    // A new file has nothing to back up; the global file is untouched.
    assert!(!bak(&written).exists());
    assert_eq!(fx.global_text(), global);
    assert!(!bak(&fx.global).exists());

    fx.set(ConfigTarget::Project, "default_agent", "plan")
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(bak(&written)).unwrap(),
        "[session]\ntool_profile = \"full\"\n"
    );
    let layered = Config::load_project_layer(&fx.project_dir()).unwrap();
    assert_eq!(layered.default_agent, "plan");
    assert_eq!(layered.session.tool_profile, "full");
}

#[test]
fn project_set_refuses_keys_the_project_layer_drops() {
    let fx = Fixture::new();
    for (key, raw) in [
        ("websearch.custom", "{ endpoint = \"https://x.example\" }"),
        ("websearch.engine", "custom"),
        ("websearch.fallback", "[\"custom\"]"),
    ] {
        let err = fx
            .set(ConfigTarget::Project, key, raw)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("cannot be set in the project config"),
            "{key}: {err}"
        );
        assert!(!fx.project_file().exists(), "{key} wrote the overlay");
    }
    // Other websearch keys are fine.
    fx.set(ConfigTarget::Project, "websearch.engine", "brave")
        .unwrap();
}

#[test]
fn malformed_or_unreadable_files_are_errors() {
    let fx = Fixture::new();
    std::fs::create_dir_all(fx.project_file().parent().unwrap()).unwrap();
    std::fs::write(fx.project_file(), "not = = toml").unwrap();
    // Malformed target, and malformed other layer.
    assert!(
        fx.set(ConfigTarget::Project, "default_agent", "plan")
            .is_err()
    );
    assert!(
        fx.set(ConfigTarget::Global, "default_agent", "plan")
            .is_err()
    );
    std::fs::remove_file(fx.project_file()).unwrap();
    std::fs::write(&fx.global, "not = = toml").unwrap();
    assert!(
        fx.set(ConfigTarget::Project, "default_agent", "plan")
            .is_err()
    );
    // A directory where the file should be cannot be read.
    std::fs::create_dir_all(fx.project_file()).unwrap();
    assert!(
        fx.set(ConfigTarget::Project, "default_agent", "plan")
            .is_err()
    );
}

#[test]
fn missing_global_file_is_written_with_just_that_key() {
    let fx = Fixture::new();
    std::fs::remove_file(&fx.global).unwrap();
    fx.set(ConfigTarget::Global, "default_agent", "build")
        .unwrap();
    assert_eq!(fx.global_text(), "default_agent = \"build\"\n");
    assert!(!bak(&fx.global).exists());
}

#[test]
fn global_set_is_judged_without_the_project_overlay() {
    let fx = Fixture::new();
    // Any project file resets `default_agent` in the merge; a global edit
    // still applies to every other project.
    fx.set(ConfigTarget::Project, "memory.auto_inject", "false")
        .unwrap();
    fx.set(ConfigTarget::Global, "default_agent", "plan")
        .unwrap();
    assert_eq!(fx.global_config().default_agent, "plan");
    let err = fx
        .set(ConfigTarget::Global, "default_agent", "nope")
        .unwrap_err()
        .to_string();
    assert!(err.contains("Default agent 'nope' not found"), "{err}");
}

#[test]
fn value_serialization_errors_become_config_errors() {
    let err = <toml::ser::Error as serde::ser::Error>::custom("nope");
    assert!(map_toml_value(Err(err)).is_err());
}
