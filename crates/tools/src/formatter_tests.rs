use super::*;

fn touch(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn ok_result() -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: "Edited".into(),
        is_error: false,
    }
}

#[test]
fn rust_needs_a_rustfmt_config_and_takes_the_cargo_edition() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let file = root.join("crates/a/src/lib.rs");
    touch(&file, "");
    assert_eq!(detect(&file, root), None);

    touch(&root.join("rustfmt.toml"), "max_width = 100\n");
    touch(
        &root.join("Cargo.toml"),
        "[workspace.package]\nedition = \"2024\"\n",
    );
    touch(
        &root.join("crates/a/Cargo.toml"),
        "[package]\nedition.workspace = true\n",
    );
    let f = detect(&file, root).unwrap();
    assert_eq!(f.name, "rustfmt");
    assert_eq!(f.program, PathBuf::from("rustfmt"));
    assert_eq!(f.args, ["--quiet", "--edition", "2024"]);
    assert_eq!(f.dir, root.join("crates/a/src"));

    std::fs::remove_file(root.join("Cargo.toml")).unwrap();
    assert_eq!(detect(&file, root).unwrap().args, ["--quiet"]);
}

#[test]
fn markers_outside_the_project_root_do_not_count() {
    let dir = tempfile::tempdir().unwrap();
    touch(&dir.path().join("rustfmt.toml"), "");
    let root = dir.path().join("proj");
    let file = root.join("main.rs");
    touch(&file, "");
    assert_eq!(detect(&file, &root), None);
    assert_eq!(detect(&dir.path().join("x.rs"), &root), None);
}

#[test]
fn go_needs_a_go_mod() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.go");
    assert_eq!(detect(&file, dir.path()), None);
    touch(&dir.path().join("go.mod"), "module x\n");
    let f = detect(&file, dir.path()).unwrap();
    assert_eq!((f.name, f.args.len()), ("gofmt", 0));
}

#[test]
fn python_uses_ruff_or_black_from_project_config() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let file = root.join("pkg/mod.py");
    assert_eq!(detect(&file, root), None);

    touch(
        &root.join("pyproject.toml"),
        "[tool.black]\nline-length = 88\n",
    );
    let f = detect(&file, root).unwrap();
    assert_eq!(f.name, "black");
    assert_eq!(f.args[0], "--quiet");
    assert_eq!(f.args.last().map(String::as_str), Some("-"));

    touch(&root.join("pyproject.toml"), "[tool.ruff]\n");
    let f = detect(&root.join("pkg/stub.pyi"), root).unwrap();
    assert_eq!(f.name, "ruff");
    assert_eq!(&f.args[..2], ["format", "--quiet"]);

    touch(&root.join("pyproject.toml"), "[project]\n");
    touch(&root.join("ruff.toml"), "");
    assert_eq!(detect(&file, root).unwrap().name, "ruff");
}

#[test]
fn prettier_needs_config_and_a_local_install() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let file = root.join("web/src/app.tsx");
    assert_eq!(detect(&file, root), None);

    touch(
        &root.join("package.json"),
        "{\"devDependencies\": {\"prettier\": \"3\"}}",
    );
    assert_eq!(detect(&file, root), None, "no local binary");

    let bin = root.join("node_modules/.bin").join(PRETTIER_BIN);
    touch(&bin, "");
    let f = detect(&file, root).unwrap();
    assert_eq!(f.name, "prettier");
    assert_eq!(f.program, bin);
    assert_eq!(f.args[0], "--stdin-filepath");

    touch(&root.join("package.json"), "{}");
    assert_eq!(detect(&file, root), None);
    touch(&root.join(".prettierrc"), "{}");
    assert!(detect(&root.join("README.md"), root).is_some());
}

#[test]
fn other_files_have_no_formatter() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(detect(&dir.path().join("notes.txt"), dir.path()), None);
    assert_eq!(detect(&dir.path().join("Makefile"), dir.path()), None);
}

