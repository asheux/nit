use nit_core::AppState;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::theme::Theme;

pub fn render(frame: &mut Frame, area: ratatui::layout::Rect, state: &AppState, theme: &Theme) {
    let cap_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    let bar_area = Rect {
        x: area.x,
        y: area.y.saturating_add(1),
        width: area.width,
        height: area.height.saturating_sub(1),
    };

    let cap = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.border));
    frame.render_widget(cap, cap_area);

    let (line, col) = state.line_col();
    let mode = format!("{:?}", state.mode).to_uppercase();
    let file = state
        .editor_buffer()
        .path()
        .map(|p| p.display().to_string());
    let dirty = if state.editor_buffer().is_dirty() {
        "*"
    } else {
        ""
    };
    let file_text = file.unwrap_or_else(|| state.editor_buffer().name().to_string());
    let file_span = format!("{file_text}{dirty}");
    let line = Line::from(vec![
        Span::styled(
            " nit ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | ", Style::default().fg(theme.border)),
        Span::styled(
            file_span,
            Style::default()
                .fg(if dirty.is_empty() {
                    theme.foreground
                } else {
                    theme.warning
                })
                .add_modifier(if dirty.is_empty() {
                    Modifier::empty()
                } else {
                    Modifier::BOLD
                }),
        ),
        Span::styled(" | ", Style::default().fg(theme.border)),
        Span::styled(mode, Style::default().fg(theme.accent)),
        Span::styled(" | UTF-8 | ", Style::default().fg(theme.border)),
        Span::styled(
            format!("Ln {}, Col {}", line, col),
            Style::default().fg(theme.foreground),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " NEURAL INTERFACE TERMINAL ",
            Style::default()
                .fg(theme.title)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(theme.border));

    let para = Paragraph::new(line)
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .alignment(Alignment::Left)
        .block(block);

    frame.render_widget(para, bar_area);
}
