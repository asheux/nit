use super::*;

#[test]
fn find_matches_returns_multiple_ranges() {
    let line = "abc main xyz main";
    let ranges = find_matches(line, "main");
    assert_eq!(ranges, vec![(4, 8), (13, 17)]);
}

#[test]
fn find_matches_handles_unicode_char_boundaries() {
    let line = "αβγδε";
    let ranges = find_matches(line, "βγ");
    assert_eq!(ranges, vec![(1, 3)]);
}
