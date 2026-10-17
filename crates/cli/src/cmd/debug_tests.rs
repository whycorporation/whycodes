use super::*;
use crate::args::Cli;
use clap::Parser;

#[test]
fn auto_update_off_when_cli_flag() {
    let cli = Cli::try_parse_from(["whycodes", "--no-auto-update"]).unwrap();
    assert!(!should_auto_update_with_env(
        &cli, true, false, false, false
    ));
}

#[test]
fn debug_dump_keys_are_camel_case() {
    let dump = collect_debug();
    let v = serde_json::to_value(&dump).unwrap();
    for key in [
        "version",
        "gitHash",
        "configPath",
        "configExists",
        "dataDir",
        "jsonlLog",
        "crashDir",
        "debugLog",
        "cwd",
        "env",
        "oauth",
    ] {
        assert!(v.get(key).is_some(), "missing {key} in {v}");
    }
    assert!(v.get("git_hash").is_none());
    let env = v["env"].as_array().expect("env array");
    assert!(!env.is_empty());
    for entry in env {
        assert!(entry.get("name").is_some());
        assert!(entry.get("set").and_then(|s| s.as_bool()).is_some());
        assert!(entry.get("value").is_none(), "env must not leak values");
    }
}

#[test]
fn auto_update_on_for_interactive_run_and_off_otherwise() {
    let run = Cli::try_parse_from(["whycodes", "run"]).unwrap();
    assert!(should_auto_update_with_env(&run, true, false, false, false));
    let json = Cli::try_parse_from(["whycodes", "run", "--format", "json", "hi"]).unwrap();
    assert!(!should_auto_update_with_env(
        &json, true, false, false, false
    ));
    let stats = Cli::try_parse_from(["whycodes", "stats"]).unwrap();
    assert!(!should_auto_update_with_env(
        &stats, true, false, false, false
    ));
    assert!(!should_auto_update_with_env(
        &run, false, false, false, false
    ));
    assert!(!should_auto_update_with_env(&run, true, true, false, false));
    assert!(!should_auto_update_with_env(&run, true, false, true, false));
    assert!(!should_auto_update_with_env(&run, true, false, false, true));
    assert!(spawn_update_check_if(false).is_none());
    let dump = collect_debug();
    let _ = cmd_version("rustc");
    let _ = cmd_version("definitely-missing-bin");
    assert!(!dump.version.is_empty());
}

#[test]
fn debug_printer_helpers() {
    assert!(debug_header_line().contains("Debug"));
    assert!(debug_exists_mark(true).contains("✓") || debug_exists_mark(true).contains("green"));
    assert!(debug_exists_mark(false).contains("not found"));
    assert!(!debug_dir_exists_mark(false).contains("not found"));
    assert!(debug_config_line("/tmp/c.toml", true).contains("/tmp/c.toml"));
    assert!(debug_data_dir_line("/tmp/data", false).contains("/tmp/data"));
    assert!(debug_jsonl_line("/tmp/log.jsonl", true).contains("JSONL"));
    assert!(debug_jsonl_line("/tmp/log.jsonl", false).contains("/tmp/log.jsonl"));
    assert!(debug_crash_dir_line("/tmp/crash").contains("/tmp/crash"));
    assert!(debug_log_line("/tmp/latest.log").contains("WHYCODES_LOG_FILE"));
    assert!(debug_cwd_line("/tmp").contains("/tmp"));
    assert!(debug_home_line("/home/x").contains("/home/x"));
    assert!(debug_tool_line("Rust", "rustc 1").contains("rustc 1"));
    assert!(debug_env_set_line("FOO", "sk-xxxx").contains("set"));
    assert!(
        !debug_env_set_line("FOO", &mask_secret("sk-live-secret-value")).contains("sk-live-secret"),
        "masked debug lines must not contain the raw secret"
    );
    assert!(debug_env_unset_line("FOO").contains("not set"));
    assert!(debug_oauth_empty_line().contains("auth login"));
    assert!(debug_oauth_entry_line("acme", "oauth", "no expiry").contains("acme"));
    assert!(debug_oauth_store_error_line("denied").contains("denied"));
    assert!(after_tui_upgrade_skip_line().contains("latest"));
    assert!(after_tui_upgrade_ok_line("1.0", "1.1").contains("1.1"));
    assert!(after_tui_upgrade_failed_line("offline").contains("offline"));
}

