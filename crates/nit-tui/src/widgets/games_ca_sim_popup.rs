use nit_core::{AppState, UiSelectionPane};
use nit_games::config::StrategySpecKind;
use nit_games::game::Action;
use nit_games::strategy::{decode_ca_rule_table, run_shrinking_ca};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::theme::Theme;
use crate::widgets::text_selection::apply_ui_selection;

const MIN_WIDTH: u16 = 72;
const MIN_HEIGHT: u16 = 16;

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = screen.width.clamp(MIN_WIDTH, 118);
    let height = screen.height.clamp(MIN_HEIGHT, 32);
    (width, height)
}

pub fn build_lines(state: &AppState, theme: &Theme, inner_width: u16) -> Vec<Line<'static>> {
    let max_width = inner_width.max(1) as usize;
    let (left_width, right_width, gap) = split_columns(max_width);
    let (left_lines, right_lines) = build_columns(state, theme, left_width, right_width);
    merge_columns(left_lines, right_lines, left_width, right_width, gap)
}

pub fn build_columns(
    state: &AppState,
    theme: &Theme,
    left_width: usize,
    right_width: usize,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let header_style = Style::default()
        .fg(theme.title)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let warn_style = Style::default()
        .fg(theme.warning)
        .add_modifier(Modifier::BOLD);
    let number_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);

    let mut left_lines: Vec<Line<'static>> = Vec::new();
    let mut right_lines: Vec<Line<'static>> = Vec::new();

    let status = if state.games.ca_sim.last_error.is_some() {
        "ERROR"
    } else if state.games.ca_sim.definition.is_some() && state.games.ca_sim.input.is_some() {
        "READY"
    } else {
        "IDLE"
    };
    left_lines.push(Line::from(vec![
        Span::styled("status: ", label_style),
        Span::styled(
            status,
            if state.games.ca_sim.last_error.is_some() {
                warn_style
            } else {
                number_style
            },
        ),
    ]));

    if let Some(source) = state.games.ca_sim.source_label.as_deref() {
        left_lines.push(Line::from(vec![
            Span::styled("source: ", label_style),
            Span::styled(source.to_string(), value_style),
        ]));
    }

    if let Some(err) = state.games.ca_sim.last_error.as_ref() {
        left_lines.push(Line::from(vec![
            Span::styled("error: ", warn_style),
            Span::styled(trim_to_width(err, left_width), value_style),
        ]));
    }

    let (Some(def), Some(input)) = (
        state.games.ca_sim.definition.as_ref(),
        state.games.ca_sim.input,
    ) else {
        left_lines.push(Line::from(""));
        left_lines.push(Line::from(Span::styled(
            "Use :games ca [run|config] <input> [steps] [strategy_id]",
            dim_style,
        )));
        left_lines.push(Line::from(Span::styled(
            "or :games ca {n,k,r} <input> [steps] (t defaults to 10)",
            dim_style,
        )));
        return (left_lines, right_lines);
    };

    let StrategySpecKind::Ca { n, k, r, t } = &def.kind else {
        left_lines.push(Line::from(""));
        left_lines.push(Line::from(Span::styled(
            "Selected strategy is not a CA strategy.",
            warn_style,
        )));
        return (left_lines, right_lines);
    };

    let Some(two_r) = parse_two_r(*r) else {
        left_lines.push(Line::from(""));
        left_lines.push(Line::from(Span::styled(
            "error: invalid radius r (requires r >= 0 and IntegerQ[2r])",
            warn_style,
        )));
        return (left_lines, right_lines);
    };

    let symbols = (*k).max(2);
    let step_limit = state.games.ca_sim.steps_override.unwrap_or(*t);
    let rule_table = decode_ca_rule_table(*n, symbols, two_r);
    let input_digits = digits_in_base(input, symbols);
    let run = run_shrinking_ca(&rule_table, symbols, two_r, step_limit, &input_digits);
    let rows = pad_rows_for_plot(&run.rows, two_r);
    let output_action = if run.output_symbol == 0 {
        Action::Cooperate
    } else {
        Action::Defect
    };

    left_lines.push(Line::from(""));
    left_lines.push(Line::from(vec![
        Span::styled("strategy: ", label_style),
        Span::styled(def.id.clone(), value_style),
    ]));
    left_lines.push(Line::from(vec![
        Span::styled("input: ", label_style),
        Span::styled(input.to_string(), value_style),
        Span::styled(" (base ", label_style),
        Span::styled(symbols.to_string(), value_style),
        Span::styled(")", label_style),
    ]));
    left_lines.push(Line::from(vec![
        Span::styled("rule: ", label_style),
        Span::styled(
            format!(
                "n={}  k={}  r={}  t={}",
                n,
                symbols,
                format_radius(two_r),
                t
            ),
            value_style,
        ),
    ]));
    if let Some(steps_override) = state.games.ca_sim.steps_override {
        let mut spans = vec![
            Span::styled("steps: ", label_style),
            Span::styled(step_limit.to_string(), value_style),
        ];
        if steps_override != *t {
            spans.push(Span::styled(" (override)", dim_style));
        }
        left_lines.push(Line::from(spans));
    }
    left_lines.push(Line::from(vec![
        Span::styled("rule_space: ", label_style),
        Span::styled(ca_rule_space_label(symbols, two_r), value_style),
    ]));
    left_lines.push(Line::from(vec![
        Span::styled("input_semantics: ", label_style),
        Span::styled(
            "row = digits(input, base k); gameplay uses Flatten[history] for k=2".to_string(),
            value_style,
        ),
    ]));
    left_lines.push(Line::from(vec![
        Span::styled("output: ", label_style),
        Span::styled("last cell of final row".to_string(), value_style),
    ]));

    left_lines.push(Line::from(""));
    left_lines.push(Line::from(Span::styled("Simulation", header_style)));
    left_lines.push(Line::from(vec![
        Span::styled("input digits: ", label_style),
        Span::styled(digits_to_string(&input_digits), value_style),
    ]));
    left_lines.push(Line::from(vec![
        Span::styled("rows: ", label_style),
        Span::styled(run.rows.len().to_string(), value_style),
        Span::styled("  simulated_steps: ", label_style),
        Span::styled(run.steps_executed.to_string(), value_style),
        Span::styled(" / ", label_style),
        Span::styled(step_limit.to_string(), value_style),
    ]));
    left_lines.push(Line::from(vec![
        Span::styled("result: ", label_style),
        Span::styled(
            format!(
                "symbol={} -> {}",
                run.output_symbol,
                output_action.as_char()
            ),
            value_style,
        ),
    ]));
    if run.stopped_early {
        left_lines.push(Line::from(Span::styled(
            "note: stopped early (row length <= 2r)",
            dim_style,
        )));
    }
    if let Some(last_row) = run.rows.last() {
        left_lines.push(Line::from(vec![
            Span::styled("final row: ", label_style),
            Span::styled(
                trim_to_width(&digits_to_string(last_row), left_width.saturating_sub(11)),
                value_style,
            ),
        ]));
    }

    if !rows.is_empty() {
        if right_width > 0 {
            right_lines.push(Line::from(Span::styled("Evolution", header_style)));
            right_lines.extend(build_grid_lines(&rows, right_width.max(1), theme));
            right_lines.push(Line::from(""));
            right_lines.extend(build_legend_lines(theme, run.output_symbol, output_action));
        } else {
            left_lines.push(Line::from(""));
            left_lines.push(Line::from(Span::styled("Evolution", header_style)));
            left_lines.extend(build_grid_lines(&rows, left_width.max(1), theme));
            left_lines.push(Line::from(""));
            left_lines.extend(build_legend_lines(theme, run.output_symbol, output_action));
        }
    }

    left_lines.push(Line::from(""));
    left_lines.push(Line::from(Span::styled(
        "Esc close · ↑/↓ scroll · R reset scroll",
        dim_style,
    )));

    (left_lines, right_lines)
}

