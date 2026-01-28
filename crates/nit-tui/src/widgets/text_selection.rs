use nit_core::{UiSelection, UiSelectionPane};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

pub fn apply_ui_selection(
    lines: Vec<Line<'static>>,
    selection: Option<&UiSelection>,
    pane: UiSelectionPane,
    selection_bg: Color,
    scroll: usize,
) -> Vec<Line<'static>> {
    let Some(selection) = selection else {
        return lines;
    };
    if selection.pane != pane {
        return lines;
    }

    let (start_line, start_col, end_line, end_col) = normalize(selection);
    lines
        .into_iter()
        .enumerate()
        .map(|(idx, line)| {
            let line_idx = scroll.saturating_add(idx);
            if line_idx < start_line || line_idx > end_line {
                return line;
            }
            let (sel_start, sel_end) = if start_line == end_line {
                (start_col, end_col)
            } else if line_idx == start_line {
                (start_col, usize::MAX)
            } else if line_idx == end_line {
                (0, end_col)
            } else {
                (0, usize::MAX)
            };
            highlight_line(line, sel_start, sel_end, selection_bg)
        })
        .collect()
}

fn normalize(selection: &UiSelection) -> (usize, usize, usize, usize) {
    let start = (selection.start_line, selection.start_col);
    let end = (selection.end_line, selection.end_col);
    if start <= end {
        (selection.start_line, selection.start_col, selection.end_line, selection.end_col)
    } else {
        (selection.end_line, selection.end_col, selection.start_line, selection.start_col)
    }
}

fn highlight_line(line: Line<'static>, sel_start: usize, sel_end: usize, selection_bg: Color) -> Line<'static> {
    if sel_start == sel_end {
        return line;
    }
    let mut spans = Vec::new();
    let mut index = 0usize;
    for span in line.spans.into_iter() {
        let content = span.content.to_string();
        let span_len = content.chars().count();
        let span_start = index;
        let span_end = index.saturating_add(span_len);
        if sel_end <= span_start || sel_start >= span_end {
            spans.push(span);
        } else {
            let start_in_span = sel_start.saturating_sub(span_start).min(span_len);
            let end_in_span = sel_end
                .min(span_end)
                .saturating_sub(span_start)
                .min(span_len);
            let (left, rest) = split_at_char(&content, start_in_span);
            let (mid, right) = split_at_char(&rest, end_in_span.saturating_sub(start_in_span));
            if !left.is_empty() {
                spans.push(Span::styled(left, span.style));
            }
            if !mid.is_empty() {
                let style = span.style.patch(Style::default().bg(selection_bg));
                spans.push(Span::styled(mid, style));
            }
            if !right.is_empty() {
                spans.push(Span::styled(right, span.style));
            }
        }
        index = span_end;
    }
    Line::from(spans)
}

fn split_at_char(input: &str, idx: usize) -> (String, String) {
    if idx == 0 {
        return ("".into(), input.to_string());
    }
    let mut count = 0usize;
    for (byte_idx, _) in input.char_indices() {
        if count == idx {
            return (input[..byte_idx].to_string(), input[byte_idx..].to_string());
        }
        count += 1;
    }
    (input.to_string(), "".into())
}