#[test]
fn tool_catalog_is_sorted_and_marks_profiles_aliases_and_guides() {
    let catalog = collect_tool_catalog();
    let names: Vec<&str> = catalog.iter().map(|e| e.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        names, sorted,
        "catalog must be sorted by name without duplicates"
    );

    let by_name = |n: &str| catalog.iter().find(|e| e.name == n).expect(n);
    assert_eq!(by_name("bash").profile, "core");
    assert_eq!(by_name("swarm").profile, "deferred");
    assert_eq!(by_name("shell").alias_of.as_deref(), Some("bash"));
    assert_eq!(by_name("todo").alias_of.as_deref(), Some("todowrite"));
    assert!(by_name("bash").alias_of.is_none());

    let guide = by_name("bash").guide.as_deref().expect("bash guide");
    assert!(guide.starts_with("Execute a shell command"), "{guide}");
    assert!(
        !guide.contains("# agent://tools/"),
        "heading must be stripped"
    );
    assert!(by_name("swarm").guide.is_none());

    let v = serde_json::to_value(&catalog).unwrap();
    let first = &v[0];
    for key in [
        "name",
        "description",
        "parameters",
        "profile",
        "aliasOf",
        "guide",
    ] {
        assert!(first.get(key).is_some(), "missing {key}");
    }
    assert!(first.get("alias_of").is_none());
}

#[test]
fn tool_guide_body_drops_heading_and_skips_unknown_tools() {
    let ctx = whycodes_core::tool::ToolContext::new(".");
    let body = tool_guide_body("read", &ctx).expect("read guide");
    assert!(body.starts_with("Read a text file"), "{body}");
    assert!(tool_guide_body("no_such_tool", &ctx).is_none());
    assert!(
        tool_guide_body("", &ctx).is_some(),
        "index text has no heading"
    );
}

#[test]
fn tool_catalog_line_shows_profile_or_alias() {
    let entry = ToolCatalogEntry {
        name: "x".into(),
        description: "first line\nsecond".into(),
        parameters: serde_json::json!({}),
        profile: "core",
        alias_of: None,
        guide: None,
    };
    let line = tool_catalog_line(&entry);
    assert!(line.starts_with("x "), "{line}");
    assert!(line.contains(" core "), "{line}");
    assert!(line.ends_with("first line"), "{line}");

    let alias = ToolCatalogEntry {
        alias_of: Some("y".into()),
        description: String::new(),
        ..entry
    };
    assert!(tool_catalog_line(&alias).contains("alias of y"));
}

#[tokio::test]
async fn cmd_debug_tools_prints_text_and_json() {
    cmd_debug(false, true).await.unwrap();
    cmd_debug(true, true).await.unwrap();
}

/// `docs/tools/catalog.json` is the source for the generated tool pages.
/// Regenerate it after changing a tool's name, description, schema, profile
/// or long guide with a `--features ast-tools` build:
/// `whycodes debug --tools --json > docs/tools/catalog.json` then
/// `python scripts/gen_tool_docs.py`.
#[test]
fn committed_tool_catalog_matches_binary() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/tools/catalog.json");
    let mut committed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("docs/tools/catalog.json"))
            .expect("catalog json");
    // The committed catalog comes from a `--features ast-tools` build so the
    // `ast_grep` / `ast_edit` pages exist; a default build lacks those two.
    if cfg!(not(feature = "ast-tools")) {
        committed
            .as_array_mut()
            .expect("catalog array")
            .retain(|t| !t["name"].as_str().unwrap_or("").starts_with("ast_"));
    }
    let live = serde_json::to_value(collect_tool_catalog()).unwrap();
    assert_eq!(
        committed, live,
        "docs/tools/catalog.json is stale — run `whycodes debug --tools --json > docs/tools/catalog.json && python scripts/gen_tool_docs.py` with a `--features ast-tools` build"
    );
}
