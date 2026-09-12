use super::*;
use std::fs;
use std::io::Write;

fn tmp() -> tempfile::TempDir {
    tempfile::TempDir::new().unwrap()
}

#[test]
fn pack_and_extract_unique_sorted() {
    assert_eq!(pack_trigram(b'a', b'b', b'c'), 0x00_61_62_63);
    assert!(extract_trigrams(b"ab").is_empty());
    assert_eq!(
        extract_trigrams(b"abc"),
        vec![pack_trigram(b'a', b'b', b'c')]
    );
    let tris = extract_trigrams(b"aaaa");
    assert_eq!(tris, vec![pack_trigram(b'a', b'a', b'a')]);
    let mixed = extract_indexed_trigrams(b"Abc");
    assert!(mixed.contains(&pack_trigram(b'A', b'b', b'c')));
    assert!(mixed.contains(&pack_trigram(b'a', b'b', b'c')));
    assert_eq!(extract_indexed_trigrams(b"abc"), extract_trigrams(b"abc"));
}

#[test]
fn required_trigrams_literal_and_escape() {
    let t = required_trigrams("fn main", false).unwrap();
    assert!(t.contains(&pack_trigram(b'f', b'n', b' ')));
    assert!(required_trigrams("ab", false).is_none());
    assert!(required_trigrams("a.b", false).is_none());
    assert!(required_trigrams("a*", false).is_none());
    let escaped = required_trigrams(r"a\.b", false).unwrap();
    assert_eq!(escaped, extract_trigrams(b"a.b"));
    assert!(required_trigrams(r"\d+", false).is_none());
    assert!(required_trigrams(r"\x", false).is_none());
    assert!(required_trigrams(r"trailing\", false).is_none());
    let slash = required_trigrams(r"a\\b", false).unwrap();
    assert_eq!(slash, extract_trigrams(br"a\b"));
}

#[test]
fn required_trigrams_case_fold() {
    let a = required_trigrams("Hello", true).unwrap();
    let b = required_trigrams("hello", false).unwrap();
    assert_eq!(a, b);
    let cs = required_trigrams("Hello", false).unwrap();
    assert_ne!(cs, b);
    assert!(required_trigrams("İstanbul", true).is_none());
    assert!(required_trigrams("İstanbul", false).is_some());
}

#[test]
fn as_literal_rejects_each_meta() {
    for c in REGEX_META {
        assert!(as_literal(&format!("a{c}b")).is_none(), "{c}");
    }
    assert_eq!(as_literal("plain").as_deref(), Some("plain"));
    assert_eq!(as_literal(r"\.\*\+").as_deref(), Some(".*+"));
}

#[test]
fn content_index_upsert_match_and_skip() {
    let dir = tmp();
    let root = dir.path();
    fs::write(root.join("hit.rs"), "fn unique_needle() {}\n").unwrap();
    fs::write(root.join("miss.rs"), "fn other() {}\n").unwrap();
    fs::write(root.join("bin.dat"), b"abc\0def").unwrap();
    fs::write(root.join("tiny.rs"), "ab").unwrap();
    fs::create_dir_all(root.join("src")).unwrap();

    let mut idx = ContentIndex::new();
    assert!(!idx.is_complete());
    idx.upsert(root, "hit.rs", 0);
    idx.upsert(root, "miss.rs", 0);
    idx.upsert(root, "bin.dat", 0);
    idx.upsert(root, "tiny.rs", 0);
    idx.upsert(root, "src", 0);
    idx.upsert(root, "gone.rs", 0);
    idx.mark_complete();
    assert!(idx.is_complete());
    assert_eq!(idx.indexed_len(), 3);
    assert_eq!(idx.skipped_len(), 1);

    let req = required_trigrams("unique_needle", false).unwrap();
    assert!(idx.may_match("hit.rs", &req));
    assert!(!idx.may_match("miss.rs", &req));
    assert!(!idx.may_match("bin.dat", &req));
    assert!(idx.may_match("gone.rs", &req));
    assert!(!idx.may_match("tiny.rs", &req));
    assert!(idx.may_match("hit.rs", &[]));

    idx.remove("hit.rs");
    assert!(idx.may_match("hit.rs", &req));

    fs::write(root.join("Case.rs"), "HelloWorld\n").unwrap();
    idx.upsert(root, "Case.rs", 0);
    let mixed = required_trigrams("HelloWorld", false).unwrap();
    assert!(idx.may_match("Case.rs", &mixed));
    let lower_req = required_trigrams("helloworld", true).unwrap();
    assert!(idx.may_match("Case.rs", &lower_req));
    let all_caps = required_trigrams("HELLOWORLD", false).unwrap();
    assert!(!idx.may_match("Case.rs", &all_caps));

    idx.clear();
    assert!(!idx.is_complete());
    assert_eq!(idx.indexed_len(), 0);
}

#[test]
fn content_index_size_hint_and_remove_tree() {
    let dir = tmp();
    let root = dir.path();
    fs::create_dir_all(root.join("src/nested")).unwrap();
    fs::write(root.join("src/a.rs"), "fn alpha() {}\n").unwrap();
    fs::write(root.join("src/nested/b.rs"), "fn beta() {}\n").unwrap();
    fs::write(root.join("keep.rs"), "fn keep() {}\n").unwrap();

    let mut idx = ContentIndex::new();
    idx.upsert(root, "src/a.rs", 0);
    idx.upsert(root, "src/nested/b.rs", 0);
    idx.upsert(root, "keep.rs", 0);
    idx.upsert(root, "huge.rs", MAX_INDEX_BYTES + 1);
    assert_eq!(idx.skipped_len(), 0);
    let huge_req = required_trigrams("alpha", false).unwrap();
    assert!(idx.may_match("huge.rs", &huge_req));

    let req = required_trigrams("alpha", false).unwrap();
    assert!(idx.may_match("src/a.rs", &req));
    idx.remove_tree("src");
    assert!(idx.may_match("src/a.rs", &req));
    assert!(idx.may_match("src/nested/b.rs", &req));
    let keep = required_trigrams("keep", false).unwrap();
    assert!(idx.may_match("keep.rs", &keep));
}

#[test]
fn classify_metadata_skips_dir_and_missing() {
    let dir = tmp();
    assert_eq!(classify_metadata(dir.path()), Load::Unknown);
    assert_eq!(
        classify_metadata(&dir.path().join("nope.rs")),
        Load::Unknown
    );
    let p = dir.path().join("ok.rs");
    fs::write(&p, "hello world").unwrap();
    match classify_metadata(&p) {
        Load::Bytes(b) => assert_eq!(b, b"hello world"),
        other => panic!("{other:?}"),
    }
    let big = dir.path().join("big.rs");
    fs::write(&big, "x").unwrap();
    let f = fs::OpenOptions::new().write(true).open(&big).unwrap();
    f.set_len(MAX_INDEX_BYTES + 1).unwrap();
    drop(f);
    assert_eq!(classify_metadata(&big), Load::Unknown);
}

#[test]
fn load_file_and_read_helpers() {
    let dir = tmp();
    let p = dir.path().join("a.rs");
    fs::write(&p, "abc").unwrap();
    match load_file(&p, 3) {
        Load::Bytes(b) => assert_eq!(b, b"abc"),
        other => panic!("{other:?}"),
    }
    assert_eq!(load_file(&p, MAX_INDEX_BYTES + 1), Load::Unknown);
    assert_eq!(open_failed(), Load::Unknown);
    assert_eq!(read_failed(), Load::Unknown);
    assert_eq!(append_failed(), Load::Unknown);
    assert_eq!(missing_path(), Load::Unknown);
    assert_eq!(skip_non_file(), Load::Unknown);
    assert_eq!(too_large(), Load::Unknown);
    assert_eq!(tree_prefix("src"), "src/");
    assert!(contains_all(&[1, 3, 5], &[1, 5]));
    assert!(!contains_all(&[1, 3, 5], &[2]));
    assert!(!skip_unicode_casefold("Hello", true));
    assert!(skip_unicode_casefold("İ", true));
}

#[cfg(unix)]
#[test]
fn classify_skips_symlink() {
    let dir = tmp();
    let target = dir.path().join("t.rs");
    fs::write(&target, "fn target() {}\n").unwrap();
    let link = dir.path().join("l.rs");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert_eq!(classify_metadata(&link), Load::Unknown);
}

#[test]
fn read_text_or_binary_large_and_failed_open() {
    let dir = tmp();
    let p = dir.path().join("wide.rs");
    let mut f = fs::File::create(&p).unwrap();
    let chunk = vec![b'a'; BINARY_SNIFF + 32];
    f.write_all(&chunk).unwrap();
    f.flush().unwrap();
    drop(f);
    match read_text_or_binary(&p) {
        Load::Bytes(b) => assert_eq!(b.len(), BINARY_SNIFF + 32),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        read_text_or_binary(&dir.path().join("missing.rs")),
        Load::Unknown
    );
    let sniff = [b'x'; BINARY_SNIFF];
    let mut dummy = fs::File::open(&p).unwrap();
    assert_eq!(
        classify_sniff(Err(std::io::Error::other("read")), &sniff, &mut dummy),
        Load::Unknown
    );
    let mut buf = vec![b'a'; 3];
    assert_eq!(
        classify_append(
            Err(std::io::Error::other("append")),
            &mut buf,
            vec![b'b'; 2]
        ),
        Load::Unknown
    );
    let mut buf = vec![b'a'; 2];
    match classify_append(Ok(2), &mut buf, vec![b'b'; 2]) {
        Load::Bytes(b) => assert_eq!(b, b"aabb"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        classify_sniff(Ok(0), &sniff, &mut dummy),
        Load::Bytes(Vec::new())
    );
    let mut nul = [0u8; 4];
    nul[0] = b'a';
    nul[1] = 0;
    assert_eq!(classify_sniff(Ok(4), &nul, &mut dummy), Load::Binary);
}
