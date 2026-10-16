//! Config get/set.
use crate::args::*;
use colored::*;
use std::path::PathBuf;
use whycodes_config::Config;

pub(crate) async fn cmd_config(
    project_dir: &std::path::Path,
    cmd: &ConfigCmd,
) -> anyhow::Result<()> {
    let config = Config::load_or_create()?;

    match cmd {
        ConfigCmd::Show => {
            let config_path = Config::default_path()?;
            println!("{}", config_path_line(&config_path.display().to_string()));
            println!();
            let text = toml::to_string_pretty(&config)?;
            println!("{}", text);
        }
        ConfigCmd::Get { key, json } => {
            let resolved = resolve_config_value(&config_layers(&config, project_dir), key);
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "key": key,
                        "value": resolved.as_ref().map(|(value, _)| value),
                        "overriddenBy": resolved.as_ref().map(|(_, layer)| layer),
                    }))?
                );
            } else {
                match resolved {
                    Some((value, layer)) => println!("{}", config_get_line(&value, layer)),
                    None => eprintln!("{}", config_key_missing_line(key)),
                }
            }
        }
        ConfigCmd::Set { key, value } => {
            let mut config = config.clone();
            set_config_value(&mut config, key, value)?;
            config.save()?;
            println!("{}", config_set_line(key, value));
        }
        ConfigCmd::Path => {
            let config_path = Config::default_path()?;
            println!("{}", config_path.display());
        }
    }

    Ok(())
}

pub(crate) fn config_path_line(path: &str) -> String {
    format!("{} Config path: {}", "⚙".bold(), path.cyan())
}

pub(crate) fn config_get_line(value: &str, layer: &str) -> String {
    format!("{value}  {}", format!("(from: {layer})").dimmed())
}

/// The layers of `Config::load_layered`, each as the config resolved up to
/// that point: built-in defaults, then the global file merged in, then the
/// project file, then `WHYCODES_*` env overrides. `global` is the already
/// loaded global config so the file is read once.
pub(crate) fn config_layers(
    global: &Config,
    project_dir: &std::path::Path,
) -> [(&'static str, Config); 4] {
    let defaults = Config::default();
    let with_global = defaults.merge_with(global);
    let with_project = match Config::load_project_layer(project_dir) {
        Some(project) => with_global.merge_with(&project),
        None => with_global.clone(),
    };
    let mut with_env = with_project.clone();
    with_env.apply_env_overrides();
    [
        ("default", defaults),
        ("global", with_global),
        ("project", with_project),
        ("env", with_env),
    ]
}

/// The final value of `key` and the last layer that changed it.
pub(crate) fn resolve_config_value(
    layers: &[(&'static str, Config)],
    key: &str,
) -> Option<(String, &'static str)> {
    let mut winner: Option<(Option<String>, &'static str)> = None;
    for (layer, config) in layers {
        let value = get_config_value(config, key);
        match &winner {
            Some((current, _)) if *current == value => {}
            _ => winner = Some((value, layer)),
        }
    }
    let (value, layer) = winner?;
    Some((value?, layer))
}

pub(crate) fn config_key_missing_line(key: &str) -> String {
    format!("{} Key '{}' not found.", "✗".red(), key)
}

pub(crate) fn config_set_line(key: &str, value: &str) -> String {
    format!("{} Set '{}' = '{}'", "✓".green(), key.cyan(), value)
}

pub(crate) fn get_config_value(config: &Config, key: &str) -> Option<String> {
    match key {
        "default_agent" => Some(config.default_agent.clone()),
        "project_path" => config
            .general
            .project_path
            .as_ref()
            .map(|p| p.display().to_string()),
        "log_level" => config.general.log_level.clone(),
        _ => None,
    }
}

pub(crate) fn set_config_value(config: &mut Config, key: &str, value: &str) -> anyhow::Result<()> {
    match key {
        "default_agent" => {
            config.default_agent = value.to_string();
        }
        "project_path" => {
            config.general.project_path = Some(PathBuf::from(value));
        }
        "log_level" => {
            config.general.log_level = Some(value.to_string());
        }
        _ => {
            anyhow::bail!(
                "Unknown config key: {}. Supported: default_agent, project_path, log_level",
                key
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
