use super::*;
use std::fs;
use std::io::Write;

#[test]
fn normalize_ext_strips_dot_and_case() {
    assert_eq!(normalize_ext(".Rs"), "rs");
    assert_eq!(normalize_ext("PY"), "py");
    assert_eq!(normalize_ext("tsx"), "tsx");
}

#[test]
fn file_uri_unix_and_drive_letters() {
    assert_eq!(file_uri_from_lossy("/tmp/a.rs"), "file:///tmp/a.rs");
    assert_eq!(
        file_uri_from_lossy("C:/foo/bar.rs"),
        "file:///C:/foo/bar.rs"
    );
    assert_eq!(
        file_uri_from_lossy(r"C:\foo\bar.rs"),
        "file:///C:/foo/bar.rs"
    );
    assert_eq!(
        file_uri_from_lossy("//server/share/x.rs"),
        "file://server/share/x.rs"
    );
    assert_eq!(
        file_uri_from_lossy("relative/x.rs"),
        "file:///relative/x.rs"
    );
}

#[test]
fn path_from_file_uri_roundtrips_unix_shape() {
    let p = path_from_file_uri("file:///tmp/a.rs").unwrap();
    if cfg!(windows) {
        assert!(p.to_string_lossy().contains("tmp"));
    } else {
        assert_eq!(p, PathBuf::from("/tmp/a.rs"));
    }
    let drive = path_from_file_uri("file:///C:/foo/bar.rs").unwrap();
    let s = drive.to_string_lossy();
    assert!(s.contains("foo"));
}

#[test]
fn empty_markers_always_match() {
    assert!(root_markers_match(Path::new("."), &[]));
}

#[test]
fn root_markers_are_cwd_only() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    let markers = vec!["Cargo.toml".into()];
    assert!(root_markers_match(dir.path(), &markers));
    assert!(!root_markers_match(&dir.path().join("src"), &markers));
}

#[test]
fn wildcard_markers_match_names_in_cwd() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("App.csproj"), "<Project/>").unwrap();
    assert!(root_markers_match(dir.path(), &["*.csproj".into()]));
    assert!(!root_markers_match(dir.path(), &["*.sln".into()]));
    assert!(wildcard_match("foo.cabal", "*.cabal"));
    assert!(!wildcard_match("cabal", "*.cabal"));
    assert!(wildcard_match("exact", "exact"));
}

#[test]
fn resolve_command_prefers_project_local_bin() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("node_modules").join(".bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let exe_name = if cfg!(windows) {
        "my-lsp.CMD"
    } else {
        "my-lsp"
    };
    let exe = bin_dir.join(exe_name);
    {
        let mut f = fs::File::create(&exe).unwrap();
        f.write_all(b"echo").unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let found = resolve_command(dir.path(), "my-lsp").unwrap();
    assert_eq!(found.file_stem().unwrap(), "my-lsp");
}

#[test]
fn resolve_command_accepts_absolute_and_relative() {
    let dir = tempfile::tempdir().unwrap();
    let abs = dir.path().join("tool.bin");
    fs::write(&abs, b"x").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&abs, fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert_eq!(
        resolve_command(dir.path(), abs.to_str().unwrap()).unwrap(),
        abs
    );
    fs::write(dir.path().join("rel.bin"), b"x").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            dir.path().join("rel.bin"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    assert!(resolve_command(dir.path(), "rel.bin").is_some());
    assert!(resolve_command(dir.path(), "whycodes-lsp-bin-that-does-not-exist").is_none());
}

#[test]
fn which_command_finds_something_on_path() {
    let found = which_command("sh")
        .or_else(|| which_command("cmd"))
        .or_else(|| which_command("python3"));
    assert!(found.is_some(), "expected sh, cmd, or python3 on PATH");
    assert!(which_command("whycodes-lsp-bin-that-does-not-exist").is_none());
}
