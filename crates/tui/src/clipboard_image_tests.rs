use super::*;

#[test]
fn sniff_png_jpeg_gif_webp_bmp_tiff_ico() {
    assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\nxxxx").unwrap().ext, "png");
    assert_eq!(sniff_image(b"\xff\xd8\xff\xe0rest").unwrap().ext, "jpg");
    assert_eq!(sniff_image(b"GIF89a....").unwrap().ext, "gif");
    let mut webp = b"RIFF".to_vec();
    webp.extend_from_slice(&[0, 0, 0, 0]);
    webp.extend_from_slice(b"WEBP");
    assert_eq!(sniff_image(&webp).unwrap().media_type, "image/webp");
    assert_eq!(sniff_image(b"BM6\0\0\0").unwrap().ext, "bmp");
    assert_eq!(sniff_image(b"II*\0....").unwrap().ext, "tiff");
    assert_eq!(sniff_image(&[0, 0, 1, 0, 1, 0]).unwrap().ext, "ico");
    assert!(sniff_image(b"hello").is_none());
    assert!(sniff_image(b"").is_none());
}

#[test]
fn stash_writes_sniffed_extension() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = b"\x89PNG\r\n\x1a\nhello-png";
    let path = stash_image_bytes_in(bytes, dir.path()).unwrap();
    assert_eq!(path.extension().and_then(|e| e.to_str()), Some("png"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn stash_rejects_unknown_and_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(stash_image_bytes_in(b"", dir.path()).is_err());
    assert!(stash_image_bytes_in(b"not-an-image", dir.path()).is_err());
}

#[test]
fn stash_rejects_oversize() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize((MAX_IMAGE_BYTES as usize) + 1, 0);
    let err = stash_image_bytes_in(&bytes, dir.path()).unwrap_err();
    assert!(err.contains("too large"), "{err}");
}

#[test]
fn parse_uri_list_keeps_existing_images() {
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("shot.png");
    std::fs::write(&img, b"\x89PNG").unwrap();
    let txt = dir.path().join("notes.txt");
    std::fs::write(&txt, b"hi").unwrap();
    let list = format!(
        "# comment\nfile://{}\n{}\nhttp://example.test/x.png\n",
        img.display(),
        txt.display()
    );
    let paths = parse_uri_list(&list);
    assert_eq!(paths.len(), 1, "{paths:?}");
    assert_eq!(
        paths[0].file_name().and_then(|n| n.to_str()),
        Some("shot.png")
    );
}

#[test]
fn first_image_mime_prefers_png() {
    let types = "text/plain\nimage/jpeg\nimage/png\n";
    assert_eq!(first_image_mime(types.lines()), Some("image/png"));
    assert_eq!(first_image_mime(["text/plain"].into_iter()), None);
}

