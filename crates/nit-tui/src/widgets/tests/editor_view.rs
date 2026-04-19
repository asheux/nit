//! Tests for the editor view span builder: trailing-space padding, tab
//! expansion style propagation, and highlight snapshot versioning.

use super::*;
use nit_syntax::{EngineKind, HighlightSnapshot, LanguageId, SyntaxStatus};
use ratatui::style::Color;

/// Concatenate span contents into a plain string for visible-range assertions.
fn spans_to_string(spans: &[Span<'_>]) -> String {
    spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("")
}

#[test]
fn build_spans_fills_trailing_spaces() {
    let chars: Vec<char> = "ab".chars().collect();
    let base = Style::default();
    let styles = vec![base; chars.len()];
    let spans = build_spans(&chars, &styles, 0, 6, 4, base);
    let rendered = spans_to_string(&spans);
    let visible: String = rendered.chars().take(6).collect();
    assert_eq!(&visible[..2], "ab");
    assert!(visible.chars().skip(2).all(|ch| ch == ' '));
}

#[test]
fn tab_expands_with_style() {
    let chars: Vec<char> = "a\tb".chars().collect();
    let base = Style::default();
    let tab_style = Style::default().fg(Color::Red);
    let styles = vec![base, tab_style, base];
    let spans = build_spans(&chars, &styles, 0, 8, 4, base);
    let mut found = false;
    for span in &spans {
        let text = span.content.as_ref();
        if text.chars().all(|ch| ch == ' ') && text.len() == 3 {
            found = true;
            assert_eq!(span.style, tab_style);
        }
    }
    assert!(found, "expected tab expansion span with tab style");
}

#[test]
fn plain_snapshot_version_matches() {
    let snapshot = HighlightSnapshot::plain(
        1,
        1,
        LanguageId::PlainText,
        EngineKind::Plain,
        SyntaxStatus::Ok(EngineKind::Plain),
        "",
    );
    assert_eq!(snapshot.version, 1);
}
