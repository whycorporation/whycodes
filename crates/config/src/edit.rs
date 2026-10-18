//! `whycodes config set`: write one dotted key to the global `config.toml`
//! or the project overlay, after checking the result loads and validates.

use std::path::{Path, PathBuf};

use crate::dotted::{lookup, parse_key, set_in_document};
use crate::load::{
    ensure_parent_dir, parse_global_text, parse_project_text, toml_err, write_atomic,
};
use crate::types::Config;
use whycodes_core::{Error, Result};

/// The file [`Config::set_dotted`] writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigTarget {
    /// `config.toml` in the WhyCodes home.
    Global,
    /// `<project>/.whycodes/config.toml`.
    Project,
}

impl Config {
    /// The project overlay: `<project>/.whycodes/config.toml`.
    pub fn project_config_path(project_dir: &Path) -> PathBuf {
        whycodes_core::project_dir(project_dir).join("config.toml")
    }

    /// The whole config as a `toml::Value` tree, for dotted lookups.
    pub fn to_toml_value(&self) -> Result<toml::Value> {
        map_toml_value(toml::Value::try_from(self))
    }

    /// Set the dotted `key` to `raw` in one config file and return its path.
    ///
    /// `raw` is a TOML literal (bool, number, array, inline table); text
    /// that is not one, or whose type does not fit the key, is tried as a
    /// string. Only that key changes in the file: other keys, comments and
    /// layout stay. Refused, with the file untouched, when the result does
    /// not load as a config, adds a validation error, would not change the
    /// effective value (unknown key, or a merge that keeps the lower layer),
    /// or (project) sets a key the project layer may not set. The first
    /// write to an existing file copies it to `<file>.bak`.
    pub fn set_dotted(
        project_dir: &Path,
        target: ConfigTarget,
        key: &str,
        raw: &str,
    ) -> Result<PathBuf> {
        set_dotted_in(&Self::default_path()?, project_dir, target, key, raw)
    }

    /// Defaults, then `global`, then `project` (what `load_layered` merges
    /// before its env overlay).
    fn layered(global: &Config, project: Option<&Config>) -> Config {
        let merged = Config::default().merge_with(global);
        match project {
            Some(project) => merged.merge_with(project),
            None => merged,
        }
    }
}

pub(crate) fn set_dotted_in(
    global_path: &Path,
    project_dir: &Path,
    target: ConfigTarget,
    key: &str,
    raw: &str,
) -> Result<PathBuf> {
    let path = parse_key(key)?;
    let project_path = Config::project_config_path(project_dir);
    let global_text = read_optional(global_path)?;
    let project_text = read_optional(&project_path)?;
    let (file, original) = match target {
        ConfigTarget::Global => (global_path.to_path_buf(), &global_text),
        ConfigTarget::Project => (project_path.clone(), &project_text),
    };
    // The target first: a malformed target fails here, the other file below.
    let (text, layer) = patch_text(original.as_deref().unwrap_or_default(), &path, raw)?;
    let global = parse_global_text(global_text.as_deref().unwrap_or_default())?;
    let project = project_layer(project_text.as_deref(), &project_path)?;
    // The layers up to the target, before and after the edit. A global
    // edit is judged without the project overlay: it applies to every
    // project, and this one may override it.
    let (before, after) = match target {
        ConfigTarget::Global => (
            Config::layered(&global, None),
            Config::layered(&layer, None),
        ),
        ConfigTarget::Project => {
            let mut overlay = layer.clone();
            crate::load::clear_unset_default_agent(&mut overlay, &text);
            if !overlay.websearch.strip_project_only_keys().is_empty()
                && path.first().is_some_and(|seg| seg == "websearch")
            {
                return Err(Error::Config(format!(
                    "{key} cannot be set in the project config: the custom search endpoint, its headers and header_env are read only from the global config."
                )));
            }
            (
                Config::layered(&global, project.as_ref()),
                Config::layered(&global, Some(&overlay)),
            )
        }
    };

    let wanted = lookup(&layer.to_toml_value()?, &path).cloned();
    let Some(wanted) = wanted else {
        return Err(Error::Config(format!("unknown config key: {key}")));
    };
    refuse_new_issues(&before, &after)?;
    if lookup(&after.to_toml_value()?, &path) != Some(&wanted) {
        return Err(Error::Config(format!(
            "{key} in {} would have no effect: the merged config keeps the value from a lower layer.",
            file.display()
        )));
    }
    if original.is_some() {
        backup_once(&file)?;
    }
    ensure_parent_dir(&file)?;
    write_atomic(&file, text.as_bytes())?;
    Ok(file)
}

/// Kept apart from `to_toml_value`: serializing a `Config` does not fail
/// today, so only a test reaches the error arm.
fn map_toml_value(r: std::result::Result<toml::Value, toml::ser::Error>) -> Result<toml::Value> {
    match r {
        Ok(value) => Ok(value),
        Err(e) => Err(toml_err(e.to_string())),
    }
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(std::fs::read_to_string(path)?))
}

fn project_layer(text: Option<&str>, path: &Path) -> Result<Option<Config>> {
    text.map(|text| parse_project_text(text, path).map_err(|e| toml_err(e.to_string())))
        .transpose()
}

/// `original` with `path` set from `raw`, and the file it makes, parsed.
/// `raw` as a TOML literal first, then as a string; the first error wins.
fn patch_text(original: &str, path: &[String], raw: &str) -> Result<(String, Config)> {
    let doc: toml_edit::DocumentMut = original.parse().map_err(|e| toml_err(format!("{e}")))?;
    let as_string = || try_value(&doc, path, toml_edit::Value::from(raw));
    match raw.parse::<toml_edit::Value>().ok() {
        Some(value) => {
            try_value(&doc, path, value).or_else(|typed| as_string().map_err(|_s| typed))
        }
        None => as_string(),
    }
}

fn try_value(
    doc: &toml_edit::DocumentMut,
    path: &[String],
    value: toml_edit::Value,
) -> Result<(String, Config)> {
    let mut doc = doc.clone();
    set_in_document(&mut doc, path, value)?;
    let text = doc.to_string();
    let config = parse_global_text(&text)?;
    Ok((text, config))
}

/// Refuse validation errors `after` has and `before` did not. A config that
/// already has an issue (a provider whose key lives outside the file) can
/// still change unrelated keys.
fn refuse_new_issues(before: &Config, after: &Config) -> Result<()> {
    let with_env = |config: &Config| {
        let mut config = config.clone();
        config.apply_env_overrides();
        config.validation_errors()
    };
    let old = with_env(before);
    let new: Vec<String> = with_env(after)
        .into_iter()
        .filter(|issue| !old.contains(issue))
        .collect();
    if new.is_empty() {
        return Ok(());
    }
    Err(Error::Config(new.join("; ")))
}

/// Copy `file` to `<file>.bak` unless that backup already exists, so the
/// backup keeps the file as it was before the first edit.
fn backup_once(file: &Path) -> Result<()> {
    let mut name = file.as_os_str().to_owned();
    name.push(".bak");
    let backup = PathBuf::from(name);
    if !backup.exists() {
        std::fs::copy(file, &backup)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