#[test]
fn missing_binary_is_not_found() {
    match command_stdout("whycodes-no-such-clipboard-bin", &[], TIMEOUT) {
        Err(RunErr::NotFound) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn stub_roundtrip() {
    let got = with_stub(Ok(PromptClipboard::Empty), read_for_prompt).unwrap();
    assert_eq!(got, PromptClipboard::Empty);
    let got = with_stub(Err("boom".into()), read_for_prompt).unwrap_err();
    assert_eq!(got, "boom");
}

#[test]
fn bytes_to_prompt_covers_empty_unknown_and_errors() {
    assert!(matches!(
        bytes_to_prompt(Ok(Vec::new())).unwrap(),
        PromptClipboard::Empty
    ));
    assert!(matches!(
        bytes_to_prompt(Ok(b"not-an-image".to_vec())).unwrap(),
        PromptClipboard::Empty
    ));
    assert!(matches!(
        bytes_to_prompt(Err(RunErr::NotFound)).unwrap(),
        PromptClipboard::Empty
    ));
    assert!(matches!(
        bytes_to_prompt(Err(RunErr::Exit)).unwrap(),
        PromptClipboard::Empty
    ));
    assert!(matches!(
        bytes_to_prompt(Err(RunErr::Timeout)).unwrap(),
        PromptClipboard::Empty
    ));
    let err = bytes_to_prompt(Err(RunErr::TooLarge)).unwrap_err();
    assert!(err.contains("too large"), "{err}");
    let err = bytes_to_prompt(Err(RunErr::Io("boom".into()))).unwrap_err();
    assert_eq!(err, "boom");
}

#[test]
fn send_run_drops_when_receiver_is_gone() {
    let (tx, rx) = std::sync::mpsc::channel();
    drop(rx);
    send_run(tx, Ok(b"png".to_vec()));
}

#[test]
fn command_stdout_not_found_and_timeout() {
    match command_stdout("whycodes-no-such-clipboard-bin", &[], TIMEOUT) {
        Err(RunErr::NotFound) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
    #[cfg(windows)]
    let hanging = command_stdout(
        "cmd",
        &["/C", "ping -n 30 127.0.0.1 > NUL"],
        Duration::from_millis(1),
    );
    #[cfg(not(windows))]
    let hanging = command_stdout("sleep", &["30"], Duration::from_millis(1));
    match hanging {
        Err(RunErr::Timeout) | Err(RunErr::Exit) | Err(RunErr::Io(_)) => {}
        other => panic!("expected timeout/exit, got {other:?}"),
    }
}

#[test]
fn command_status_and_cleanup_temp() {
    #[cfg(windows)]
    let ok = command_status("cmd", &["/C", "exit 0"], TIMEOUT);
    #[cfg(not(windows))]
    let ok = command_status("true", &[], TIMEOUT);
    assert!(ok.is_ok(), "{ok:?}");
    match command_status("whycodes-no-such-clipboard-bin", &[], TIMEOUT) {
        Err(RunErr::NotFound) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
    let path = std::env::temp_dir().join("whycodes-clip-missing.png");
    cleanup_temp(&path);
}

#[cfg(target_os = "windows")]
#[test]
fn read_windows_image_empty_clipboard_is_empty() {
    // Live PowerShell path: empty clipboard → Empty, not a panic.
    match read_os_image() {
        Ok(PromptClipboard::Empty | PromptClipboard::ImagePaths(_)) => {}
        Err(e) => {
            assert!(
                e.contains("PowerShell") || e.contains("too large") || e.contains("read clipboard"),
                "{e}"
            );
        }
        #[cfg(test)]
        Ok(PromptClipboard::Text(_)) => panic!("production OS path must not return Text"),
    }
}

#[test]
fn stash_clipboard_image_writes_under_data_dir() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("WHYCODES_HOME");
    unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
    let bytes = b"\x89PNG\r\n\x1a\nhello-png";
    let path = stash_clipboard_image(bytes).unwrap();
    assert!(path.exists());
    assert_eq!(path.extension().and_then(|e| e.to_str()), Some("png"));
    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_HOME", v) },
        None => unsafe { std::env::remove_var("WHYCODES_HOME") },
    }
}

fn set_mtime_old(path: &std::path::Path) {
    #[cfg(windows)]
    {
        let p = path.to_string_lossy().replace('\'', "''");
        let status = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Item -LiteralPath '{p}').LastWriteTime = (Get-Date).AddDays(-2)"),
            ])
            .status()
            .expect("powershell mtime");
        assert!(status.success(), "set LastWriteTime failed: {status}");
    }
    #[cfg(not(windows))]
    {
        let status = std::process::Command::new("touch")
            .arg("-d")
            .arg("2 days ago")
            .arg(path)
            .status()
            .expect("touch mtime");
        assert!(status.success(), "touch -d failed: {status}");
    }
}

#[test]
fn prune_old_clipboard_images_skips_missing_and_keeps_fresh() {
    prune_old_clipboard_images(std::path::Path::new(
        "C:/dev/whycodes/target/no-such-clipboard-dir",
    ));
    let dir = tempfile::tempdir().unwrap();
    let fresh = dir.path().join("fresh.png");
    std::fs::write(&fresh, b"\x89PNG\r\n\x1a\n").unwrap();
    prune_old_clipboard_images(dir.path());
    assert!(fresh.exists());
}

