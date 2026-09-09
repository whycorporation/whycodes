use super::*;
use std::io::Write;

#[test]
fn media_types() {
    assert_eq!(media_type_for_path(Path::new("a.PNG")), Some("image/png"));
    assert_eq!(media_type_for_path(Path::new("x.jpeg")), Some("image/jpeg"));
    assert_eq!(media_type_for_path(Path::new("a.txt")), None);
}

#[test]
fn normalize_strips_quotes_and_file_uri() {
    let p = normalize_path_token("\"/tmp/shot.png\"").unwrap();
    assert_eq!(p, PathBuf::from("/tmp/shot.png"));
    let p = normalize_path_token("file:///tmp/shot.png").unwrap();
    assert_eq!(p, PathBuf::from("/tmp/shot.png"));
    let p = normalize_path_token("file:///tmp/my%20shot.png").unwrap();
    assert_eq!(p, PathBuf::from("/tmp/my shot.png"));
}

#[test]
fn classify_whole_paste_as_image() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pic.png");
    std::fs::write(&path, b"\x89PNG\r\n").unwrap();
    let path_str = path.to_string_lossy().to_string();

    let c = classify_paste(&path_str);
    assert_eq!(c.images.len(), 1);
    assert!(c.text.is_empty());

    let quoted = format!("'{path_str}'");
    let c = classify_paste(&quoted);
    assert_eq!(c.images.len(), 1);

    let with_nl = format!("{path_str}\n");
    let c = classify_paste(&with_nl);
    assert_eq!(c.images.len(), 1);
}

#[test]
fn classify_mixed_keeps_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.jpg");
    std::fs::write(&path, b"jpeg").unwrap();
    let paste = format!("what is this? {}", path.display());
    let c = classify_paste(&paste);
    assert_eq!(c.images.len(), 1);
    assert!(c.text.contains("what is this?"));
    // Image path token is stripped from leftover text.
    assert!(!c.text.contains("a.jpg"));
}

#[test]
fn classify_plain_text_unchanged() {
    let c = classify_paste("hello world\nsecond line");
    assert!(c.images.is_empty());
    assert_eq!(c.text, "hello world\nsecond line");
}

#[test]
fn load_and_encode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.png");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(b"fakepng").unwrap();
    let img = load_prompt_image(&path).unwrap();
    assert_eq!(img.media_type, "image/png");
    assert_eq!(img.label, "x.png");
    let block = encode_image_block(&img).unwrap();
    match block {
        ContentBlock::Image {
            source: ImageSource::Base64 { media_type, data },
        } => {
            assert_eq!(media_type, "image/png");
            assert_eq!(STANDARD.decode(data).unwrap(), b"fakepng");
        }
        _ => panic!("expected image block"),
    }
}

#[test]
fn build_blocks_image_only_adds_cue() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("y.webp");
    std::fs::write(&path, b"webp").unwrap();
    let img = load_prompt_image(&path).unwrap();
    let blocks = build_user_blocks("", &[img]).unwrap();
    assert!(matches!(&blocks[0], ContentBlock::Text { text } if text.contains("image")));
    assert!(matches!(&blocks[1], ContentBlock::Image { .. }));
}

#[test]
fn tokenize_respects_quotes() {
    let t = tokenize(r#"/tmp/a.png "/tmp/my photo.png" rest"#);
    assert_eq!(t.len(), 3);
    assert_eq!(t[1], "\"/tmp/my photo.png\"");
}

#[test]
fn normalize_tilde_and_empty_tokens() {
    assert!(normalize_path_token("").is_none());
    assert!(normalize_path_token("   ").is_none());
    assert!(normalize_path_token("\"\"").is_none());
    let prev_home = std::env::var_os("HOME");
    let prev_profile = std::env::var_os("USERPROFILE");
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("USERPROFILE", home.path());
    }
    let tilde = normalize_path_token("~").unwrap();
    assert_eq!(tilde, home.path());
    let nested = normalize_path_token("~/shot.png").unwrap();
    assert_eq!(nested, home.path().join("shot.png"));
    unsafe {
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_profile {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
    }
    assert!(from_hex(b'g').is_none());
    assert_eq!(from_hex(b'A'), Some(10));
    assert_eq!(from_hex(b'a'), Some(10));
    assert_eq!(percent_decode("%zz"), "%zz");
    assert_eq!(
        normalize_path_token("file://localhost/tmp/shot.png").unwrap(),
        PathBuf::from("/tmp/shot.png")
    );
    assert_eq!(
        normalize_path_token("file://shot.png").unwrap(),
        PathBuf::from("shot.png")
    );
    assert!(is_image_extension(Path::new("a.gif")));
    assert!(!is_image_extension(Path::new("a.txt")));
    for (name, mime) in [
        ("a.gif", "image/gif"),
        ("a.webp", "image/webp"),
        ("a.bmp", "image/bmp"),
        ("a.tif", "image/tiff"),
        ("a.tiff", "image/tiff"),
        ("a.ico", "image/x-icon"),
        ("a.svg", "image/svg+xml"),
        ("a.heic", "image/heic"),
        ("a.heif", "image/heic"),
        ("a.avif", "image/avif"),
        ("a.jpg", "image/jpeg"),
    ] {
        assert_eq!(media_type_for_path(Path::new(name)), Some(mime));
    }
    assert_eq!(
        PromptImage::label_for(Path::new("dir/shot.png")),
        "shot.png"
    );
}

#[test]
fn classify_empty_and_load_rejects() {
    let empty = classify_paste("   ");
    assert!(empty.images.is_empty());
    assert!(empty.text.is_empty());

    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("gone.png");
    assert!(load_prompt_image(&missing).is_err());
    let txt = dir.path().join("notes.txt");
    std::fs::write(&txt, b"hi").unwrap();
    assert!(
        load_prompt_image(&txt)
            .unwrap_err()
            .contains("not a supported")
    );
    let zero = dir.path().join("empty.png");
    std::fs::write(&zero, b"").unwrap();
    assert!(load_prompt_image(&zero).unwrap_err().contains("empty"));
    assert!(build_user_blocks("", &[]).is_err());

    let huge = dir.path().join("huge.png");
    {
        let f = std::fs::File::create(&huge).unwrap();
        f.set_len(MAX_IMAGE_BYTES + 1).unwrap();
    }
    assert!(load_prompt_image(&huge).unwrap_err().contains("too large"));

    let img = PromptImage {
        path: dir.path().join("gone-encode.png"),
        label: "gone-encode.png".into(),
        media_type: "image/png".into(),
    };
    assert!(encode_image_block(&img).is_err());
    let huge_img = PromptImage {
        path: huge,
        label: "huge.png".into(),
        media_type: "image/png".into(),
    };
    assert!(
        encode_image_block(&huge_img)
            .unwrap_err()
            .contains("too large")
    );

    let ok = dir.path().join("ok.png");
    std::fs::write(&ok, b"png").unwrap();
    let loaded = load_prompt_image(&ok).unwrap();
    let blocks = build_user_blocks("see this", &[loaded]).unwrap();
    assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "see this"));
    assert!(matches!(&blocks[1], ContentBlock::Image { .. }));
}
