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
#[path = "../tests/multipane_selection.rs"]
mod tests;