pub fn render(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    if !state.games.ca_sim.open {
        return;
    }
    frame.render_widget(Clear, area);

    let border_style = Style::default().fg(theme.border_focused);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(theme.background))
        .title(Span::styled(
            " CA SIMULATOR ",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (left_area, right_area) = layout_for_ca_sim(inner);
    if let Some(right_area) = right_area {
        let right_block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .style(Style::default().bg(theme.background));
        let right_inner = right_block.inner(right_area);
        frame.render_widget(right_block, right_area);

        let (left_lines, right_lines) = build_columns(
            state,
            theme,
            left_area.width.max(1) as usize,
            right_inner.width.max(1) as usize,
        );
        let content_height = left_area.height.min(right_inner.height) as usize;
        let max_lines = left_lines.len().max(right_lines.len());
        let max_scroll = max_lines.saturating_sub(content_height);
        let scroll = state.games.ca_sim.scroll_offset.min(max_scroll);

        let left_visible: Vec<Line> = left_lines
            .into_iter()
            .skip(scroll)
            .take(left_area.height as usize)
            .collect();
        let left_visible = apply_ui_selection(
            left_visible,
            state.ui_selection.as_ref(),
            UiSelectionPane::GamesCaSimPopupLeft,
            theme.selection_bg,
            scroll,
        );
        let left_paragraph = Paragraph::new(left_visible)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .wrap(Wrap { trim: true });
        frame.render_widget(left_paragraph, left_area);

        if right_inner.width > 0 && right_inner.height > 0 {
            let right_visible: Vec<Line> = right_lines
                .into_iter()
                .skip(scroll)
                .take(right_inner.height as usize)
                .collect();
            let right_visible = apply_ui_selection(
                right_visible,
                state.ui_selection.as_ref(),
                UiSelectionPane::GamesCaSimPopupRight,
                theme.selection_bg,
                scroll,
            );
            let right_paragraph = Paragraph::new(right_visible)
                .style(Style::default().fg(theme.foreground).bg(theme.background))
                .wrap(Wrap { trim: true });
            frame.render_widget(right_paragraph, right_inner);
        }
    } else {
        let (lines, _) = build_columns(state, theme, inner.width.max(1) as usize, 0);
        let height = inner.height as usize;
        let max_scroll = lines.len().saturating_sub(height);
        let scroll = state.games.ca_sim.scroll_offset.min(max_scroll);
        let visible: Vec<Line> = lines.into_iter().skip(scroll).take(height).collect();
        let visible = apply_ui_selection(
            visible,
            state.ui_selection.as_ref(),
            UiSelectionPane::GamesCaSimPopupLeft,
            theme.selection_bg,
            scroll,
        );
        let paragraph = Paragraph::new(visible)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, inner);
    }
}

