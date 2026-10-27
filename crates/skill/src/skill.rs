use serde::{Deserialize, Serialize};
use std::path::Path;

/// A skill definition loaded from a `.skill.md` file.
///
/// Skills augment agent behaviour with domain-specific instructions,
/// tool constraints, and file-pattern filters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Unique name of the skill.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// The system/meta prompt injected when the skill is active.
    pub prompt: String,
    /// Tools the agent is permitted to use while this skill is active.
    pub tools_allowed: Vec<String>,
    /// File glob patterns that trigger or scope the skill.
    pub file_patterns: Vec<String>,
    /// Folder of a `<name>/SKILL.md` skill: the paths its body names
    /// (`references/`, `scripts/`, a plugin's `agents/`) are relative to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<std::path::PathBuf>,
}

impl Skill {
    /// Load a skill from a `.skill.md` file.
    ///
    /// The file format is Markdown with YAML-like frontmatter delimited by `---`:
    ///
    /// ```text
    /// ---
    /// name: my-skill
    /// description: Does something useful
    /// tools_allowed:
    ///   - read
    ///   - write
    /// file_patterns:
    ///   - "*.rs"
    /// ---
    ///
    /// # Skill: My Skill
    ///
    /// The rest of the file is the prompt / instruction text.
    /// ```
    pub fn from_file(path: &Path) -> crate::error::Result<Self> {
        let content = std::fs::read_to_string(path)?;

        let mut parts = content.splitn(3, "---");

        // First split yields empty string before the first `---`
        let _empty = parts.next();

        // Second split is the frontmatter block
        let frontmatter = parts
            .next()
            .ok_or_else(|| {
                crate::error::SkillError::msg(format!(
                    "Invalid skill file {:?}: missing frontmatter block between '---' markers",
                    path
                ))
            })?
            .trim();

        // Third split is the prompt body (everything after the closing `---`)
        let prompt = parts.next().unwrap_or("").trim().to_string();

        // Parse frontmatter with simple line-based key: value parser
        let parsed = parse_frontmatter(frontmatter)?;

        let name = parsed.get("name").cloned().ok_or_else(|| {
            crate::error::SkillError::msg(format!(
                "Skill file {:?} missing required field 'name'",
                path
            ))
        })?;

        let description = parsed.get("description").cloned().unwrap_or_default();

        let tools_allowed = parse_string_list(parsed.get("tools_allowed"));

        let file_patterns = parse_string_list(parsed.get("file_patterns"));

        let dir = path
            .file_name()
            .is_some_and(|f| f == "SKILL.md")
            .then(|| path.parent().map(Path::to_path_buf))
            .flatten();

        Ok(Skill {
            name,
            description,
            prompt,
            tools_allowed,
            file_patterns,
            dir,
        })
    }

    /// Where the skill's relative paths resolve, shown with its body. A
    /// skill inside a plugin package (`<plugin>/skills/<name>/SKILL.md`
    /// next to `<plugin>/plugin.json`) also names the plugin root, where
    /// shared `agents/` and `resources/` folders live.
    pub fn location_note(&self) -> String {
        let Some(dir) = &self.dir else {
            return String::new();
        };
        let mut note = format!(
            "Skill folder: `{}` (relative paths in this skill resolve here).",
            dir.display()
        );
        if let Some(root) = plugin_root(dir) {
            note.push_str(&format!(
                "\nPlugin root: `{}` (paths such as `agents/` or `resources/` resolve here).",
                root.display()
            ));
        }
        note
    }
}

/// `<plugin>` for a skill folder at `<plugin>/skills/<name>` when
/// `<plugin>/plugin.json` exists.
fn plugin_root(skill_dir: &Path) -> Option<&Path> {
    let skills = skill_dir.parent()?;
    let root = skills.parent()?;
    (skills.file_name()? == "skills" && root.join("plugin.json").is_file()).then_some(root)
}

/// Simple frontmatter parser that handles `key: value` lines and
/// `key:` followed by indented `- item` list entries.
///
/// This is deliberately not a full YAML parser — it covers the
/// subset needed for skill frontmatter.
fn parse_frontmatter(
    input: &str,
) -> crate::error::Result<std::collections::HashMap<String, String>> {
    let mut map = std::collections::HashMap::new();
    let mut current_key: Option<String> = None;
    let mut current_list = String::new();

    for line in input.lines() {
        let trimmed = line.trim();

        // Skip empty lines and full-line comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // List item under the current key
        if let Some(stripped) = trimmed.strip_prefix("- ") {
            if let Some(ref _key) = current_key {
                if !current_list.is_empty() {
                    current_list.push('\n');
                }
                current_list.push_str(stripped.trim());
            }
            continue;
        }

        // Flush any pending list before starting a new key
        if let Some(ref key) = current_key {
            map.insert(key.clone(), std::mem::take(&mut current_list));
            current_key = None;
        }

        // Key: value line
        if let Some((key, value)) = trimmed.split_once(':') {
            let key = key.trim().to_string();
            let value = value.trim().to_string();

            if value.is_empty() {
                // Possibly the start of a list block
                current_key = Some(key);
                current_list = String::new();
            } else {
                map.insert(key, value);
            }
        }
    }

    // Flush the last pending list
    if let Some(ref key) = current_key {
        map.insert(key.clone(), current_list);
    }

    Ok(map)
}

