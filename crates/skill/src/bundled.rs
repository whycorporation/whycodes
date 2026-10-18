//! Skills compiled into the binary.
//!
//! Today there is one: `whycodes`, the product's own manual. Its body is a
//! topic index; each topic is a separate Markdown file loaded on demand with
//! `read skill://whycodes/<topic>`, so a system prompt carries at most the
//! index (names and one-line descriptions), never a topic body.

use crate::skill::Skill;

/// Name of the bundled product manual skill.
pub const WHYCODES_SKILL: &str = "whycodes";

const WHYCODES_DESCRIPTION: &str = "WhyCodes itself: config files and layers, agents, tools, \
     providers, permissions, repair. Load one topic: `read skill://whycodes/<topic>`.";

/// One independently loadable topic of a bundled skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Topic {
    pub name: &'static str,
    pub description: &'static str,
    pub body: &'static str,
}

/// Topics of the `whycodes` skill, in index order.
pub const WHYCODES_TOPICS: &[Topic] = &[
    Topic {
        name: "overview",
        description: "Process model: CLI/TUI, sessions, named agents vs task/swarm workers",
        body: include_str!("bundled/whycodes/overview.md"),
    },
    Topic {
        name: "config",
        description: "File locations, layer order, `whycodes config`, `/doctor`, `debug --json`",
        body: include_str!("bundled/whycodes/config.md"),
    },
    Topic {
        name: "agents",
        description: "build/plan/ask/whycodes, subagents general/explore/scout, custom agents, switching",
        body: include_str!("bundled/whycodes/agents.md"),
    },
    Topic {
        name: "tools",
        description: "Core vs full tool profile, `tool_search`, internal URLs, formatter/LSP after edits",
        body: include_str!("bundled/whycodes/tools.md"),
    },
    Topic {
        name: "providers",
        description: "Providers and models, credential order, subscription login, local servers",
        body: include_str!("bundled/whycodes/providers.md"),
    },
    Topic {
        name: "skills",
        description: "Skill catalog vs body, where skill files live, bundled skills",
        body: include_str!("bundled/whycodes/skills.md"),
    },
    Topic {
        name: "memory",
        description: "MEMORY.md, semantic facts, code and session recall, `--no-memory`",
        body: include_str!("bundled/whycodes/memory.md"),
    },
    Topic {
        name: "mcp-lsp",
        description: "`[mcp_servers]`, `{server}_{tool}` names, MCP OAuth, `[lsp]` servers",
        body: include_str!("bundled/whycodes/mcp-lsp.md"),
    },
    Topic {
        name: "permissions-hooks",
        description: "`[permission]`, path and shell rules, approval mode, risk, sandbox, `[[hooks]]`",
        body: include_str!("bundled/whycodes/permissions-hooks.md"),
    },
    Topic {
        name: "session-tui",
        description: "Slash commands, keys, `--plain`, `--format json|stream-json`, headless asks",
        body: include_str!("bundled/whycodes/session-tui.md"),
    },
    Topic {
        name: "repair",
        description: "How to change config safely: dotted keys, layer choice, validation, backups",
        body: include_str!("bundled/whycodes/repair.md"),
    },
];

/// Topic index of the `whycodes` skill: names and descriptions only.
pub fn whycodes_catalog_markdown() -> String {
    let mut out = String::from(
        "# WhyCodes manual (skill `whycodes`)\n\n\
         Load one topic per question with `read skill://whycodes/<topic>` \
         (or `skill` action=load, name `whycodes/<topic>`). Never load them all.\n\n",
    );
    for topic in WHYCODES_TOPICS {
        out.push_str("- `");
        out.push_str(topic.name);
        out.push_str("`: ");
        out.push_str(topic.description);
        out.push('\n');
    }
    out
}

/// The bundled `whycodes` skill. Its body is the topic index.
pub fn whycodes_skill() -> Skill {
    Skill {
        name: WHYCODES_SKILL.to_string(),
        description: WHYCODES_DESCRIPTION.to_string(),
        prompt: whycodes_catalog_markdown(),
        tools_allowed: Vec::new(),
        file_patterns: Vec::new(),
    }
}

/// Resolve `<skill>/<topic>` (case-insensitive) to a bundled topic.
pub fn topic(skill: &str, topic: &str) -> Result<&'static Topic, String> {
    if !skill.eq_ignore_ascii_case(WHYCODES_SKILL) {
        return Err(format!(
            "Skill '{skill}' has no topics. Only `{WHYCODES_SKILL}/<topic>` is split into topics."
        ));
    }
    match WHYCODES_TOPICS
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(topic.trim()))
    {
        Some(found) => Ok(found),
        None => Err(format!(
            "Unknown topic '{topic}' in skill://{WHYCODES_SKILL}. Topics: {}.",
            topic_names()
        )),
    }
}

/// `skill://<skill>/<topic>` as `read` and `skill` (`action=load`) return it.
pub fn render_topic(skill: &str, name: &str) -> Result<String, String> {
    let found = topic(skill, name)?;
    Ok(format!(
        "# skill://{WHYCODES_SKILL}/{}\n{}\n\n{}",
        found.name, found.description, found.body
    ))
}

fn topic_names() -> String {
    WHYCODES_TOPICS
        .iter()
        .map(|t| t.name)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_lists_every_topic_without_bodies() {
        let catalog = whycodes_catalog_markdown();
        for t in WHYCODES_TOPICS {
            assert!(catalog.contains(&format!("`{}`: {}", t.name, t.description)));
            assert!(!t.body.trim().is_empty(), "{} is empty", t.name);
            assert!(!catalog.contains(t.body.trim()), "{} inlined", t.name);
        }
        let skill = whycodes_skill();
        assert_eq!(skill.name, "whycodes");
        assert_eq!(skill.prompt, catalog);
        assert!(skill.description.contains("skill://whycodes/<topic>"));
    }

    #[test]
    fn topic_resolves_known_and_rejects_unknown() {
        for t in WHYCODES_TOPICS {
            assert_eq!(topic("whycodes", t.name).unwrap().body, t.body);
        }
        assert_eq!(topic("WhyCodes", " Config ").unwrap().name, "config");
        let unknown = topic("whycodes", "nope").unwrap_err();
        assert!(unknown.contains("Unknown topic 'nope'"), "{unknown}");
        assert!(unknown.contains("overview, config"), "{unknown}");
        let other = topic("demo", "x").unwrap_err();
        assert!(other.contains("has no topics"), "{other}");
        let rendered = render_topic("whycodes", "repair").unwrap();
        assert!(
            rendered.starts_with("# skill://whycodes/repair\n"),
            "{rendered}"
        );
        assert!(rendered.ends_with(topic("whycodes", "repair").unwrap().body));
        assert!(render_topic("whycodes", "nope").is_err());
    }
}