#[test]
fn prune_old_clipboard_images_removes_stale_and_skips_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let stale = dir.path().join("stale.png");
    std::fs::write(&stale, b"\x89PNG\r\n\x1a\nold").unwrap();
    set_mtime_old(&stale);
    let nested = dir.path().join("nested-dir");
    std::fs::create_dir(&nested).unwrap();
    set_mtime_old(&nested);
    let fresh = dir.path().join("fresh.png");
    std::fs::write(&fresh, b"\x89PNG\r\n\x1a\n").unwrap();
    prune_old_clipboard_images(dir.path());
    assert!(!stale.exists(), "files older than 24h must be pruned");
    assert!(fresh.exists(), "fresh clipboard images must stay");
    assert!(
        nested.exists(),
        "directories must survive remove_file failure"
    );
    prune_clipboard_dir_entry(
        Err(std::io::Error::other("bad dirent")),
        std::time::SystemTime::now(),
    );
    prune_clipboard_path(
        std::path::Path::new("C:/dev/whycodes/target/no-such-prune.png"),
        Err(std::io::Error::other("no meta")),
        std::time::SystemTime::now(),
    );
    let missing = dir.path().join("no-mtime.png");
    prune_clipboard_path(
        &missing,
        Ok(std::fs::metadata(&fresh).unwrap()),
        std::time::UNIX_EPOCH,
    );
    prune_clipboard_path(
        &nested,
        Ok(std::fs::metadata(&nested).unwrap()),
        std::time::UNIX_EPOCH,
    );
}

#[test]
fn sniff_covers_remaining_headers() {
    assert_eq!(sniff_image(b"GIF87a....").unwrap().ext, "gif");
    assert_eq!(sniff_image(b"MM\0*....").unwrap().ext, "tiff");
    assert!(sniff_image(b"RIFF????XXXX").is_none());
    assert_eq!(
        first_image_mime(["image/jpeg"].into_iter()),
        Some("image/jpeg")
    );
    note_clipboard_dir_lock(&std::io::Error::other("chmod"));
    assert!(clipboard_dir_mode(std::path::Path::new(".")).is_ok());
}

#[test]
fn bytes_to_prompt_covers_empty_invalid_and_errors() {
    assert!(matches!(
        bytes_to_prompt(Ok(Vec::new())).unwrap(),
        PromptClipboard::Empty
    ));
    assert!(matches!(
        bytes_to_prompt(Ok(b"not-an-image".to_vec())).unwrap(),
        PromptClipboard::Empty
    ));
    assert!(matches!(
        bytes_to_prompt(Err(RunErr::NotFound)).unwrap(),
        PromptClipboard::Empty
    ));
    assert!(
        bytes_to_prompt(Err(RunErr::TooLarge))
            .unwrap_err()
            .contains("too large")
    );
    assert!(
        bytes_to_prompt(Err(RunErr::Io("disk".into())))
            .unwrap_err()
            .contains("disk")
    );
}

#[test]
fn classify_stdout_too_large_ok_and_exit() {
    let too = vec![0u8; (MAX_IMAGE_BYTES as usize) + 1];
    match classify_stdout(too, true) {
        Err(RunErr::TooLarge) => {}
        other => panic!("expected TooLarge, got {other:?}"),
    }
    let png = b"\x89PNG\r\n\x1a\n".to_vec();
    match classify_stdout(png.clone(), true) {
        Ok(out) => assert_eq!(out, png),
        other => panic!("expected Ok, got {other:?}"),
    }
    match classify_stdout(png, false) {
        Err(RunErr::Exit) => {}
        other => panic!("expected Exit, got {other:?}"),
    }
}

