use crate::theme::Theme;
use nit_core::{Buffer, Mode, PaneId};
use nit_syntax::{HighlightSnapshot, LineSegment};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthChar;

pub struct CursorPlacement {
    pub x: u16,
    pub y: u16,
}

pub fn render_editor(
    frame: &mut Frame,
    area: Rect,
    buffer: &Buffer,
    snapshot: Option<&HighlightSnapshot>,
    focus: PaneId,
    _mode: Mode,
    theme: &Theme,
    tab_width: usize,
) -> Option<CursorPlacement> {
    render_buffer(
        frame,
        area,
        buffer,
        snapshot,
        PaneId::Editor,
        focus,
        "EDITOR  [ SAVE ]",
        theme,
        tab_width,
        true,
    )
}

pub fn render_buffer(
    frame: &mut Frame,
    area: Rect,
    buffer: &Buffer,
    snapshot: Option<&HighlightSnapshot>,
    pane_id: PaneId,
    focus: PaneId,
    title: &str,
    theme: &Theme,
    tab_width: usize,
    show_cursor: bool,
) -> Option<CursorPlacement> {
    let focused = focus == pane_id;
    let border_style = if focused {
        Style::default().fg(theme.border_focused)
    } else {
        Style::default().fg(theme.border)
    };
    let border_type = if focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    let title_color = if focused {
        theme.title_focused
    } else {
        theme.title
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .border_type(border_type)
        .style(Style::default().bg(theme.background))
        .title(Span::styled(
            title,
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ));

    let total_lines = buffer.lines_len().max(1);
    let line_num_width = total_lines.to_string().len().max(3);
    let gutter_width = line_num_width + 4;
    let start = buffer.viewport.offset_line;
    let height = buffer.viewport.height.max(1);
    let end = (start + height).min(total_lines);
    let content_width = buffer.viewport.width.max(1);

    let selection = buffer.selection_range();
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for i in start..end {
        let mut content = buffer.line_as_string(i).replace('\r', "");
        if content.ends_with('\n') {
            content.pop();
        }

        let is_cursor_line = i == buffer.cursor.line;
        let mut base_style = Style::default().fg(theme.foreground);
        if is_cursor_line {
            base_style = base_style
                .bg(theme.cursor_line_bg)
                .add_modifier(Modifier::UNDERLINED);
        }

        let chars: Vec<char> = content.chars().collect();
        let mut styles = vec![base_style; chars.len()];
        if let Some(snapshot) = snapshot {
            if let Some(segments) = snapshot.per_line.get(i) {
                apply_syntax_spans(&content, segments, &mut styles, theme);
            }
        }

        if let Some((sel_start, sel_end)) = selection.and_then(|(start, end)| {
            let line_start = buffer.line_char_start(i);
            let line_end = buffer.line_char_end(i);
            if end <= line_start || start >= line_end {
                return None;
            }
            let mut sel_start = start.saturating_sub(line_start);
            let mut sel_end = end.saturating_sub(line_start);
            if sel_start > chars.len() {
                sel_start = chars.len();
            }
            if sel_end > chars.len() {
                sel_end = chars.len();
            }
            if sel_end <= sel_start {
                None
            } else {
                Some((sel_start, sel_end))
            }
        }) {
            for idx in sel_start..sel_end.min(styles.len()) {
                styles[idx] = styles[idx].bg(theme.selection_bg);
            }
        }

        let ln = format!("{:>width$}", i + 1, width = line_num_width);
        let mut spans = vec![
            Span::styled(
                format!(" {ln} "),
                if is_cursor_line {
                    Style::default()
                        .fg(theme.border_focused)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.border)
                },
            ),
            Span::styled(
                "│ ",
                if is_cursor_line {
                    Style::default().fg(theme.border_focused)
                } else {
                    Style::default().fg(theme.border)
                },
            ),
        ];

        let offset_display = display_col_for_char_idx(&content, buffer.viewport.offset_col, tab_width);
        spans.extend(build_spans(
            &chars,
            &styles,
            offset_display,
            content_width,
            tab_width,
            base_style,
        ));
        lines.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines)
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .block(block);

    frame.render_widget(paragraph, area);

    if show_cursor && focused {
        let cursor_line = buffer.cursor.line.saturating_sub(start);
        let cursor_line_index = buffer
            .cursor
            .line
            .min(buffer.lines_len().saturating_sub(1));
        let line_str = buffer.line_as_string(cursor_line_index);
        let mut line = line_str.as_str();
        if line.ends_with('\n') {
            line = &line[..line.len().saturating_sub(1)];
        }
        let cursor_display_col =
            display_col_for_char_idx(line, buffer.cursor.col, tab_width);
        let offset_display = display_col_for_char_idx(line, buffer.viewport.offset_col, tab_width);
        let x = area.x + 1 + gutter_width as u16 + cursor_display_col.saturating_sub(offset_display) as u16;
        let y = area.y + 1 + cursor_line as u16;
        return Some(CursorPlacement { x, y });
    }
    None
}

