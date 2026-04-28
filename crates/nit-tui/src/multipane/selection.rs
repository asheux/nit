//! Pure helpers for the per-pane chat-thread text selection. State
//! lives on `PaneSession::selection`; coordinates are LOGICAL row
//! indices into `build_pane_thread_rows` output (pre-`chat_thread_scroll`),
//! so the highlight survives viewport scroll.

use nit_core::{AgentConsoleRow as ThreadRow, PaneSelection, PaneSession};

pub fn extend_to(pane: &mut PaneSession, line: usize, col: usize) {
    let sel = pane.selection.get_or_insert(PaneSelection {
        anchor_line: line,
        anchor_col: col,
        end_line: line,
        end_col: col,
    });
    sel.end_line = line;
    sel.end_col = col;
}

pub fn clear(pane: &mut PaneSession) {
    pane.selection = None;
}

/// Resolve the text covered by `pane.selection` against the rendered
/// thread rows. Returns `None` when no selection is active or the
/// selection collapses to zero characters.
pub fn resolve_text(pane: &PaneSession, rendered_rows: &[ThreadRow]) -> Option<String> {
    let sel = pane.selection.as_ref()?;
    if rendered_rows.is_empty() {
        return None;
    }
    let (start_line, start_col, end_line, end_col) =
        if (sel.anchor_line, sel.anchor_col) <= (sel.end_line, sel.end_col) {
            (sel.anchor_line, sel.anchor_col, sel.end_line, sel.end_col)
        } else {
            (sel.end_line, sel.end_col, sel.anchor_line, sel.anchor_col)
        };
    let last_line = rendered_rows.len() - 1;
    let start_line = start_line.min(last_line);
    let end_line = end_line.min(last_line);
    if start_line > end_line {
        return None;
    }
    let mut out: Vec<String> = Vec::with_capacity(end_line - start_line + 1);
    for (idx, row) in rendered_rows
        .iter()
        .enumerate()
        .take(end_line + 1)
        .skip(start_line)
    {
        let chars: Vec<char> = row.text.chars().collect();
        let n = chars.len();
        let lo = if idx == start_line {
            start_col.min(n)
        } else {
            0
        };
        let hi = if idx == end_line { end_col.min(n) } else { n };
        let slice: String = if lo < hi {
            chars[lo..hi].iter().collect()
        } else {
            String::new()
        };
        out.push(slice.trim_end().to_string());
    }
    let joined = out.join("\n");
    (!joined.trim().is_empty()).then_some(joined)
}

#[cfg(test)]
mod tests {
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
}
