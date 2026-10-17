use super::*;

fn agent_file(dir: &Path, file: &str, body: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(file);
    std::fs::write(&p, body).unwrap();
    p
}

fn sample(model: Option<&str>) -> CustomAgent {
    CustomAgent {
        name: "rev".into(),
        description: "d".into(),
        prompt: "p".into(),
        tools: None,
        disallowed_tools: Vec::new(),
        model: model.map(str::to_string),
        prewalk: false,
        path: PathBuf::from("rev.md"),
    }
}

#[test]
fn parses_shared_definition_shape() {
    let text = "\u{feff}---\r\nname: reviewer\r\ndescription: Reviews diffs\r\ntools: Read, Grep, Bash, LS, MultiEdit, TodoWrite, mcp__gh__search, Read\r\nmodel: inherit\r\n---\r\nYou review code.\r\n";
    let a = parse(text, Path::new("x.md")).unwrap();
    assert_eq!(a.name, "reviewer");
    assert_eq!(a.description, "Reviews diffs");
    assert_eq!(a.prompt, "You review code.");
    assert_eq!(
        a.tools.unwrap(),
        [
            "read",
            "grep",
            "shell",
            "bash",
            "list",
            "edit",
            "todowrite",
            "gh_search"
        ]
    );
    assert_eq!(a.model.as_deref(), Some("inherit"));
    assert!(a.disallowed_tools.is_empty());
}

#[test]
fn parses_lists_quotes_and_block_text() {
    let text = "---\n# comment\n\ndescription: |\n  First line\n  second line\ntools:\n  - read\n  - \"glob\"\ndisallowedTools: [write, 'edit']\nmodel: \"xai/grok-4\"\nnot a field\n---\n";
    let a = parse(text, Path::new("/agents/doc-writer.md")).unwrap();
    assert_eq!(a.name, "doc-writer", "name falls back to the file stem");
    assert_eq!(a.description, "First line\nsecond line");
    assert_eq!(a.tools.unwrap(), ["read", "glob"]);
    assert_eq!(a.disallowed_tools, ["write", "edit"]);
    assert_eq!(a.model.as_deref(), Some("xai/grok-4"));
    assert_eq!(
        a.prompt,
        crate::agent::Agent::system_prompt_for("general"),
        "empty body uses the general prompt"
    );

    let folded = "---\ndescription: >\n  one\n  two\ntools:\n  read\nextra:\n  - a\n  b\n---\nbody";
    let a = parse(folded, Path::new("f.md")).unwrap();
    assert_eq!(a.description, "one two");
    assert_eq!(a.tools.unwrap(), ["read"]);
    assert_eq!(a.prompt, "body");

    let snake = "---\ndescription: d\ndisallowed_tools: shell\ntools: \"\"\n---\nb";
    let a = parse(snake, Path::new("s.md")).unwrap();
    assert_eq!(a.disallowed_tools, ["shell", "bash"]);
    assert!(a.tools.is_none(), "empty tools keeps the parent set");
    assert!(a.model.is_none());
}

#[test]
fn rejects_non_definitions() {
    let p = Path::new("w.md");
    assert!(parse("plain worker output", p).is_none(), "no frontmatter");
    assert!(parse("---\ndescription: d\n", p).is_none(), "unclosed");
    assert!(
        parse("---\nname: x\n---\nbody", p).is_none(),
        "no description"
    );
    assert!(parse("---\ndescription: \"\"\n---\n", p).is_none());
    assert!(
        parse("---\nname: bad name\ndescription: d\n---\n", p).is_none(),
        "invalid name"
    );
    assert!(
        parse("---\nname: explore\ndescription: d\n---\n", p).is_none(),
        "built-in names stay built-in"
    );
    assert!(parse("---\ndescription: d\n---\n", Path::new("")).is_none());
    let closed_at_eof = parse("---\ndescription: d\n---", p).unwrap();
    assert_eq!(closed_at_eof.name, "w");
}

#[test]
fn load_from_dirs_later_wins_and_skips_noise() {
    let tmp = tempfile::tempdir().unwrap();
    let low = tmp.path().join("low");
    let high = tmp.path().join("high");
    agent_file(&low, "a.md", "---\nname: a\ndescription: low a\n---\nlow");
    agent_file(&low, "b.md", "---\nname: b\ndescription: low b\n---\nb");
    agent_file(
        &high,
        "a.md",
        "---\nname: a\ndescription: high a\n---\nhigh",
    );
    agent_file(&high, "task-1.md", "worker output, not a definition");
    agent_file(&high, "notes.txt", "---\nname: c\ndescription: d\n---\n");
    std::fs::create_dir_all(high.join("dir.md")).unwrap();
    let missing = tmp.path().join("missing");

    let agents = load_from(&[low, missing, high]);
    let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
    assert_eq!(agents[0].description, "high a");
    assert_eq!(agents[0].prompt, "high");
}

#[test]
fn load_from_skips_unreadable_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("bad.md"), [0xff, 0xfe, 0x00]).unwrap();
    assert!(load_from(&[tmp.path().to_path_buf()]).is_empty());
}

#[test]
fn project_dirs_include_both_conventions() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    let d = dirs(project);
    assert!(d.contains(&project.join(".claude").join("agents")));
    assert_eq!(
        d.last().unwrap(),
        &whycodes_core::project_dir(project).join("agents")
    );
    agent_file(
        &project.join(".claude").join("agents"),
        "cc.md",
        "---\ndescription: from claude\n---\nx",
    );
    agent_file(
        &whycodes_core::project_dir(project).join("agents"),
        "wc.md",
        "---\ndescription: from whycodes\n---\ny",
    );
    let names: Vec<String> = load(project).into_iter().map(|a| a.name).collect();
    assert!(names.contains(&"cc".to_string()), "{names:?}");
    assert!(names.contains(&"wc".to_string()), "{names:?}");
}

