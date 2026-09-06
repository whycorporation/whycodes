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
