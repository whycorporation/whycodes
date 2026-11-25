use crate::frontmatter::Field;
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

        let parsed = crate::frontmatter::parse(frontmatter);

        let name = parsed.get("name").map(Field::text).ok_or_else(|| {
            crate::error::SkillError::msg(format!(
                "Skill file {:?} missing required field 'name'",
                path
            ))
        })?;

        let description = parsed
            .get("description")
            .map(Field::text)
            .unwrap_or_default();

        let tools_allowed = string_list(parsed.get("tools_allowed"));

        let file_patterns = string_list(parsed.get("file_patterns"));

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

/// A list field: `- item` lines, `[a, b]`, or `a, b`; blanks dropped.
fn string_list(field: Option<&Field>) -> Vec<String> {
    field
        .map(Field::items)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect()
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
        assert_eq!(skill.file_patterns, vec!["*.rs", "Cargo.toml"]);
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
    fn quoted_names_folded_descriptions_and_inline_lists() {
        let (_dir, path) = write_temp(
            "---\nname: \"my-skill\"\ndescription: >\n  Reviews code\n  for bugs: carefully\ntools_allowed: [read, grep]\nfile_patterns:\n---\nbody\n",
        );
        let skill = Skill::from_file(&path).unwrap();
        assert_eq!(skill.name, "my-skill");
        assert_eq!(skill.description, "Reviews code for bugs: carefully");
        assert_eq!(skill.tools_allowed, vec!["read", "grep"]);
        assert!(skill.file_patterns.is_empty());
    }
}