#[test]
fn resolve_builtin_custom_and_unknown() {
    let agents = vec![sample(None)];
    assert_eq!(resolve("explore", &agents), Ok(None));
    assert_eq!(resolve("rev", &agents).unwrap().unwrap().name, "rev");
    let err = resolve("nope", &agents).unwrap_err();
    assert!(err.contains("`nope`"), "{err}");
    assert!(err.contains("general, explore, scout, rev"), "{err}");
}

#[test]
fn catalog_lists_agents_compactly() {
    assert_eq!(catalog_markdown(&[]), "");
    let mut a = sample(None);
    a.description = format!("Line one\n\n  line   two {}", "x".repeat(300));
    let cat = catalog_markdown(&[a]);
    assert!(cat.starts_with("# Custom agents"), "{cat}");
    assert!(cat.contains("- `rev`: Line one line two x"), "{cat}");
    let line = cat.lines().last().unwrap();
    assert!(line.len() < 220, "{line}");
}

#[test]
fn permission_only_narrows() {
    let parent = PermissionSet {
        allowed_tools: Some(vec!["read".into(), "grep".into()]),
        denied_tools: Some(vec!["swarm".into()]),
        ..Default::default()
    };
    let mut a = sample(None);
    a.tools = Some(vec!["read".into(), "shell".into()]);
    a.disallowed_tools = vec!["grep".into()];
    let perm = a.permission(&parent, &["swarm", "todo"]);
    assert_eq!(perm.allowed_tools.unwrap(), ["read"]);
    assert_eq!(perm.denied_tools.unwrap(), ["swarm", "grep", "todo"]);

    let open = PermissionSet::default();
    let perm = a.permission(&open, &[]);
    assert_eq!(perm.allowed_tools.unwrap(), ["read", "shell"]);
    let perm = sample(None).permission(&open, &["task"]);
    assert!(perm.allowed_tools.is_none());
    assert_eq!(perm.denied_tools.unwrap(), ["task"]);
}

#[test]
fn route_honours_model_field() {
    let worker = || ("p".to_string(), "small".to_string());
    assert_eq!(sample(None).route("p", "big", worker()), worker());
    assert_eq!(sample(Some(" ")).route("p", "big", worker()), worker());
    assert_eq!(sample(Some("sonnet")).route("p", "big", worker()), worker());
    assert_eq!(
        sample(Some("inherit")).route("p", "big", worker()),
        ("p".to_string(), "big".to_string())
    );
    assert_eq!(
        sample(Some("xai/grok-4")).route("p", "big", worker()),
        ("xai".to_string(), "grok-4".to_string())
    );
    assert_eq!(
        sample(Some("other-model")).route("p", "big", worker()),
        ("p".to_string(), "other-model".to_string())
    );
}

#[test]
fn prewalk_frontmatter_starts_strong_and_hands_off_to_the_worker() {
    let on = parse(
        "---
description: d
prewalk: True
---
body
",
        Path::new("pw.md"),
    )
    .unwrap();
    assert!(on.prewalk);
    let off = parse(
        "---
description: d
prewalk: no
---
",
        Path::new("x.md"),
    )
    .unwrap();
    assert!(!off.prewalk);
    assert!(!sample(None).prewalk, "off by default");

    let worker = || ("p".to_string(), "small".to_string());
    let big = || ("p".to_string(), "big".to_string());
    assert_eq!(
        sample(None).prewalk_route("p", "big", worker()),
        (worker(), None),
        "without prewalk the route is unchanged"
    );
    let mut agent = sample(None);
    agent.prewalk = true;
    assert_eq!(
        agent.prewalk_route("p", "big", worker()),
        (big(), Some(worker()))
    );
    agent.model = Some("xai/grok-4".into());
    assert_eq!(
        agent.prewalk_route("p", "big", worker()),
        (("xai".to_string(), "grok-4".to_string()), Some(worker()))
    );
    agent.model = None;
    assert_eq!(
        agent.prewalk_route("p", "big", big()),
        (big(), None),
        "no cheaper worker model: stay on the strong one"
    );
}

#[test]
fn list_valued_text_field_joins_its_items() {
    let text = "---\ndescription:\n  - checks tests\n  - checks docs\n---\nbody\n";
    let a = parse(text, Path::new("lister.md")).unwrap();
    assert_eq!(a.name, "lister");
    assert_eq!(a.description, "checks tests, checks docs");
}

#[test]
fn mcp_tool_names_use_the_bridge_wire_form() {
    // Plain names keep the `{server}_{tool}` shape.
    assert_eq!(map_tool("mcp__gh__search"), vec!["gh_search".to_string()]);
    // Dots, slashes and long names are sanitised exactly like the bridge
    // registers them, so the frontmatter entry matches a real tool.
    let long_tool = "a".repeat(80);
    for (server, tool) in [("my.server", "do/thing"), ("gh", long_tool.as_str())] {
        let expected = crate::mcp_load::bridge_tool_name(server, tool);
        assert_eq!(
            map_tool(&format!("mcp__{server}__{tool}")),
            vec![expected.clone()]
        );
        assert_ne!(expected, format!("{server}_{tool}"));
    }
}
