use super::*;

#[test]
fn crlf_needs_a_majority_of_crlf_breaks() {
    assert!(is_crlf("a\r\nb\r\n"));
    assert!(is_crlf("a\r\nb\n"));
    assert!(!is_crlf("a\r\nb\nc\n"));
    assert!(!is_crlf("a\nb\n"));
    assert!(!is_crlf(""));
}

#[test]
fn to_crlf_does_not_double_existing_crlf() {
    assert_eq!(to_crlf("a\nb\r\nc"), "a\r\nb\r\nc");
}

#[test]
fn fold_maps_typographic_punctuation_to_ascii() {
    let text = "\u{2018}a\u{2019} \u{201C}b\u{201D} c\u{2014}d\u{00A0}e \u{2212}1";
    let (folded, _) = fold(text);
    assert_eq!(folded, "'a' \"b\" c-d e -1");
}

#[test]
fn fold_map_points_back_into_the_original() {
    let text = "x\u{2019}y";
    let (folded, map) = fold(text);
    assert_eq!(folded, "x'y");
    assert_eq!(map, vec![0, 1, 4, 5]);
    let y = folded.find('y').unwrap();
    assert_eq!(&text[map[y]..map[folded.len()]], "y");
}
