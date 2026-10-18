//! `whycodes config`: show, get, set, keys and path over dotted keys.
use crate::args::*;
use colored::*;
use toml::Value;
use whycodes_config::dotted::{self, REDACTED};
use whycodes_config::{Config, ConfigTarget};

pub(crate) async fn cmd_config(
    project_dir: &std::path::Path,
    cmd: &ConfigCmd,
) -> anyhow::Result<()> {
    // Creates the first-run file; the rest reads it as written, so `${VAR}`
    // placeholders print as names.
    Config::load_or_create()?;
    let global = Config::load_global_raw()?;

    match cmd {
        ConfigCmd::Show => {
            let config_path = Config::default_path()?;
            println!("{}", config_path_line(&config_path.display().to_string()));
            println!();
            let tree = dotted::redact(&global.to_toml_value()?, &[]);
            println!("{}", dotted::render(&tree));
        }
        ConfigCmd::Get { key, json } => {
            let path = dotted::parse_key(canonical_key(key))?;
            let layers = layer_values(&config_layers(&global, project_dir))?;
            let resolved = resolve_config_value(&layers, &path);
            if *json {
                let out = get_json(key, resolved.as_ref())?;
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                match resolved {
                    Some((value, layer)) => {
                        println!("{}", config_get_line(&dotted::render(&value), layer))
                    }
                    None => eprintln!("{}", config_key_missing_line(key)),
                }
            }
        }
        ConfigCmd::Set {
            key,
            value,
            project,
        } => {
            let target = if *project {
                ConfigTarget::Project
            } else {
                ConfigTarget::Global
            };
            let key = canonical_key(key);
            let file = Config::set_dotted(project_dir, target, key, value)?;
            let shown = if dotted::is_secret_key(&dotted::parse_key(key)?) {
                REDACTED
            } else {
                value.as_str()
            };
            let file = file.display().to_string();
            println!("{}", config_set_line(key, shown, &file));
        }
        ConfigCmd::Keys { prefix, json } => {
            let layers = layer_values(&config_layers(&global, project_dir))?;
            let rows = config_keys(&layers, prefix.as_deref().map(canonical_key))?;
            if *json {
                let rows: Vec<serde_json::Value> = rows
                    .iter()
                    .map(|(key, value, layer)| get_json(key, Some(&(value.clone(), *layer))))
                    .collect::<anyhow::Result<_>>()?;
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for (key, value, layer) in &rows {
                    println!("{}", config_get_line(&format!("{key} = {value}"), layer));
                }
            }
        }
        ConfigCmd::Path => {
            let config_path = Config::default_path()?;
            println!("{}", config_path.display());
        }
    }

    Ok(())
}

/// The two keys `config get|set` took before dotted paths, as dotted paths.
pub(crate) fn canonical_key(key: &str) -> &str {
    match key {
        "log_level" => "general.log_level",
        "project_path" => "general.project_path",
        other => other,
    }
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

/// [`config_layers`] as value trees for dotted lookups.
pub(crate) fn layer_values(
    layers: &[(&'static str, Config)],
) -> anyhow::Result<Vec<(&'static str, Value)>> {
    layers
        .iter()
        .map(|(layer, config)| Ok((*layer, config.to_toml_value()?)))
        .collect()
}

/// The final value at `path` (secrets redacted) and the last layer that
/// changed it.
pub(crate) fn resolve_config_value(
    layers: &[(&'static str, Value)],
    path: &[String],
) -> Option<(Value, &'static str)> {
    let mut winner: Option<(Option<&Value>, &'static str)> = None;
    for (layer, tree) in layers {
        let value = dotted::lookup(tree, path);
        match &winner {
            Some((current, _)) if *current == value => {}
            _ => winner = Some((value, layer)),
        }
    }
    let (value, layer) = winner?;
    Some((dotted::redact(value?, path), layer))
}

/// Every leaf key of the effective (last) layer under `prefix`, as
/// (dotted key, value with secrets redacted, winning layer).
pub(crate) fn config_keys(
    layers: &[(&'static str, Value)],
    prefix: Option<&str>,
) -> anyhow::Result<Vec<(String, Value, &'static str)>> {
    let prefix = match prefix {
        Some(prefix) => dotted::parse_key(prefix)?,
        None => Vec::new(),
    };
    let root = layers
        .last()
        .and_then(|(_, effective)| dotted::lookup(effective, &prefix));
    let Some(root) = root else {
        return Ok(Vec::new());
    };
    Ok(dotted::leaves(root)
        .into_iter()
        .filter_map(|(rest, _)| {
            let path = [prefix.clone(), rest].concat();
            let (value, layer) = resolve_config_value(layers, &path)?;
            Some((dotted::format_key(&path), value, layer))
        })
        .collect())
}

/// `{ "key", "value", "overriddenBy" }`; `value` keeps its TOML type.
pub(crate) fn get_json(
    key: &str,
    resolved: Option<&(Value, &'static str)>,
) -> anyhow::Result<serde_json::Value> {
    let value = resolved
        .map(|(value, _)| serde_json::to_value(value))
        .transpose()?;
    Ok(serde_json::json!({
        "key": key,
        "value": value,
        "overriddenBy": resolved.map(|(_, layer)| layer),
    }))
}

pub(crate) fn config_key_missing_line(key: &str) -> String {
    format!("{} Key '{}' not found.", "✗".red(), key)
}

pub(crate) fn config_set_line(key: &str, value: &str, file: &str) -> String {
    format!(
        "{} Set '{}' = '{}' in {}",
        "✓".green(),
        key.cyan(),
        value,
        file
    )
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