#[test]
fn classify_spawn_err_not_found_and_io() {
    match classify_spawn_err(io::Error::new(io::ErrorKind::NotFound, "gone")) {
        RunErr::NotFound => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
    match classify_spawn_err(io::Error::other("disk")) {
        RunErr::Io(msg) => assert!(msg.contains("disk"), "{msg}"),
        other => panic!("expected Io, got {other:?}"),
    }
}

#[test]
fn classify_command_output_maps_spawn_and_stdout() {
    match classify_command_output(Err(io::Error::new(io::ErrorKind::NotFound, "x"))) {
        Err(RunErr::NotFound) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
    match classify_command_output(Err(io::Error::other("pipe"))) {
        Err(RunErr::Io(msg)) => assert!(msg.contains("pipe"), "{msg}"),
        other => panic!("expected Io, got {other:?}"),
    }
}

#[test]
fn finish_windows_clipboard_covers_temp_read_and_run_errs() {
    let missing = std::env::temp_dir().join("whycodes-clip-no-such-file.png");
    let _ = std::fs::remove_file(&missing);
    let err = finish_windows_saved_image(&missing).unwrap_err();
    assert!(err.contains("read clipboard temp"), "{err}");

    let dest = tempfile::NamedTempFile::new().unwrap();
    let path = dest.path().to_path_buf();
    std::fs::write(&path, b"\x89PNG\r\n\x1a\nhello").unwrap();
    match finish_windows_saved_image(&path) {
        Ok(PromptClipboard::ImagePaths(p)) => assert_eq!(p.len(), 1),
        other => panic!("expected ImagePaths, got {other:?}"),
    }

    assert!(matches!(
        windows_clipboard_run_err(&missing, RunErr::Exit),
        Ok(PromptClipboard::Empty)
    ));
    assert!(matches!(
        windows_clipboard_run_err(&missing, RunErr::Timeout),
        Ok(PromptClipboard::Empty)
    ));
    let err = windows_clipboard_run_err(&missing, RunErr::NotFound).unwrap_err();
    assert!(err.contains("PowerShell"), "{err}");
    let err = windows_clipboard_run_err(&missing, RunErr::TooLarge).unwrap_err();
    assert!(err.contains("too large"), "{err}");
    let err = windows_clipboard_run_err(&missing, RunErr::Io("disk".into())).unwrap_err();
    assert_eq!(err, "disk");

    let dest = tempfile::NamedTempFile::new().unwrap();
    let path = dest.path().to_path_buf();
    match finish_windows_clipboard(path.clone(), Err(RunErr::Exit)) {
        Ok(PromptClipboard::Empty) => {}
        other => panic!("expected Empty, got {other:?}"),
    }

    let dest = tempfile::NamedTempFile::new().unwrap();
    let ok_path = dest.path().to_path_buf();
    std::fs::write(&ok_path, b"\x89PNG\r\n\x1a\nhello").unwrap();
    match finish_windows_clipboard(ok_path, Ok(())) {
        Ok(PromptClipboard::ImagePaths(p)) => assert_eq!(p.len(), 1),
        other => panic!("expected ImagePaths, got {other:?}"),
    }

    let dir = tempfile::tempdir().unwrap();
    cleanup_temp(dir.path());
    assert!(dir.path().exists(), "directory must survive remove_file");
}

#[test]
fn windows_clipboard_script_embeds_the_dest_path() {
    let script = windows_clipboard_script(r"C:\tmp\clip.png");
    assert!(script.contains("System.Windows.Forms"));
    assert!(script.contains("GetImage()"));
    assert!(script.contains(r"C:\tmp\clip.png"));
    assert!(script.contains("ImageFormat]::Png"));

    let empty = with_windows_run(Err(RunErr::Exit), read_windows_image).unwrap();
    assert!(matches!(empty, PromptClipboard::Empty));
    let missing = with_windows_run(Err(RunErr::NotFound), read_windows_image).unwrap_err();
    assert!(missing.contains("PowerShell"), "{missing}");
    assert!(matches!(host_clipboard_os(), HostClipboard::Windows));
    assert!(matches!(host_from_name("macos"), HostClipboard::Macos));
    assert!(matches!(host_from_name("linux"), HostClipboard::Linux));
    assert!(matches!(host_from_name("windows"), HostClipboard::Windows));
}

#[test]
fn command_stdout_echo_and_missing_bin() {
    #[cfg(windows)]
    let out = command_stdout("cmd", &["/C", "echo hi"], TIMEOUT).expect("echo");
    #[cfg(not(windows))]
    let out = command_stdout("echo", &["hi"], TIMEOUT).expect("echo");
    assert!(!out.is_empty());
    match command_stdout("whycodes-no-such-clipboard-bin", &[], TIMEOUT) {
        Err(RunErr::NotFound) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}

fn png_bytes() -> Vec<u8> {
    b"\x89PNG\r\n\x1a\nhello-png".to_vec()
}

#[test]
fn linux_clipboard_covers_wayland_xclip_and_errors() {
    let png = png_bytes();
    let png_hit = png.clone();
    let got = with_cmd_stub(
        move |bin, args| {
            if bin == "wl-paste" && args == ["--list-types"] {
                return Ok(b"text/plain\nimage/png\n".to_vec());
            }
            if bin == "wl-paste" && args.len() == 2 && args[1] == "image/png" {
                return Ok(png_hit.clone());
            }
            Err(RunErr::NotFound)
        },
        read_linux_image,
    )
    .unwrap();
    assert!(matches!(got, PromptClipboard::ImagePaths(p) if p.len() == 1));

    let empty = with_cmd_stub(
        |bin, args| {
            if bin == "wl-paste" && args == ["--list-types"] {
                return Ok(b"text/plain\n".to_vec());
            }
            Err(RunErr::NotFound)
        },
        read_linux_image,
    )
    .unwrap();
    assert!(matches!(empty, PromptClipboard::Empty));

    let too_large = with_cmd_stub(|_, _| Err(RunErr::TooLarge), read_linux_image).unwrap_err();
    assert!(too_large.contains("too large"), "{too_large}");

    let io_err = with_cmd_stub(
        |_, _| Err(RunErr::Io("wayland down".into())),
        read_linux_image,
    )
    .unwrap_err();
    assert!(io_err.contains("wayland"), "{io_err}");

    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("shot.png");
    std::fs::write(&img, b"\x89PNG\r\n\x1a\n").unwrap();
    let list = format!("{}\n", img.display());
    let list_hit = list.clone();
    let uris = with_cmd_stub(
        move |bin, args| {
            if bin == "wl-paste" && args == ["--list-types"] {
                return Ok(b"text/uri-list\n".to_vec());
            }
            if bin == "wl-paste" && args.len() == 2 && args[1] == "text/uri-list" {
                return Ok(list_hit.clone().into_bytes());
            }
            Err(RunErr::NotFound)
        },
        read_linux_image,
    )
    .unwrap();
    assert!(matches!(uris, PromptClipboard::ImagePaths(p) if !p.is_empty()));

    let uri_fail = with_cmd_stub(
        |bin, args| {
            if bin == "wl-paste" && args == ["--list-types"] {
                return Ok(b"text/uri-list\n".to_vec());
            }
            if args.len() == 2 && args[1] == "text/uri-list" {
                return Err(RunErr::Io("uri read".into()));
            }
            Err(RunErr::NotFound)
        },
        read_linux_image,
    )
    .unwrap_err();
    assert!(uri_fail.contains("uri read"), "{uri_fail}");

    let jpeg = png.clone();
    let xclip = with_cmd_stub(
        move |bin, args| {
            if bin == "wl-paste" {
                return Err(RunErr::NotFound);
            }
            if bin == "xclip" && args.contains(&"TARGETS") {
                return Ok(b"image/jpeg text/plain\n".to_vec());
            }
            if bin == "xclip" && args.contains(&"image/jpeg") {
                return Ok(jpeg.clone());
            }
            Err(RunErr::Exit)
        },
        read_linux_image,
    )
    .unwrap();
    assert!(matches!(xclip, PromptClipboard::ImagePaths(p) if p.len() == 1));

    let xclip_uri = list;
    let xclip_paths = with_cmd_stub(
        move |bin, args| {
            if bin == "wl-paste" {
                return Err(RunErr::Timeout);
            }
            if bin == "xclip" && args.contains(&"TARGETS") {
                return Ok(b"TEXT text/uri-list\n".to_vec());
            }
            if bin == "xclip" && args.contains(&"text/uri-list") {
                return Ok(xclip_uri.clone().into_bytes());
            }
            Err(RunErr::Exit)
        },
        read_linux_image,
    )
    .unwrap();
    assert!(matches!(xclip_paths, PromptClipboard::ImagePaths(p) if !p.is_empty()));

    let xclip_io = with_cmd_stub(
        |bin, args| {
            if bin == "wl-paste" {
                return Err(RunErr::Exit);
            }
            if bin == "xclip" && args.contains(&"TARGETS") {
                return Err(RunErr::Io("x11".into()));
            }
            Err(RunErr::NotFound)
        },
        read_linux_image,
    )
    .unwrap_err();
    assert!(xclip_io.contains("x11"), "{xclip_io}");

    let xclip_empty = with_cmd_stub(
        |bin, args| {
            if bin == "wl-paste" {
                return Err(RunErr::NotFound);
            }
            if bin == "xclip" && args.contains(&"TARGETS") {
                return Ok(b"STRING\n".to_vec());
            }
            Err(RunErr::NotFound)
        },
        read_linux_image,
    )
    .unwrap();
    assert!(matches!(xclip_empty, PromptClipboard::Empty));

    assert_eq!(
        first_image_mime(["image/webp", "image/gif"].into_iter()),
        Some("image/webp")
    );
    assert_eq!(first_image_mime(["  ", ""].into_iter()), None);
    let dup = format!("{img}\n{img}\n", img = img.display());
    let paths = parse_uri_list(&dup);
    assert_eq!(paths.len(), 1, "{paths:?}");

    let uri_too_large = with_cmd_stub(
        |bin, args| {
            if bin == "wl-paste" && args == ["--list-types"] {
                return Ok(b"text/uri-list\n".to_vec());
            }
            Err(RunErr::TooLarge)
        },
        read_linux_image,
    )
    .unwrap_err();
    assert!(uri_too_large.contains("too large"), "{uri_too_large}");

    let xclip_uri_too_large = with_cmd_stub(
        |bin, args| {
            if bin == "wl-paste" {
                return Err(RunErr::NotFound);
            }
            if bin == "xclip" && args.contains(&"TARGETS") {
                return Ok(b"text/uri-list\n".to_vec());
            }
            Err(RunErr::TooLarge)
        },
        read_linux_image,
    )
    .unwrap_err();
    assert!(
        xclip_uri_too_large.contains("too large"),
        "{xclip_uri_too_large}"
    );
}

#[test]
fn macos_clipboard_covers_pngpaste_and_osascript() {
    let png = png_bytes();
    let png_hit = png.clone();
    let pasted = with_cmd_stub(
        move |bin, _| {
            if bin == "pngpaste" {
                return Ok(png_hit.clone());
            }
            Err(RunErr::NotFound)
        },
        read_macos_image,
    )
    .unwrap();
    assert!(matches!(pasted, PromptClipboard::ImagePaths(p) if p.len() == 1));

    let empty_paste = with_cmd_stub(
        |bin, _| {
            if bin == "pngpaste" {
                return Err(RunErr::Exit);
            }
            if bin == "osascript" {
                return Ok(b"\n".to_vec());
            }
            Err(RunErr::NotFound)
        },
        read_macos_image,
    )
    .unwrap();
    assert!(matches!(empty_paste, PromptClipboard::Empty));

    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png.as_slice());
    let decoded = with_cmd_stub(
        move |bin, _| {
            if bin == "pngpaste" {
                return Err(RunErr::NotFound);
            }
            if bin == "osascript" {
                return Ok(b64.clone().into_bytes());
            }
            Err(RunErr::NotFound)
        },
        read_macos_image,
    )
    .unwrap();
    assert!(matches!(decoded, PromptClipboard::ImagePaths(p) if p.len() == 1));

    let missing = with_cmd_stub(
        |bin, _| {
            if bin == "pngpaste" || bin == "osascript" {
                return Err(RunErr::NotFound);
            }
            Err(RunErr::Exit)
        },
        read_macos_image,
    )
    .unwrap_err();
    assert!(missing.contains("osascript"), "{missing}");

    let bad_b64 = with_cmd_stub(
        |bin, _| {
            if bin == "pngpaste" {
                return Err(RunErr::Timeout);
            }
            if bin == "osascript" {
                return Ok(b"!!!not-base64!!!".to_vec());
            }
            Err(RunErr::NotFound)
        },
        read_macos_image,
    )
    .unwrap_err();
    assert!(bad_b64.contains("decode"), "{bad_b64}");

    let huge = with_cmd_stub(|_, _| Err(RunErr::TooLarge), read_macos_image).unwrap_err();
    assert!(huge.contains("too large"), "{huge}");
    let io_err = with_cmd_stub(
        |_, _| Err(RunErr::Io("pasteboard".into())),
        read_macos_image,
    )
    .unwrap_err();
    assert!(io_err.contains("pasteboard"), "{io_err}");
}
