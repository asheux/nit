use super::*;
use nit_core::AgentConsoleRowKind as ThreadRowKind;

fn row(text: &str) -> ThreadRow {
    ThreadRow {
        text: text.into(),
        kind: ThreadRowKind::Agent,
    }
}

#[test]
fn extend_to_seeds_selection_when_absent() {
    let mut pane = PaneSession::default();
    extend_to(&mut pane, 3, 7);
    let sel = pane.selection.as_ref().expect("seeded");
    assert_eq!(sel.anchor_line, 3);
    assert_eq!(sel.anchor_col, 7);
    assert_eq!(sel.end_line, 3);
    assert_eq!(sel.end_col, 7);
}

#[test]
fn extend_to_updates_end_only() {
    let mut pane = PaneSession::default();
    extend_to(&mut pane, 1, 0);
    extend_to(&mut pane, 4, 9);
    let sel = pane.selection.as_ref().unwrap();
    assert_eq!(sel.anchor_line, 1);
    assert_eq!(sel.anchor_col, 0);
    assert_eq!(sel.end_line, 4);
    assert_eq!(sel.end_col, 9);
}

#[test]
fn clear_drops_selection() {
    let mut pane = PaneSession::default();
    extend_to(&mut pane, 0, 0);
    clear(&mut pane);
    assert!(pane.selection.is_none());
}

#[test]
fn resolve_text_returns_none_without_selection() {
    let pane = PaneSession::default();
    let rows = vec![row("hello")];
    assert!(resolve_text(&pane, &rows).is_none());
}

#[test]
fn resolve_text_slices_single_row() {
    let mut pane = PaneSession::default();
    extend_to(&mut pane, 0, 2);
    extend_to(&mut pane, 0, 5);
    let rows = vec![row("hello world")];
    assert_eq!(resolve_text(&pane, &rows).as_deref(), Some("llo"));
}

#[test]
fn resolve_text_handles_reverse_order() {
    let mut pane = PaneSession::default();
    extend_to(&mut pane, 0, 5);
    extend_to(&mut pane, 0, 2);
    let rows = vec![row("hello world")];
    assert_eq!(resolve_text(&pane, &rows).as_deref(), Some("llo"));
}

#[test]
fn resolve_text_spans_multiple_rows() {
    let mut pane = PaneSession::default();
    extend_to(&mut pane, 0, 6);
    extend_to(&mut pane, 2, 5);
    let rows = vec![row("hello world"), row("middle line"), row("third stop")];
    let text = resolve_text(&pane, &rows).expect("multi-row");
    assert_eq!(text, "world\nmiddle line\nthird");
}

#[test]
fn resolve_text_clamps_overshoot_columns() {
    let mut pane = PaneSession::default();
    extend_to(&mut pane, 0, 0);
    extend_to(&mut pane, 0, 999);
    let rows = vec![row("short")];
    assert_eq!(resolve_text(&pane, &rows).as_deref(), Some("short"));
}
