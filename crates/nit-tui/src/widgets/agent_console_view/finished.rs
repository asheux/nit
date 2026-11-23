//! Static terminal-turn bands. The rendered line supplies both the button
//! styling and its hit rectangle, including the short-width variant.
use super::*;
use unicode_width::UnicodeWidthStr;

fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut cells = 1;
    for ch in text.chars() {
        cells += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if cells > width {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

fn button(width: usize) -> Option<&'static str> {
    if width >= "[Artifacts]".width() {
        Some("[Artifacts]")
    } else if width >= "[Art]".width() {
        Some("[Art]")
    } else {
        None
    }
}

pub(super) fn rows(
    status: &str,
    agent: &str,
    elapsed: Option<&str>,
    width: usize,
    artifact: Option<usize>,
) -> Vec<ThreadRow> {
    let width = width.max(1);
    let details = match elapsed {
        Some(t) => format!("{agent} · {t}"),
        None => agent.to_string(),
    };
    let label = format!(" {status}  {details}");
    let link = artifact.and_then(|_| button(width));
    let mut rows = Vec::new();
    let mut emit = |text: String, artifact| {
        rows.push(ThreadRow {
            text,
            kind: ThreadRowKind::FinishedBand { artifact },
        })
    };
    match link {
        Some(link) if label.width() + 2 + link.width() <= width => {
            let gap = width - label.width() - link.width();
            emit(format!("{label}{}{link}", " ".repeat(gap)), artifact);
        }
        Some(link) => {
            emit(fit(&format!(" {status}"), width), None);
            let available = width.saturating_sub(link.width() + 1);
            if available < 4 && !details.is_empty() {
                emit(fit(&details, width), None);
                emit(
                    format!("{}{link}", " ".repeat(width - link.width())),
                    artifact,
                );
                return rows;
            }
            let detail = fit(&details, available);
            let gap = width.saturating_sub(detail.width() + link.width());
            emit(format!("{detail}{}{link}", " ".repeat(gap)), artifact);
        }
        None => emit(fit(&label, width), None),
    }
    rows
}

fn button_cols(text: &str) -> Option<std::ops::Range<usize>> {
    ["[Artifacts]", "[Art]"].iter().find_map(|label| {
        let prefix = text.strip_suffix(label)?;
        Some(prefix.width()..prefix.width() + label.width())
    })
}

pub(super) fn line(text: &str, action: bool, theme: &Theme) -> Line<'static> {
    let band = Style::default()
        .bg(theme.list_background())
        .fg(theme.foreground);
    let mut spans = Vec::new();
    let label = action
        .then(|| {
            ["[Artifacts]", "[Art]"]
                .into_iter()
                .find(|s| text.ends_with(s))
        })
        .flatten();
    let body = label.and_then(|s| text.strip_suffix(s)).unwrap_or(text);
    if let Some(status) = ["Completed", "Failed", "Aborted", "Reply"]
        .iter()
        .find(|s| {
            body.strip_prefix(' ')
                .is_some_and(|rest| rest.starts_with(**s))
        })
    {
        let end = status.len() + 1;
        let fg = match *status {
            "Completed" => theme.success,
            "Failed" => theme.error,
            "Aborted" => theme.warning,
            _ => theme.muted,
        };
        spans.push(Span::styled(
            body[..end].to_owned(),
            band.fg(fg).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(body[end..].to_owned(), band));
    } else {
        spans.push(Span::styled(body.to_owned(), band));
    }
    if let Some(label) = label {
        spans.push(Span::styled(
            label,
            Style::default()
                .bg(theme.selection_bg)
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ));
    }
    Line::from(spans).style(band)
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
            let row = rows.last().unwrap();
            assert_eq!(button_cols(&row.text).unwrap().end, width);
            assert!(matches!(
                row.kind,
                ThreadRowKind::FinishedBand { artifact: Some(42) }
            ));
            assert!(rows.iter().any(|r| r.text.contains('分')));
        }
    }
}
