use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::Theme;

use super::render_code_block_line;

pub(super) fn is_markdown_table_candidate(line: &str) -> bool {
    line.starts_with('|') && line.ends_with('|') && line.matches('|').count() >= 3
}

pub(super) fn flush_markdown_table(
    out: &mut Vec<Line<'static>>,
    table_lines: &mut Vec<String>,
    theme: &Theme,
    width: usize,
) {
    if table_lines.is_empty() {
        return;
    }
    out.extend(render_markdown_table(table_lines.as_slice(), theme, width));
    table_lines.clear();
}

fn render_markdown_table(lines: &[String], theme: &Theme, width: usize) -> Vec<Line<'static>> {
    if lines.len() < 2 {
        return lines
            .iter()
            .flat_map(|line| render_code_block_line(line, theme, width))
            .collect();
    }

    let rows = lines
        .iter()
        .map(|line| split_markdown_table_cells(line))
        .collect::<Vec<_>>();
    if rows.len() < 2 || !is_markdown_table_separator(rows[1].as_slice()) {
        return lines
            .iter()
            .flat_map(|line| render_code_block_line(line, theme, width))
            .collect();
    }

    let headers = rows.first().cloned().unwrap_or_default();
    let body = rows.iter().skip(2).cloned().collect::<Vec<Vec<String>>>();
    let cols = headers
        .len()
        .max(body.iter().map(|row| row.len()).max().unwrap_or(0));
    if cols == 0 {
        return vec![Line::from("")];
    }

    let mut widths = vec![3usize; cols];
    for (idx, cell) in headers.iter().enumerate() {
        widths[idx] = widths[idx].max(UnicodeWidthStr::width(cell.as_str()));
    }
    for row in body.iter() {
        for (idx, cell) in row.iter().enumerate() {
            if idx < widths.len() {
                widths[idx] = widths[idx].max(UnicodeWidthStr::width(cell.as_str()));
            }
        }
    }

    let chrome = cols.saturating_mul(3).saturating_add(1);
    let available = width.saturating_sub(chrome).max(cols);
    let max_col = available / cols;
    for cell_width in widths.iter_mut() {
        *cell_width = (*cell_width).min(max_col.max(6));
    }
    // Give remaining space to the last column so it fills the full width.
    let used: usize = widths.iter().sum();
    if used < available && cols > 0 {
        widths[cols - 1] += available - used;
    }

    let border = format_table_border(widths.as_slice());
    let border_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let border_line = || Line::from(Span::styled(border.clone(), border_style));

    let mut out = Vec::new();
    out.push(border_line());
    out.push(table_row_line(
        headers.as_slice(),
        widths.as_slice(),
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
        theme,
    ));
    out.push(border_line());
    for row in body.iter() {
        out.push(table_row_line(
            row.as_slice(),
            widths.as_slice(),
            Style::default().fg(theme.foreground),
            theme,
        ));
    }
    out.push(border_line());
    out
}

fn table_row_line(
    cells: &[String],
    widths: &[usize],
    cell_style: Style,
    theme: &Theme,
) -> Line<'static> {
    let mut spans = vec![Span::styled(
        "|".to_string(),
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
    )];
    for (idx, width) in widths.iter().enumerate() {
        let cell = cells.get(idx).cloned().unwrap_or_default();
        let cell = truncate_to_width(cell.as_str(), *width);
        let pad = width.saturating_sub(UnicodeWidthStr::width(cell.as_str()));
        spans.push(Span::styled(" ".to_string(), cell_style));
        spans.push(Span::styled(cell, cell_style));
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), cell_style));
        }
        spans.push(Span::styled(" ".to_string(), cell_style));
        spans.push(Span::styled(
            "|".to_string(),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ));
    }
    Line::from(spans)
}

fn split_markdown_table_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn is_markdown_table_separator(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let trimmed = cell.trim();
            !trimmed.is_empty() && trimmed.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
        })
}

fn format_table_border(widths: &[usize]) -> String {
    let mut line = String::from("+");
    for width in widths {
        line.push_str(&"-".repeat(width.saturating_add(2)));
        line.push('+');
    }
    line
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        if used.saturating_add(ch_width).saturating_add(1) > width {
            break;
        }
        out.push(ch);
        used = used.saturating_add(ch_width);
    }
    out.push('…');
    out
}
