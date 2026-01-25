use nit_core::{AppState, PaneId};
use nit_utils::hashing::{stable_hash_bytes, XorShift64};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::theme::Theme;

pub fn render(frame: &mut Frame, area: ratatui::layout::Rect, state: &AppState, theme: &Theme) {
    let focused = state.focus == PaneId::Visualizer;
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
            "VISUALIZER  [ APPLY ] [ SEED ]",
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    let buffer = state.editor_buffer();
    let seed_base = stable_hash_bytes(buffer.content_as_string().as_bytes());
    let mut rng = XorShift64::new(seed_base ^ state.visualizer.seed);
    let charset = if state.visualizer.variant.is_multiple_of(2) {
        " .:-=+*#%@"
    } else {
        "░▒▓█╱╲╳"
    }
    .chars()
    .collect::<Vec<_>>();

    let height = inner.height as usize;
    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for _ in 0..height {
        let mut s = String::with_capacity(width);
        for _ in 0..width {
            let v = rng.next_f32();
            let idx = (v * charset.len() as f32) as usize;
            let idx = idx.min(charset.len().saturating_sub(1));
            s.push(charset[idx]);
        }
        lines.push(Line::from(Span::styled(
            s,
            Style::default().fg(theme.foreground),
        )));
    }

    let paragraph =
        Paragraph::new(lines).style(Style::default().bg(theme.background).fg(theme.foreground));
    frame.render_widget(block, area);
    frame.render_widget(paragraph, inner);
}
