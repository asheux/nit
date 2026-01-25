use nit_core::{AppState, PaneId};
use ratatui::{
    layout::Constraint,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, BorderType, Borders, Cell, Row, Table},
    Frame,
};

use crate::theme::Theme;

pub fn render(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    syntax_status: &str,
) {
    let focused = state.focus == PaneId::GateMonitor;
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
            "GATE MONITOR",
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    let (ln, col) = state.line_col();
    let rows = vec![
        Row::new(vec![Cell::from("Focus"), Cell::from(state.focus.title())]),
        Row::new(vec![
            Cell::from("Mode"),
            Cell::from(format!("{:?}", state.mode)),
        ]),
        Row::new(vec![
            Cell::from("Dirty"),
            Cell::from(if state.editor_buffer().is_dirty() {
                "Y"
            } else {
                "N"
            }),
        ]),
        Row::new(vec![
            Cell::from("Ln/Col"),
            Cell::from(format!("{ln}/{col}")),
        ]),
        Row::new(vec![
            Cell::from("Bytes"),
            Cell::from(state.editor_buffer().bytes_len().to_string()),
        ]),
        Row::new(vec![
            Cell::from("Render(ms)"),
            Cell::from(state.metrics.last_render_ms.to_string()),
        ]),
        Row::new(vec![
            Cell::from("Frame"),
            Cell::from(state.metrics.frame_count.to_string()),
        ]),
        Row::new(vec![
            Cell::from("Workspace"),
            Cell::from(shorten_path(&state.workspace_root, 30)),
        ]),
        Row::new(vec![
            Cell::from("Viz Seed"),
            Cell::from(format!("{}", state.visualizer.seed)),
        ]),
        Row::new(vec![Cell::from("Syntax"), Cell::from(syntax_status)]),
        Row::new(vec![
            Cell::from("Job paused"),
            Cell::from(format!("{}", state.job.paused)),
        ]),
    ];

    let table = Table::new(rows, [Constraint::Length(14), Constraint::Min(5)])
        .column_spacing(1)
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));

    frame.render_widget(block, area);
    frame.render_widget(table, inner);
}

fn shorten_path(path: &std::path::Path, max: usize) -> String {
    let s = path.display().to_string();
    if s.len() <= max {
        s
    } else {
        format!("…{}", &s[s.len() - (max - 1)..])
    }
}