pub fn layout_for_ca_sim(inner: Rect) -> (Rect, Option<Rect>) {
    let min_left = 32u16;
    let min_right_inner = 24u16;
    let total = inner.width;
    if total < min_left + min_right_inner + 2 {
        return (inner, None);
    }
    let mut right_inner = (total / 2).max(min_right_inner);
    if total < min_left + right_inner + 2 {
        right_inner = total.saturating_sub(min_left + 2);
    }
    if right_inner < min_right_inner {
        return (inner, None);
    }
    let right_total = right_inner + 2;
    let left_total = total.saturating_sub(right_total);
    if left_total < min_left {
        return (inner, None);
    }
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(left_total),
            Constraint::Length(right_total),
        ])
        .split(inner);
    (cols[0], Some(cols[1]))
}

fn split_columns(total_width: usize) -> (usize, usize, usize) {
    let gap = 2usize;
    let min_right = 24usize;
    let min_left = 32usize;
    if total_width < min_left + min_right + gap {
        return (total_width, 0, 0);
    }
    let right = (total_width / 2).max(min_right);
    let left = total_width.saturating_sub(right + gap);
    if left < min_left {
        (total_width, 0, 0)
    } else {
        (left, right, gap)
    }
}

fn merge_columns(
    left: Vec<Line<'static>>,
    right: Vec<Line<'static>>,
    left_width: usize,
    right_width: usize,
    gap: usize,
) -> Vec<Line<'static>> {
    if right_width == 0 || right.is_empty() {
        return left;
    }
    let left_len = left.len();
    let right_len = right.len();
    let pad_top = left_len.saturating_sub(right_len) / 2;
    let mut right_padded: Vec<Line<'static>> = Vec::with_capacity(right_len + pad_top);
    for _ in 0..pad_top {
        right_padded.push(Line::from(""));
    }
    right_padded.extend(right);
    let max_lines = left_len.max(right_padded.len());
    let mut merged = Vec::with_capacity(max_lines);
    for idx in 0..max_lines {
        let mut spans = Vec::new();
        let left_line = left.get(idx).cloned().unwrap_or_else(|| Line::from(""));
        let left_line_width = line_width(&left_line);
        spans.extend(left_line.spans);
        let pad = left_width.saturating_sub(left_line_width);
        spans.push(Span::raw(" ".repeat(pad + gap)));
        if let Some(right_line) = right_padded.get(idx) {
            spans.extend(right_line.spans.clone());
        }
        merged.push(Line::from(spans));
    }
    merged
}

