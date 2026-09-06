//! Config get/set.
use crate::args::*;
use colored::*;
use std::path::PathBuf;
use whycodes_config::Config;

pub(crate) async fn cmd_config(cmd: &ConfigCmd) -> anyhow::Result<()> {
    let config = Config::load()?;

    match cmd {
        ConfigCmd::Show => {
            let config_path = Config::default_path()?;
            println!("{}", config_path_line(&config_path.display().to_string()));
            println!();
            let text = toml::to_string_pretty(&config)?;
            println!("{}", text);
        }
        ConfigCmd::Get { key } => match get_config_value(&config, key) {
            Some(val) => println!("{}", val),
            None => eprintln!("{}", config_key_missing_line(key)),
        },
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
