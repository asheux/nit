use nit_core::AppState;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::theme::Theme;

const MIN_WIDTH: u16 = 70;
const MIN_HEIGHT: u16 = 20;
const PANEL_WIDTH: usize = 18;
const PANEL_GAP: usize = 2;
const RESERVED_LINES: usize = 8;

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = ((screen.width as u32).saturating_mul(9) / 10) as u16;
    let height = ((screen.height as u32).saturating_mul(9) / 10) as u16;
    let width = width.max(MIN_WIDTH.min(screen.width)).min(screen.width);
    let height = height.max(MIN_HEIGHT.min(screen.height)).min(screen.height);
    (width, height)
}

pub fn render(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " MATCH HISTORY PLOTS ",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = build_lines(state, theme, inner);
    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

fn build_lines(state: &AppState, theme: &Theme, inner: Rect) -> Vec<Line<'static>> {
    let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let warn_style = Style::default()
        .fg(theme.warning)
        .add_modifier(Modifier::BOLD);
    let zero_style = Style::default().bg(Color::White).fg(Color::White);
    let one_style = Style::default().bg(theme.accent).fg(theme.accent);
    let empty_cell_style = Style::default().bg(Color::DarkGray).fg(Color::DarkGray);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let entries = state.games.match_history.entries.as_slice();
    let total = entries.len();

    lines.push(Line::from(vec![
        Span::styled("status: ", label_style),
        Span::styled(if total == 0 { "EMPTY" } else { "READY" }, value_style),
        Span::styled("  ", dim_style),
        Span::styled("matches: ", label_style),
        Span::styled(total.to_string(), value_style),
    ]));

    if let Some(err) = state.games.match_history.last_error.as_ref() {
        lines.push(Line::from(vec![
            Span::styled("note: ", warn_style),
            Span::styled(err.clone(), value_style),
        ]));
    }

    if total == 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("No completed matches to plot yet.", dim_style),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Esc", value_style),
            Span::styled(" close", dim_style),
        ]));
        return lines;
    }

    let capacity = panel_capacity(inner.width);
    let mut start = state.games.match_history.column_offset.min(total.saturating_sub(1));
    if total > capacity {
        start = start.min(total - capacity);
    } else {
        start = 0;
    }
    let end = (start + capacity).min(total);
    let visible = &entries[start..end];
    let available_rows = (inner.height as usize).saturating_sub(RESERVED_LINES);
    let max_rounds_in_view = visible
        .iter()
        .map(|entry| entry.outcomes_prefix.len())
        .max()
        .unwrap_or(0);
    let shown_rounds = available_rows.min(max_rounds_in_view);
    let clipped_for_height = max_rounds_in_view > shown_rounds;

    lines.push(Line::from(vec![
        Span::styled("range: ", label_style),
        Span::styled(
            format!("{}-{} of {}", start + 1, end, total),
            value_style,
        ),
        Span::styled("  ", dim_style),
        Span::styled("layout: ", label_style),
        Span::styled("left → right", value_style),
    ]));
    lines.push(Line::from(""));

    let pad_line = |text: &str| -> String { pad_to_width(text, PANEL_WIDTH) };
    let mut header_idx = Vec::new();
    let mut header_pair = Vec::new();
    let mut header_cols = Vec::new();
    for (i, entry) in visible.iter().enumerate() {
        if i > 0 {
            header_idx.push(Span::raw(" ".repeat(PANEL_GAP)));
            header_pair.push(Span::raw(" ".repeat(PANEL_GAP)));
            header_cols.push(Span::raw(" ".repeat(PANEL_GAP)));
        }
        header_idx.push(Span::styled(
            pad_line(&format!("#{} / {}", entry.match_index, entry.total_matches)),
            label_style,
        ));
        header_pair.push(Span::styled(
            pad_line(&truncate_text(&format!("{} vs {}", entry.a, entry.b), PANEL_WIDTH)),
            value_style,
        ));
        header_cols.push(Span::styled(
            pad_line("r   A  B"),
            dim_style.add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::from(header_idx));
    lines.push(Line::from(header_pair));
    lines.push(Line::from(header_cols));

    for round_idx in 0..shown_rounds {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (panel_idx, entry) in visible.iter().enumerate() {
            if panel_idx > 0 {
                spans.push(Span::raw(" ".repeat(PANEL_GAP)));
            }
            spans.push(Span::styled(format!("{:>3} ", round_idx + 1), dim_style));
            let (a_bit, b_bit) = decode_outcome(entry.outcomes_prefix.as_bytes().get(round_idx).copied());
            spans.push(Span::styled(
                "  ",
                if a_bit == Some(1) {
                    one_style
                } else if a_bit == Some(0) {
                    zero_style
                } else {
                    empty_cell_style
                },
            ));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                "  ",
                if b_bit == Some(1) {
                    one_style
                } else if b_bit == Some(0) {
                    zero_style
                } else {
                    empty_cell_style
                },
            ));
            let consumed = 4 + 2 + 1 + 2;
            let pad = PANEL_WIDTH.saturating_sub(consumed);
            if pad > 0 {
                spans.push(Span::raw(" ".repeat(pad)));
            }
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Legend: ", label_style),
        Span::styled("0", dim_style),
        Span::styled("=", dim_style),
        Span::styled("white", value_style),
        Span::styled("  ", dim_style),
        Span::styled("1", dim_style),
        Span::styled("=", dim_style),
        Span::styled("cyan", value_style),
        Span::styled("  ", dim_style),
        Span::styled("A/B columns", dim_style),
    ]));

    let max_rounds_total = visible
        .iter()
        .map(|entry| entry.rounds_total as usize)
        .max()
        .unwrap_or(0);
    let clipped_for_capture = visible
        .iter()
        .any(|entry| entry.rounds_total as usize > entry.outcomes_prefix.len());
    let mut footer = String::from("Esc close | ←/→ pan");
    if clipped_for_height {
        footer.push_str(&format!(" | showing first {} rounds (height)", shown_rounds));
    } else if shown_rounds > 0 {
        footer.push_str(&format!(" | rounds shown: {}", shown_rounds.min(max_rounds_total)));
    }
    if clipped_for_capture {
        footer.push_str(" | preview capture capped");
    }
    lines.push(Line::from(Span::styled(footer, dim_style)));
    lines
}

fn panel_capacity(inner_width: u16) -> usize {
    let width = inner_width.max(1) as usize;
    ((width + PANEL_GAP) / (PANEL_WIDTH + PANEL_GAP)).max(1)
}

fn decode_outcome(outcome: Option<u8>) -> (Option<u8>, Option<u8>) {
    match outcome {
        Some(b'0') => (Some(0), Some(0)),
        Some(b'1') => (Some(0), Some(1)),
        Some(b'2') => (Some(1), Some(0)),
        Some(b'3') => (Some(1), Some(1)),
        _ => (None, None),
    }
}

fn pad_to_width(text: &str, width: usize) -> String {
    let mut out = truncate_text(text, width);
    let len = out.chars().count();
    if len < width {
        out.push_str(&" ".repeat(width - len));
    }
    out
}

fn truncate_text(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let len = text.chars().count();
    if len <= width {
        return text.to_string();
    }
    if width <= 3 {
        return text.chars().take(width).collect();
    }
    let mut out: String = text.chars().take(width - 3).collect();
    out.push_str("...");
    out
}
