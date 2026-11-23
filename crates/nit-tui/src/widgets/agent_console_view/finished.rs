//! Static terminal-turn bands. The rendered line supplies both the button
//! styling and its hit rectangle, including the short-width variant.
use super::*;
use unicode_width::UnicodeWidthStr;

pub(super) fn rows(
    status: &str,
    agent: &str,
    elapsed: Option<&str>,
    width: usize,
    artifact: Option<usize>,
) -> Vec<ThreadRow> {
    let width = width.max(1);
    let label = if width >= 11 {
        "[Artifacts]"
    } else if width >= 5 {
        "[Art]"
    } else {
        "…"
    };
    let header = if artifact.is_some() { label } else { "Reply" };
    vec![
        ThreadRow {
            text: pad_to_width(header, width),
            kind: ThreadRowKind::FinishedBand { artifact },
        },
        breather::panel_row(
            width,
            "AGENT",
            "ELAP",
            "HB",
            "OUT",
            ThreadRowKind::StatusHeader,
        ),
        breather::panel_row(
            width,
            agent,
            elapsed.unwrap_or("—"),
            "—",
            "—",
            ThreadRowKind::StatusRow,
        ),
        ThreadRow {
            text: pad_to_width(&format!("↳ {status}"), width),
            kind: ThreadRowKind::StatusSubRow,
        },
    ]
}

fn button_cols(text: &str) -> Option<std::ops::Range<usize>> {
    let label = text.trim_end();
    matches!(label, "[Artifacts]" | "[Art]" | "…").then(|| 0..label.width())
}

pub(super) fn line(text: &str, action: bool, theme: &Theme) -> Line<'static> {
    let mut style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    if action {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    Line::styled(text.trim_end().to_owned(), style)
}

pub(super) fn remember(state: &mut AppState, row: &ThreadRow, line: usize, area: Rect, y: u16) {
    let ThreadRowKind::FinishedBand {
        artifact: Some(idx),
    } = row.kind
    else {
        return;
    };
    if let Some(cols) = button_cols(&row.text) {
        state.agents.console_rows_cache.remember_button(
            area.x + cols.start as u16..area.x + cols.end as u16,
            y,
            line,
            idx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_button_and_identity_fit_terminal_cells() {
        for width in 5..85 {
            let rows = rows("Completed", "分析-agent", Some("2s"), width, Some(42));
            assert!(rows.iter().all(|r| r.text.width() <= width));
            let row = &rows[0];
            assert_eq!(button_cols(&row.text).unwrap().start, 0);
            assert_eq!(rows.len(), 4);
            assert!(matches!(
                row.kind,
                ThreadRowKind::FinishedBand { artifact: Some(42) }
            ));
            assert!(rows[2].text.contains('分'));
        }
    }
}
