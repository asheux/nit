use nit_core::{AppState, UiSelectionPane};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::theme::Theme;
use crate::widgets::text_selection::apply_ui_selection;

const MIN_WIDTH: u16 = 68;
const MIN_HEIGHT: u16 = 18;

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = screen.width.clamp(MIN_WIDTH, 110);
    let height = screen.height.clamp(MIN_HEIGHT, 32);
    (width, height)
}

/// Count the rendered lines without actually building them. Used by the
/// scroll hot path to compute `max_scroll` cheaply — rebuilding styled line
/// vectors on every wheel tick was causing sluggish scrolling. Must stay in
/// lock-step with `build_lines` below.
pub fn line_count(state: &AppState) -> usize {
    // status line
    let mut count = 1usize;
    // error line
    if state.games.run_browser.last_error.is_some() {
        count += 1;
    }
    if state.games.run_browser.loading {
        // blank + "Loading runs..."
        return count + 2;
    }
    // blank
    count += 1;
    if state.games.run_browser.entries.is_empty() {
        // "No runs found ..."
        count += 1;
    } else {
        count += state.games.run_browser.entries.len();
    }
    // blank + footer hint
    count += 2;
    count
}

pub fn build_lines(state: &AppState, theme: &Theme, inner_width: u16) -> Vec<Line<'static>> {
    let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let warn_style = Style::default()
        .fg(theme.warning)
        .add_modifier(Modifier::BOLD);
    let selected_style = Style::default()
        .fg(theme.foreground)
        .bg(theme.selection_bg)
        .add_modifier(Modifier::BOLD);

    let max_width = inner_width.max(1) as usize;
    let mut lines = Vec::new();

    let status = if state.games.run_browser.loading {
        "LOADING"
    } else if state.games.run_browser.last_error.is_some() {
        "ERROR"
    } else {
        "READY"
    };
    lines.push(Line::from(vec![
        Span::styled("status: ", label_style),
        Span::styled(
            status,
            if state.games.run_browser.last_error.is_some() {
                warn_style
            } else {
                value_style
            },
        ),
    ]));

    if let Some(err) = state.games.run_browser.last_error.as_ref() {
        lines.push(Line::from(vec![
            Span::styled("error: ", warn_style),
            Span::styled(trim_to_width(err, max_width), value_style),
        ]));
    }

    if state.games.run_browser.loading {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Loading runs...", dim_style)));
        return lines;
    }

    lines.push(Line::from(""));
    if state.games.run_browser.entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "No runs found in runs/games or legacy folders.",
            dim_style,
        )));
    } else {
        for (idx, entry) in state.games.run_browser.entries.iter().enumerate() {
            let style = if idx == state.games.run_browser.selected {
                selected_style
            } else {
                value_style
            };
            let prefix = if idx == state.games.run_browser.selected {
                "›"
            } else {
                " "
            };
            let text = format!(
                "{prefix} {}",
                trim_to_width(&entry.label, max_width.saturating_sub(2))
            );
            lines.push(Line::from(Span::styled(text, style)));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Enter load · R refresh · Esc close",
        dim_style,
    )));

    lines
}

pub fn render(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " RUN BROWSER ",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = build_lines(state, theme, inner.width);
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    let scroll = state.games.run_browser.scroll_offset.min(max_scroll);
    lines = apply_ui_selection(
        lines,
        state.ui_selection.as_ref(),
        UiSelectionPane::GamesRunBrowserPopup,
        theme.selection_bg,
        scroll,
    );
    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));
    frame.render_widget(paragraph, inner);
}

fn trim_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    text.chars().take(max_width).collect()
}
