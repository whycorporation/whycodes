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