fn apply_syntax_spans(
    line: &str,
    segments: &[LineSegment],
    styles: &mut [Style],
    theme: &Theme,
) {
    for seg in segments {
        if seg.start >= seg.end || seg.start >= line.len() {
            continue;
        }
        let start = byte_to_char_idx(line, seg.start);
        let end = byte_to_char_idx(line, seg.end.min(line.len()));
        let style = theme.highlight_style(seg.group);
        for idx in start..end.min(styles.len()) {
            styles[idx] = styles[idx].patch(style);
        }
    }
}

fn byte_to_char_idx(line: &str, mut byte: usize) -> usize {
    if byte > line.len() {
        byte = line.len();
    }
    while byte > 0 && !line.is_char_boundary(byte) {
        byte = byte.saturating_sub(1);
    }
    line[..byte].chars().count()
}

fn display_col_for_char_idx(line: &str, char_idx: usize, tab_width: usize) -> usize {
    let mut col = 0;
    let mut count = 0;
    for ch in line.chars() {
        if count >= char_idx {
            break;
        }
        if ch == '\t' {
            let tab = tab_width.max(1);
            let advance = tab - (col % tab);
            col += advance;
        } else {
            let w = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
            col += w;
        }
        count += 1;
    }
    col
}

fn build_spans(
    chars: &[char],
    styles: &[Style],
    offset_col: usize,
    width: usize,
    tab_width: usize,
    base_style: Style,
) -> Vec<Span<'static>> {
    if chars.is_empty() {
        return vec![Span::styled("", base_style)];
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_style = styles[0];
    let mut buffer = String::new();
    let mut col = 0;
    let visible_end = offset_col.saturating_add(width);

    let push_char = |ch: char, style: Style, spans: &mut Vec<Span<'static>>, buffer: &mut String, current_style: &mut Style| {
        if style != *current_style {
            if !buffer.is_empty() {
                spans.push(Span::styled(buffer.clone(), *current_style));
                buffer.clear();
            }
            *current_style = style;
        }
        buffer.push(ch);
    };

    for (idx, ch) in chars.iter().enumerate() {
        let style = styles[idx];
        if *ch == '\t' {
            let tab = tab_width.max(1);
            let advance = tab - (col % tab);
            for _ in 0..advance {
                if col >= visible_end {
                    break;
                }
                if col + 1 > offset_col {
                    push_char(' ', style, &mut spans, &mut buffer, &mut current_style);
                }
                col += 1;
            }
            continue;
        }

        let width = UnicodeWidthChar::width(*ch).unwrap_or(1).max(1);
        let start_col = col;
        let end_col = col + width;
        if end_col <= offset_col {
            col = end_col;
            continue;
        }
        if start_col >= visible_end {
            break;
        }
        push_char(*ch, style, &mut spans, &mut buffer, &mut current_style);
        col = end_col;
    }

    if !buffer.is_empty() {
        spans.push(Span::styled(buffer, current_style));
    }
    if spans.is_empty() {
        spans.push(Span::styled("", base_style));
    }
    spans
}
