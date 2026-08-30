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

        Ok(Skill {
            name,
            description,
            prompt,
            tools_allowed,
            file_patterns,
        })
    }
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