/// Helper: parse a value that may be a newline-separated list into a `Vec<String>`.
fn parse_string_list(value: Option<&String>) -> Vec<String> {
    match value {
        Some(s) if !s.is_empty() => s
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.skill.md");
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[test]
    fn parse_simple_skill() {
        let content = "---\nname: rust-dev\ndescription: Rust development skill\ntools_allowed:\n  - read\n  - write\n  - edit\nfile_patterns:\n  - \"*.rs\"\n  - \"Cargo.toml\"\n---\n\n# Skill: Rust Dev\n\nYou are a Rust expert.\n";
        let (_dir, path) = write_temp(content);
        let skill = Skill::from_file(&path).unwrap();
        assert_eq!(skill.name, "rust-dev");
        assert_eq!(skill.description, "Rust development skill");
        assert_eq!(skill.tools_allowed, vec!["read", "write", "edit"]);
        assert_eq!(skill.file_patterns, vec!["\"*.rs\"", "\"Cargo.toml\""]);
        assert!(skill.prompt.contains("You are a Rust expert"));
    }

    #[test]
    fn missing_name_is_error() {
        let content = "---\ndescription: no name\n---\n\nbody\n";
        let (_dir, path) = write_temp(content);
        assert!(Skill::from_file(&path).is_err());
    }

    #[test]
    fn empty_frontmatter_fields_default() {
        let content = "---\nname: minimal\n---\n\nJust a prompt.\n";
        let (_dir, path) = write_temp(content);
        let skill = Skill::from_file(&path).unwrap();
        assert_eq!(skill.name, "minimal");
        assert_eq!(skill.description, "");
        assert!(skill.tools_allowed.is_empty());
        assert!(skill.file_patterns.is_empty());
        assert_eq!(skill.prompt, "Just a prompt.");
    }

    #[test]
    fn location_note_names_the_skill_folder_and_plugin_root() {
        let (_dir, path) = write_temp("---\nname: flat\n---\nbody\n");
        let flat = Skill::from_file(&path).unwrap();
        assert!(flat.dir.is_none());
        assert_eq!(flat.location_note(), "");

        let root = tempfile::tempdir().unwrap();
        let plugin = root.path().join("egm");
        let folder = plugin.join("skills").join("egm-review");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("SKILL.md"),
            "---\nname: egm-review\n---\nbody\n",
        )
        .unwrap();
        let loose = Skill::from_file(&folder.join("SKILL.md")).unwrap();
        assert_eq!(loose.dir.as_deref(), Some(folder.as_path()));
        let note = loose.location_note();
        assert!(note.contains(&folder.display().to_string()), "{note}");
        assert!(!note.contains("Plugin root"), "no plugin.json yet: {note}");

        std::fs::write(plugin.join("plugin.json"), "{\"name\":\"egm\"}").unwrap();
        let packaged = Skill::from_file(&folder.join("SKILL.md")).unwrap();
        let note = packaged.location_note();
        assert!(
            note.contains(&format!("Plugin root: `{}`", plugin.display())),
            "{note}"
        );
    }

    #[test]
    fn missing_file_is_error() {
        assert!(Skill::from_file(std::path::Path::new("/no/such/skill.md")).is_err());
    }

    #[test]
    fn missing_frontmatter_markers_is_error() {
        let (_dir, path) = write_temp("just a body with no markers\n");
        let err = Skill::from_file(&path).unwrap_err();
        assert!(err.to_string().contains("frontmatter"), "{err}");
    }

    #[test]
    fn missing_name_field_is_error() {
        let (_dir, path) = write_temp("---\ndescription: no name\n---\nbody\n");
        let err = Skill::from_file(&path).unwrap_err();
        assert!(err.to_string().contains("name"), "{err}");
    }

    #[test]
    fn comments_blank_lines_and_orphan_list_items() {
        let parsed = parse_frontmatter(
            "# comment\n\n- orphan\nnot a pair\nname: demo\ntools_allowed:\n  - read\n# mid\n  - write\n",
        )
        .unwrap();
        assert_eq!(parsed.get("name").map(String::as_str), Some("demo"));
        assert_eq!(
            parsed.get("tools_allowed").map(String::as_str),
            Some("read\nwrite")
        );
    }

    #[test]
    fn empty_list_value_and_empty_string_list() {
        assert!(parse_string_list(None).is_empty());
        assert!(parse_string_list(Some(&String::new())).is_empty());
        let parsed = parse_frontmatter("name: x\ntools_allowed:\n").unwrap();
        assert!(parse_string_list(parsed.get("tools_allowed")).is_empty());
    }
}
