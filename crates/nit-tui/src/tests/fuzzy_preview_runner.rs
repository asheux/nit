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

#[test]
fn expand_tabs_leading_tab_becomes_four_spaces() {
    let chars: Vec<char> = "\tdocker".chars().collect();
    let styles = vec![ratatui::style::Style::default(); chars.len()];
    let (out_chars, out_styles) = expand_tabs(chars, styles);
    let s: String = out_chars.iter().collect();
    assert_eq!(s, "    docker");
    assert_eq!(out_styles.len(), out_chars.len());
}

#[test]
fn expand_tabs_advances_to_next_stop() {
    // "ab" leaves col=2; next stop is 4 -> 2 spaces.
    let chars: Vec<char> = "ab\tcd".chars().collect();
    let styles = vec![ratatui::style::Style::default(); chars.len()];
    let (out_chars, _) = expand_tabs(chars, styles);
    let s: String = out_chars.iter().collect();
    assert_eq!(s, "ab  cd");
}

#[test]
fn expand_tabs_returns_unchanged_when_no_tab() {
    let chars: Vec<char> = "no tabs here".chars().collect();
    let styles = vec![ratatui::style::Style::default(); chars.len()];
    let (out_chars, out_styles) = expand_tabs(chars.clone(), styles.clone());
    assert_eq!(out_chars, chars);
    assert_eq!(out_styles.len(), styles.len());
}