fn line_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| span.content.chars().count())
        .sum()
}

fn build_grid_lines(rows: &[Vec<i16>], max_width: usize, theme: &Theme) -> Vec<Line<'static>> {
    if rows.is_empty() || max_width == 0 {
        return Vec::new();
    }
    let width = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    if width == 0 {
        return Vec::new();
    }
    let label_width = 4usize;
    let cell_width = 2usize;
    let ellipsis_width = 3usize;
    let available_no_ellipsis = max_width.saturating_sub(label_width);
    let max_cells_no_ellipsis = available_no_ellipsis / cell_width;
    if max_cells_no_ellipsis == 0 {
        return Vec::new();
    }
    let needs_ellipsis = width > max_cells_no_ellipsis;
    let available = if needs_ellipsis {
        max_width.saturating_sub(label_width + ellipsis_width)
    } else {
        available_no_ellipsis
    };
    let max_cells = available / cell_width;
    if max_cells == 0 {
        return Vec::new();
    }
    let view_width = width.min(max_cells).max(1);
    let start = if needs_ellipsis {
        width.saturating_sub(view_width)
    } else {
        0
    };
    let end = start + view_width;
    let mut lines = Vec::new();
    for (idx, row) in rows.iter().enumerate() {
        let mut spans = Vec::new();
        spans.push(Span::styled(
            format!("t{idx:02} "),
            Style::default().fg(theme.border),
        ));
        if needs_ellipsis {
            spans.push(Span::styled("...", Style::default().fg(theme.border)));
        }
        for cell_idx in start..end {
            let value = row.get(cell_idx).copied().unwrap_or(-1);
            spans.push(Span::styled(
                " ".repeat(cell_width),
                ca_symbol_style(value, theme),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn build_legend_lines(
    theme: &Theme,
    output_symbol: u8,
    output_action: Action,
) -> Vec<Line<'static>> {
    let mut legend = Vec::new();
    legend.push(Span::styled(
        "legend: ",
        Style::default().fg(theme.title).add_modifier(Modifier::DIM),
    ));
    let mut push_cell = |label: &str, value: i16| {
        legend.push(Span::raw(" "));
        legend.push(Span::styled(
            format!(" {label} "),
            legend_cell_style(value, theme),
        ));
    };
    push_cell("-1", -1);
    push_cell("0", 0);
    push_cell("1", 1);

    let output_line = Line::from(vec![
        Span::styled(
            "output: ",
            Style::default().fg(theme.title).add_modifier(Modifier::DIM),
        ),
        Span::styled(
            format!("{output_symbol} (= {})", output_action.as_char()),
            Style::default().fg(theme.foreground),
        ),
    ]);
    vec![Line::from(legend), output_line]
}

fn ca_symbol_style(value: i16, theme: &Theme) -> Style {
    let bg = match value {
        -1 => Color::Rgb(211, 211, 211),
        0 => Color::White,
        1 => theme.accent,
        2 => theme.warning,
        3 => theme.title,
        _ => theme.selection_bg,
    };
    Style::default().bg(bg)
}

fn legend_cell_style(value: i16, theme: &Theme) -> Style {
    let mut style = ca_symbol_style(value, theme);
    let fg = match value {
        -1 | 0 => Color::Black,
        _ => theme.foreground,
    };
    style = style.fg(fg).add_modifier(Modifier::BOLD);
    style
}

fn pad_rows_for_plot(rows: &[Vec<u8>], two_r: u32) -> Vec<Vec<i16>> {
    if rows.is_empty() {
        return Vec::new();
    }
    let width = rows.first().map(|row| row.len()).unwrap_or(0);
    let left_unit = two_r.saturating_add(1) / 2;
    let right_unit = two_r / 2;
    let mut out = Vec::with_capacity(rows.len());
    for (idx, row) in rows.iter().enumerate() {
        let left_pad = idx.saturating_mul(left_unit as usize);
        let right_pad = idx.saturating_mul(right_unit as usize);
        let mut padded = Vec::with_capacity(width.max(left_pad + row.len() + right_pad));
        padded.extend(std::iter::repeat_n(-1, left_pad));
        padded.extend(row.iter().map(|&value| value as i16));
        padded.extend(std::iter::repeat_n(-1, right_pad));
        if padded.len() < width {
            padded.extend(std::iter::repeat_n(-1, width - padded.len()));
        }
        if padded.len() > width {
            padded.truncate(width);
        }
        out.push(padded);
    }
    out
}

fn digits_in_base(input: u64, base: u8) -> Vec<u8> {
    let base_u64 = base.max(2) as u64;
    if input == 0 {
        return vec![0];
    }
    let mut value = input;
    let mut digits = Vec::new();
    while value > 0 {
        digits.push((value % base_u64) as u8);
        value /= base_u64;
    }
    digits.reverse();
    digits
}

fn digits_to_string(digits: &[u8]) -> String {
    digits.iter().map(|&d| symbol_char(d)).collect()
}

fn symbol_char(symbol: u8) -> char {
    if symbol < 10 {
        (b'0' + symbol) as char
    } else {
        (b'A' + (symbol - 10)) as char
    }
}

fn parse_two_r(radius: f32) -> Option<u32> {
    if !radius.is_finite() || radius < 0.0 {
        return None;
    }
    let doubled = (radius as f64) * 2.0;
    let rounded = doubled.round();
    if (doubled - rounded).abs() > 1e-6 {
        return None;
    }
    if rounded < 0.0 || rounded > u32::MAX as f64 {
        return None;
    }
    Some(rounded as u32)
}

fn format_radius(two_r: u32) -> String {
    if two_r.is_multiple_of(2) {
        (two_r / 2).to_string()
    } else {
        format!("{two_r}/2")
    }
}

fn ca_rule_space_label(k: u8, two_r: u32) -> String {
    let base = k as u128;
    let neighborhood = two_r.saturating_add(1);
    let inner = pow_u128_checked(base, neighborhood);
    match inner.and_then(|exp| {
        if exp > u32::MAX as u128 {
            None
        } else {
            pow_u128_checked(base, exp as u32)
        }
    }) {
        Some(value) => format!("{k}^({k}^{neighborhood}) = {value}"),
        None => format!("{k}^({k}^{neighborhood})"),
    }
}

fn pow_u128_checked(base: u128, exp: u32) -> Option<u128> {
    let mut value: u128 = 1;
    for _ in 0..exp {
        value = value.checked_mul(base)?;
    }
    Some(value)
}

fn trim_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let mut out = String::new();
    for ch in text.chars().take(max_width) {
        out.push(ch);
    }
    out
}

#[cfg(test)]
#[path = "tests/games_ca_sim_popup.rs"]
mod tests;
