use nit_core::{AppState, FileTreeKind, PaneId};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::theme::Theme;

fn blend(a: ratatui::style::Color, b: ratatui::style::Color, t: f32) -> ratatui::style::Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (ratatui::style::Color::Rgb(ar, ag, ab), ratatui::style::Color::Rgb(br, bg, bb)) => {
            let mix = |x: u8, y: u8| -> u8 {
                let xf = x as f32;
                let yf = y as f32;
                (xf * (1.0 - t) + yf * t).round().clamp(0.0, 255.0) as u8
            };
            ratatui::style::Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
        }
        _ => a,
    }
}

pub fn render(f: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let hidden = if state.file_tree.show_hidden {
        "ON"
    } else {
        "OFF"
    };
    let ignored = if state.file_tree.show_ignored {
        "ON"
    } else {
        "OFF"
    };
    let commands = format!(
        "[Enter open/toggle] [Esc/q close] [r refresh] [. hidden:{}] [i ignored:{}]",
        hidden, ignored
    );
    let focused = state.focus == PaneId::Editor;
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
    let title = Line::from(vec![
        Span::styled(
            "NITTREE",
            Style::default()
                .fg(title_color)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            commands,
            Style::default()
                .fg(theme.border)
                .bg(theme.background)
                .add_modifier(Modifier::DIM),
        ),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.background))
        .border_style(border_style)
        .border_type(border_type)
        .title(title);

    let inner_height = area.height.saturating_sub(2) as usize;
    let rows = &state.file_tree.rows;
    let scroll = state.file_tree.scroll_offset;
    let selected = state.file_tree.selected;
    let file_fg = blend(theme.foreground, theme.title, 0.28);

    let end = (scroll + inner_height).min(rows.len());
    let mut lines = Vec::new();
    for (idx, row) in rows.iter().enumerate().take(end).skip(scroll) {
        let mut style = match row.kind {
            FileTreeKind::Dir => Style::default().fg(theme.title),
            FileTreeKind::File => Style::default().fg(file_fg),
            FileTreeKind::Loading => Style::default().fg(theme.warning),
        };
        if idx == selected {
            style = style.bg(theme.selection_bg).add_modifier(Modifier::BOLD);
        }
        if matches!(row.kind, FileTreeKind::Dir) {
            let indent_chars = row.depth.saturating_mul(2);
            let arrow_chars = 2;
            let mut chars = row.text.chars();
            let indent: String = chars.by_ref().take(indent_chars).collect();
            let arrow: String = chars.by_ref().take(arrow_chars).collect();
            let rest: String = chars.collect();
            if !arrow.is_empty() {
                let arrow_style = style.fg(theme.accent).add_modifier(Modifier::BOLD);
                lines.push(Line::from(vec![
                    Span::styled(indent, style),
                    Span::styled(arrow, arrow_style),
                    Span::styled(rest, style),
                ]));
            } else {
                lines.push(Line::from(Span::styled(row.text.clone(), style)));
            }
        } else {
            lines.push(Line::from(Span::styled(row.text.clone(), style)));
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .style(Style::default().bg(theme.background).fg(theme.foreground));
    f.render_widget(paragraph, area);
}
