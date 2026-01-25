use nit_core::{AppState, PaneId, VisualizerMode};
use nit_gol::{AttractorEvent, Grid};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthChar;

use crate::theme::Theme;

pub fn render(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    grid: Option<&Grid>,
) {
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
            "VISUALIZER  [ APPLY ] [ SEED ] [ SNAP ] [ SEARCH ]",
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    if inner.width == 0 || inner.height == 0 {
        frame.render_widget(block, area);
        return;
    }

    let header = build_header(state);
    let header = truncate_to_width(&header, inner.width as usize);
    let header_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);

    let live_style = Style::default().fg(theme.title_focused);
    let live_char = if state.visualizer.variant.is_multiple_of(2) {
        '█'
    } else {
        '▓'
    };
    let dead_style = Style::default().fg(theme.background);

    let mut lines: Vec<Line> = Vec::with_capacity(inner.height as usize);
    lines.push(Line::from(Span::styled(header, header_style)));

    let grid_height = inner.height.saturating_sub(1) as usize;
    let width = inner.width as usize;
    for row in 0..grid_height {
        let line = build_grid_line(grid, row, width, live_char, live_style, dead_style);
        lines.push(line);
    }

    let paragraph =
        Paragraph::new(lines).style(Style::default().bg(theme.background).fg(theme.foreground));
    frame.render_widget(block, area);
    frame.render_widget(paragraph, inner);
}

fn build_header(state: &AppState) -> String {
    let mode = match state.visualizer.mode {
        VisualizerMode::SimOnly => "SIM",
        VisualizerMode::Search => "SEARCH",
    };
    let attractor = attractor_summary(state.visualizer.last_attractor.as_ref());
    let paused = if state.visualizer.paused { " PAUSED" } else { "" };
    format!(
        "Rule: {} | Gen: {:05} | Alive: {:04} | Attractor: {} | Mode: {}{}",
        state.visualizer.rule,
        state.visualizer.generation,
        state.visualizer.alive,
        attractor,
        mode,
        paused
    )
}

fn attractor_summary(event: Option<&AttractorEvent>) -> String {
    match event {
        Some(AttractorEvent::FixedPoint { .. }) => "FIXED".into(),
        Some(AttractorEvent::Cycle { period, transient, .. }) => {
            format!("P={period} (T={transient})")
        }
        None => "--".into(),
    }
}

fn build_grid_line(
    grid: Option<&Grid>,
    row: usize,
    width: usize,
    live_char: char,
    live_style: Style,
    dead_style: Style,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run_char: Option<char> = None;
    let mut run_len = 0usize;
    let mut run_alive: Option<bool> = None;

    for col in 0..width {
        let alive = grid
            .filter(|g| row < g.height() && col < g.width())
            .map(|g| g.get(col, row))
            .unwrap_or(false);
        let ch = if alive { live_char } else { ' ' };
        match run_char {
            Some(c) if c == ch && run_alive == Some(alive) => {
                run_len += 1;
            }
            Some(c) => {
                let style = if run_alive == Some(true) {
                    live_style
                } else {
                    dead_style
                };
                spans.push(Span::styled(c.to_string().repeat(run_len), style));
                run_char = Some(ch);
                run_alive = Some(alive);
                run_len = 1;
            }
            None => {
                run_char = Some(ch);
                run_alive = Some(alive);
                run_len = 1;
            }
        }
    }

    if let Some(c) = run_char {
        let style = if run_alive == Some(true) {
            live_style
        } else {
            dead_style
        };
        spans.push(Span::styled(c.to_string().repeat(run_len), style));
    }

    Line::from(spans)
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let mut width = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if width + w > max_width.saturating_sub(1) {
            out.push('…');
            return out;
        }
        out.push(ch);
        width += w;
    }
    out
}
