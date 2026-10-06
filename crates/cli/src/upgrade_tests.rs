use super::*;

#[test]
fn latest_url_reads_current_env() {
    let url = latest_release_url();
    assert!(url.starts_with("http"));
    assert!(url.contains("github.com") || url.contains("127.0.0.1") || url.contains("localhost"));
}

/// A brew-style link whose own path is not a Cellar path still counts when
/// the target (absolute or relative) lives under Cellar. A dangling link and
/// a link to a non-Cellar file do not.
#[cfg(unix)]
#[test]
fn looks_like_homebrew_follows_symlink_targets() {
    let dir = tempfile::tempdir().unwrap();
    let cellar = dir
        .path()
        .join("Cellar")
        .join("whycodes")
        .join("1")
        .join("bin");
    std::fs::create_dir_all(&cellar).unwrap();
    let real = cellar.join("whycodes");
    std::fs::write(&real, b"bin").unwrap();
    let bindir = dir.path().join("bin");
    std::fs::create_dir_all(&bindir).unwrap();

    let absolute = bindir.join("whycodes-abs");
    std::os::unix::fs::symlink(&real, &absolute).unwrap();
    assert!(looks_like_homebrew(&absolute));

    let relative = bindir.join("whycodes-rel");
    std::os::unix::fs::symlink("../Cellar/whycodes/1/bin/whycodes", &relative).unwrap();
    assert!(looks_like_homebrew(&relative));

    let other = dir.path().join("plain");
    std::fs::write(&other, b"x").unwrap();
    let plain = bindir.join("whycodes-plain");
    std::os::unix::fs::symlink(&other, &plain).unwrap();
    assert!(!looks_like_homebrew(&plain));

    let dangling = bindir.join("whycodes-gone");
    std::os::unix::fs::symlink(bindir.join("missing"), &dangling).unwrap();
    assert!(!looks_like_homebrew(&dangling));
}

#[cfg(windows)]
#[test]
fn looks_like_homebrew_follows_symlink_targets() {
    let dir = tempfile::tempdir().unwrap();
    let cellar = dir
        .path()
        .join("Cellar")
        .join("whycodes")
        .join("1")
        .join("bin");
    std::fs::create_dir_all(&cellar).unwrap();
    let real = cellar.join("whycodes");
    std::fs::write(&real, b"bin").unwrap();
    let bindir = dir.path().join("bin");
    std::fs::create_dir_all(&bindir).unwrap();

    let absolute = bindir.join("whycodes-abs");
    if std::os::windows::fs::symlink_file(&real, &absolute).is_err() {
        // Creating a symlink needs Developer Mode or elevation.
        return;
    }
    assert!(looks_like_homebrew(&absolute));

    let relative = bindir.join("whycodes-rel");
    std::os::windows::fs::symlink_file(r"..\Cellar\whycodes\1\bin\whycodes", &relative).unwrap();
    assert!(looks_like_homebrew(&relative));

    let other = dir.path().join("plain");
    std::fs::write(&other, b"x").unwrap();
    let plain = bindir.join("whycodes-plain");
    std::os::windows::fs::symlink_file(&other, &plain).unwrap();
    assert!(!looks_like_homebrew(&plain));

    let dangling = bindir.join("whycodes-gone");
    std::os::windows::fs::symlink_file(bindir.join("missing"), &dangling).unwrap();
    assert!(!looks_like_homebrew(&dangling));
}

/// An empty `WHYCODES_UPGRADE_TARGET` is ignored; the running executable is
/// used instead.
#[test]
fn current_binary_ignores_empty_override() {
    let prev = std::env::var_os("WHYCODES_UPGRADE_TARGET");
    unsafe { std::env::set_var("WHYCODES_UPGRADE_TARGET", "") };
    let resolved = current_binary().unwrap();
    if let Some(v) = prev {
        unsafe { std::env::set_var("WHYCODES_UPGRADE_TARGET", v) };
    } else {
        unsafe { std::env::remove_var("WHYCODES_UPGRADE_TARGET") };
    }
    assert!(!resolved.as_os_str().is_empty());
}
