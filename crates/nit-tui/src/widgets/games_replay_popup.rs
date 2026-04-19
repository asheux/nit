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
use crate::widgets::text_utils::trim_to_width;

const MIN_WIDTH: u16 = 70;
const MAX_WIDTH: u16 = 120;
const MIN_HEIGHT: u16 = 20;
const MAX_HEIGHT: u16 = 40;

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = screen.width.clamp(MIN_WIDTH, MAX_WIDTH);
    let height = screen.height.clamp(MIN_HEIGHT, MAX_HEIGHT);
    (width, height)
}

pub fn pair_list(state: &AppState) -> &[(String, String)] {
    state.games.replay.pairs.as_slice()
}

pub fn build_lines(state: &AppState, theme: &Theme, inner_width: u16) -> Vec<Line<'static>> {
    let total = line_count(state);
    build_lines_window(state, theme, inner_width, 0, total)
}

pub fn render(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " MATCH REPLAY ",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let total_lines = line_count(state);
    let max_scroll = total_lines.saturating_sub(inner.height as usize);
    let scroll = state.games.replay.scroll_offset.min(max_scroll);
    let mut lines = build_lines_window(state, theme, inner.width, scroll, inner.height as usize);
    lines = apply_ui_selection(
        lines,
        state.ui_selection.as_ref(),
        UiSelectionPane::GamesReplayPopup,
        theme.selection_bg,
        scroll,
    );
    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));
    frame.render_widget(paragraph, inner);
}

/// Count the rendered lines without actually building them. Used by the
/// scroll-handling hot path to compute `max_scroll` cheaply — rebuilding the
/// full `Vec<Line>` on every wheel tick was making scroll feel sluggish.
pub fn line_count(state: &AppState) -> usize {
    let mut count = 1; // status line
    if state.games.replay.last_error.is_some() {
        count += 1;
    }
    if state.games.replay.loading {
        return count + 2; // blank + loading line
    }
    if !state.games.replay.lines.is_empty() {
        if state.games.replay.title.is_some() {
            count += 2; // blank + title
        }
        count += 1; // blank before rounds
        count += state.games.replay.lines.len();
        count += 2; // blank + footer
        return count;
    }
    count += 1; // blank before selection list
    let pairs = pair_list(state);
    if pairs.is_empty() {
        count += 1;
    } else {
        count += 1; // "Select a matchup"
        count += pairs.len();
    }
    count += 2; // blank + footer
    count
}

fn build_lines_window(
    state: &AppState,
    theme: &Theme,
    inner_width: u16,
    start: usize,
    height: usize,
) -> Vec<Line<'static>> {
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
    let end = start.saturating_add(height.max(1));
    let mut idx = 0usize;
    let push = |line: Line<'static>, idx: &mut usize, lines: &mut Vec<Line<'static>>| {
        if *idx >= start && *idx < end {
            lines.push(line);
        }
        *idx = idx.saturating_add(1);
    };

    let status = if state.games.replay.loading {
        "LOADING"
    } else if state.games.replay.last_error.is_some() {
        "ERROR"
    } else if !state.games.replay.lines.is_empty() {
        "READY"
    } else {
        "SELECT"
    };
    push(
        Line::from(vec![
            Span::styled("status: ", label_style),
            Span::styled(
                status,
                if state.games.replay.last_error.is_some() {
                    warn_style
                } else {
                    value_style
                },
            ),
        ]),
        &mut idx,
        &mut lines,
    );

    if let Some(err) = state.games.replay.last_error.as_ref() {
        push(
            Line::from(vec![
                Span::styled("error: ", warn_style),
                Span::styled(trim_to_width(err, max_width), value_style),
            ]),
            &mut idx,
            &mut lines,
        );
    }

    if state.games.replay.loading {
        push(Line::from(""), &mut idx, &mut lines);
        push(
            Line::from(Span::styled("Loading replay...", dim_style)),
            &mut idx,
            &mut lines,
        );
        return lines;
    }

    if !state.games.replay.lines.is_empty() {
        if let Some(title) = state.games.replay.title.as_ref() {
            push(Line::from(""), &mut idx, &mut lines);
            push(
                Line::from(Span::styled(trim_to_width(title, max_width), value_style)),
                &mut idx,
                &mut lines,
            );
        }
        push(Line::from(""), &mut idx, &mut lines);

        let lines_start = idx;
        let total_rounds = state.games.replay.lines.len();
        let lines_end = lines_start.saturating_add(total_rounds);
        if end > lines_start && start < lines_end {
            let slice_start = start.saturating_sub(lines_start).min(total_rounds);
            let slice_end = end.saturating_sub(lines_start).min(total_rounds);
            for line in &state.games.replay.lines[slice_start..slice_end] {
                push(
                    Line::from(Span::styled(trim_to_width(line, max_width), value_style)),
                    &mut idx,
                    &mut lines,
                );
            }
        }
        idx = lines_end;

        push(Line::from(""), &mut idx, &mut lines);
        push(
            Line::from(Span::styled("Esc close · ↑/↓ scroll", dim_style)),
            &mut idx,
            &mut lines,
        );
        return lines;
    }

    push(Line::from(""), &mut idx, &mut lines);
    let pairs = pair_list(state);
    if pairs.is_empty() {
        push(
            Line::from(Span::styled("No pairwise results available.", dim_style)),
            &mut idx,
            &mut lines,
        );
    } else {
        push(
            Line::from(Span::styled("Select a matchup:", label_style)),
            &mut idx,
            &mut lines,
        );
        let pairs_start = idx;
        let pairs_end = pairs_start.saturating_add(pairs.len());
        if end > pairs_start && start < pairs_end {
            let slice_start = start.saturating_sub(pairs_start).min(pairs.len());
            let slice_end = end.saturating_sub(pairs_start).min(pairs.len());
            for (offset, (a, b)) in pairs[slice_start..slice_end].iter().enumerate() {
                let pair_idx = slice_start + offset;
                let is_selected = pair_idx == state.games.replay.selected_index;
                let style = if is_selected { selected_style } else { value_style };
                let prefix = if is_selected { "›" } else { " " };
                let text = format!("{prefix} {a} vs {b}");
                push(
                    Line::from(Span::styled(trim_to_width(&text, max_width), style)),
                    &mut idx,
                    &mut lines,
                );
            }
        }
        idx = pairs_end;
    }

    push(Line::from(""), &mut idx, &mut lines);
    push(
        Line::from(Span::styled(
            "Enter replay · Esc close · R reset",
            dim_style,
        )),
        &mut idx,
        &mut lines,
    );
    lines
}
