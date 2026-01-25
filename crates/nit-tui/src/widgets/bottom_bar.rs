use nit_core::{AppState, PaneId};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::theme::Theme;

pub fn render(frame: &mut Frame, area: ratatui::layout::Rect, state: &AppState, theme: &Theme) {
    let focus = format!("FOCUS: {}", focus_name(state.focus));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.background));

    if area.height <= 2 {
        let titled = block.title(Span::styled(
            focus,
            Style::default().fg(theme.foreground).add_modifier(Modifier::DIM),
        ));
        frame.render_widget(titled, area);
        return;
    }

    let spans = vec![Span::styled(
        focus,
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
    )];

    let inner = block.inner(area);
    let para = Paragraph::new(Line::from(spans))
        .style(Style::default().bg(theme.background).fg(theme.foreground));
    frame.render_widget(block, area);
    frame.render_widget(para, inner);
}

fn focus_name(pane: PaneId) -> &'static str {
    pane.title()
}
