use crate::theme::Theme;
use nit_core::{Buffer, Mode, PaneId};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

pub struct CursorPlacement {
    pub x: u16,
    pub y: u16,
}

pub fn render_editor(
    frame: &mut Frame,
    area: Rect,
    buffer: &Buffer,
    focus: PaneId,
    _mode: Mode,
    theme: &Theme,
) -> Option<CursorPlacement> {
    render_buffer(frame, area, buffer, focus, "EDITOR  [ SAVE ]", theme, true)
}

pub fn render_buffer(
    frame: &mut Frame,
    area: Rect,
    buffer: &Buffer,
    focus: PaneId,
    title: &str,
    theme: &Theme,
    show_cursor: bool,
) -> Option<CursorPlacement> {
    let focused = matches!(focus, PaneId::Editor | PaneId::Notes);
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

    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for i in start..end {
        let mut content = buffer
            .line_as_string(i)
            .replace('\t', "    ")
            .replace('\r', "");
        if content.ends_with('\n') {
            content.pop();
        }
        let ln = format!("{:>width$}", i + 1, width = line_num_width);
        let is_cursor_line = i == buffer.cursor.line;
        let mut spans = vec![
            Span::styled(
                format!(" {ln} "),
                Style::default()
                    .fg(theme.border)
                    .add_modifier(if is_cursor_line {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled("│ ", Style::default().fg(theme.border)),
        ];
        let mut style = Style::default().fg(theme.foreground);
        if is_cursor_line {
            style = style.bg(theme.cursor_line_bg);
        }
        spans.push(Span::styled(content, style));
        lines.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines)
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .block(block);

    frame.render_widget(paragraph, area);

    if show_cursor && focused {
        let cursor_line = buffer.cursor.line.saturating_sub(start);
        let cursor_col = buffer.cursor.col;
        let y = area.y + 1 + cursor_line as u16;
        let x = area.x + 1 + gutter_width as u16 + cursor_col as u16;
        return Some(CursorPlacement { x, y });
    }
    None
}
