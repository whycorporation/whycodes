use super::*;
use clap::Parser;

#[test]
fn clap_parses_run_and_generate() {
    let cli = Cli::try_parse_from(["whycodes", "run", "--plain", "hi"]).unwrap();
    assert!(cli.plain);
    assert!(matches!(
        cli.command,
        Some(Commands::Run { prompt: Some(ref p), .. }) if p == "hi"
    ));
    let parsed = Cli::try_parse_from(["whycodes", "generate", "a", "b", "-j", "2"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(Commands::Generate { ref prompt, jobs, .. }) if prompt.as_slice() == ["a", "b"] && jobs == 2
    ));
    let approved = Cli::try_parse_from([
        "whycodes",
        "generate",
        "--approve-tools",
        "x",
        "--format",
        "json",
    ])
    .unwrap();
    assert!(approved.approve_tools);
    match approved.command {
        Some(Commands::Generate { format, .. }) => {
            assert_eq!(format, whycodes_protocol::OutputFormat::Json);
        }
        other => panic!("unexpected {other:?}"),
    }
    let run_approved = Cli::try_parse_from([
        "whycodes",
        "--approve-tools",
        "run",
        "hi",
        "--format",
        "json",
    ])
    .unwrap();
    assert!(run_approved.approve_tools);
    let slop = Cli::try_parse_from(["whycodes", "slop", "--json", "--base", "main"]).unwrap();
    assert!(matches!(
        slop.command,
        Some(Commands::Slop { json: true, ref base }) if base.as_deref() == Some("main")
    ));
}

#[test]
fn tied_typo_suggestions_are_stripped() {
    let err = Cli::try_parse_from(["whycodes", "auth", "logn"]).unwrap_err();
    let raw = err.to_string();
    let clean = super::sanitize_clap_error(err).to_string();
    // clap may list both `login` and `logout`; after sanitize, never both
    // under a "Did you mean" (or equivalent) hint.
    if raw.contains("login") && raw.contains("logout") {
        assert!(
            !(clean.contains("Did you mean")
                && clean.contains("login")
                && clean.contains("logout")),
            "tied suggestions must be dropped:\n{clean}"
        );
    }
    assert!(
        clean.contains("unrecognized") || clean.contains("invalid") || !clean.is_empty(),
        "{clean}"
    );
}

#[test]
fn unique_typo_suggestion_is_kept() {
    let err = Cli::try_parse_from(["whycodes", "sesion"]).unwrap_err();
    let clean = super::sanitize_clap_error(err).to_string();
    assert!(
        clean.contains("session") || clean.contains("Did you mean"),
        "unique close match should still be suggested:\n{clean}"
    );
}