/// Run `f` while holding `ENV_LOCK`: sibling tests empty `PATH` under it, and
/// these spawn `rustfmt` from `PATH`.
fn with_env_lock<F: std::future::Future>(f: F) -> F::Output {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

fn rustfmt_in(dir: &Path) -> Formatter {
    Formatter {
        name: "rustfmt",
        program: "rustfmt".into(),
        args: vec!["--quiet".into(), "--edition".into(), "2024".into()],
        dir: dir.to_path_buf(),
    }
}

#[test]
fn run_formats_stdin_and_fails_soft() {
    with_env_lock(run_formats_stdin_and_fails_soft_body());
}

async fn run_formats_stdin_and_fails_soft_body() {
    let dir = tempfile::tempdir().unwrap();
    let f = rustfmt_in(dir.path());
    assert_eq!(
        run(&f, "fn  main( ){let x=1;}\n").await.as_deref(),
        Some("fn main() {\n    let x = 1;\n}\n")
    );
    assert_eq!(run(&f, "fn main( {").await, None, "syntax error");
    let missing = Formatter {
        program: dir.path().join("no-such-formatter"),
        ..rustfmt_in(dir.path())
    };
    assert_eq!(run(&missing, "fn main() {}").await, None);
}

#[test]
fn written_file_is_formatted_and_the_result_says_so() {
    with_env_lock(written_file_is_formatted_and_the_result_says_so_body());
}

async fn written_file_is_formatted_and_the_result_says_so_body() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path().to_string_lossy().into_owned();
    touch(&dir.path().join("rustfmt.toml"), "edition = \"2024\"\n");
    let file = dir.path().join("src/lib.rs");
    touch(&file, "pub fn  f( )->u8{1}\n");

    let mut out = ok_result();
    format_written_file(Some("src/lib.rs"), &wd, &mut out).await;
    assert!(
        out.content.contains("Formatted with rustfmt"),
        "{}",
        out.content
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "pub fn f() -> u8 {\n    1\n}\n"
    );

    // Already formatted: nothing to say.
    let mut out = ok_result();
    format_written_file(Some("src/lib.rs"), &wd, &mut out).await;
    assert_eq!(out.content, "Edited");
}

#[test]
fn nothing_happens_when_formatting_cannot_apply() {
    with_env_lock(nothing_happens_when_formatting_cannot_apply_body());
}

async fn nothing_happens_when_formatting_cannot_apply_body() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path().to_string_lossy().into_owned();
    touch(&dir.path().join("rustfmt.toml"), "");
    touch(&dir.path().join("bad.rs"), "fn main( {\n");
    touch(&dir.path().join("blank.rs"), "\n\n\n");
    std::fs::create_dir_all(dir.path().join("dir.rs")).unwrap();

    let failed = ToolResult {
        is_error: true,
        ..ok_result()
    };
    for (path, start) in [
        (None, ok_result()),
        (Some(""), ok_result()),
        (Some("bad.rs"), failed.clone()),
        (Some("notes.txt"), ok_result()),
        (Some("missing.rs"), ok_result()),
        (Some("dir.rs"), ok_result()),
        (Some("bad.rs"), ok_result()),
        (Some("blank.rs"), ok_result()),
    ] {
        let mut out = start.clone();
        format_written_file(path, &wd, &mut out).await;
        assert_eq!(out.content, start.content, "{path:?}");
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("blank.rs")).unwrap(),
        "\n\n\n"
    );
}

#[cfg(unix)]
#[test]
fn unwritable_directory_leaves_the_file_alone() {
    with_env_lock(unwritable_directory_leaves_the_file_alone_body());
}

#[cfg(unix)]
async fn unwritable_directory_leaves_the_file_alone_body() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path().to_string_lossy().into_owned();
    touch(&dir.path().join("rustfmt.toml"), "");
    let ro = dir.path().join("ro");
    touch(&ro.join("a.rs"), "fn  a(){}\n");
    let mut perms = std::fs::metadata(&ro).unwrap().permissions();
    perms.set_mode(0o555);
    std::fs::set_permissions(&ro, perms.clone()).unwrap();
    let mut out = ok_result();
    format_written_file(Some("ro/a.rs"), &wd, &mut out).await;
    perms.set_mode(0o755);
    std::fs::set_permissions(&ro, perms).unwrap();
    assert_eq!(out.content, "Edited");
    assert_eq!(
        std::fs::read_to_string(ro.join("a.rs")).unwrap(),
        "fn  a(){}\n"
    );
}
