use super::*;

#[test]
fn test_short_path_unchanged() {
    let result = truncate_path("/home/user", 80);
    assert_eq!(result, "/home/user");
}

#[test]
fn test_long_path_truncated() {
    let result = truncate_path("/very/long/path/with/many/components/here", 30);
    // Should contain ... in the middle
    assert!(result.contains("..."));
    assert!(result.starts_with("/very"));
    assert!(result.ends_with("here"));
}

#[test]
fn test_very_short_max_len() {
    let result = truncate_path("/a/b/c/d/e/f/g/h", 12);
    assert!(result.len() <= 12 || result.contains("..."));
}

#[test]
fn short_component_path_is_cut_in_the_middle() {
    let p = "averylongname/anotherlongname";
    let out = truncate_path(p, 10);
    assert!(out.contains("..."), "{out}");
    assert!(out.len() < p.len());
}

#[test]
fn short_component_path_does_not_slice_mid_utf8() {
    // 3-component (or fewer) path: byte-cut at `third`. `ö` is 2 bytes.
    let p = format!("{}ö{}", "a".repeat(8), "b".repeat(8));
    let out = truncate_path(&p, 10);
    assert!(out.contains("..."), "{out}");
    assert!(out.is_char_boundary(out.len()));
}

#[test]
fn expand_sides_until_budget() {
    let p = "/aa/bb/cc/dd/ee/ff/gg/hh/ii/jj/kk";
    let out = truncate_path(p, 22);
    assert!(out.contains("..."), "{out}");
    let tight = truncate_path(p, 8);
    assert!(tight.contains("..."), "{tight}");
}
