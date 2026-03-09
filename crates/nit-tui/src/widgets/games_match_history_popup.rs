use nit_core::{AppState, UiSelectionPane};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::theme::Theme;
use crate::widgets::text_selection::apply_ui_selection;
use std::collections::{HashMap, HashSet};

const MIN_WIDTH: u16 = 70;
const MIN_HEIGHT: u16 = 20;
const PANEL_WIDTH: usize = 18;
const PANEL_GAP: usize = 2;
const RESERVED_LINES: usize = 8;
const DEFAULT_ROUND_LIMIT: usize = 30;
const CELL_GLYPH: &str = "▀▀";
const CELL_EMPTY: &str = "  ";

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = ((screen.width as u32).saturating_mul(9) / 10) as u16;
    let height = ((screen.height as u32).saturating_mul(9) / 10) as u16;
    let width = width.max(MIN_WIDTH.min(screen.width)).min(screen.width);
    let height = height.max(MIN_HEIGHT.min(screen.height)).min(screen.height);
    (width, height)
}

pub fn max_column_offset(total_matches: usize, inner_width: u16) -> usize {
    total_matches.saturating_sub(panel_capacity(inner_width))
}

pub fn max_round_limit(entries: &[nit_games::MatchHistoryPreview]) -> usize {
    entries
        .iter()
        .map(|entry| entry.preview_rounds())
        .max()
        .unwrap_or(0)
}

pub fn default_round_limit(total_rounds: usize) -> usize {
    total_rounds.min(DEFAULT_ROUND_LIMIT)
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
    let lines = apply_ui_selection(
        lines,
        state.ui_selection.as_ref(),
        UiSelectionPane::GamesMatchHistoryPopup,
        theme.selection_bg,
        0,
    );
    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

pub fn build_lines(state: &AppState, theme: &Theme, inner: Rect) -> Vec<Line<'static>> {
    let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);
    let dim_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let warn_style = Style::default()
        .fg(theme.warning)
        .add_modifier(Modifier::BOLD);
    let zero_style = Style::default().fg(Color::White).bg(theme.background);
    let one_style = Style::default().fg(theme.accent).bg(theme.background);
    let empty_cell_style = Style::default().fg(Color::DarkGray).bg(theme.background);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let entries = state.games.match_history.entries.as_slice();
    let total = if state.games.match_history.total_entries > 0 {
        state.games.match_history.total_entries
    } else {
        entries.len()
    };
    let loaded_start = state
        .games
        .match_history
        .loaded_start
        .min(total.saturating_sub(1));
    let loaded_end = loaded_start.saturating_add(entries.len()).min(total);

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
        lines.push(Line::from(vec![Span::styled(
            "No completed matches to plot yet.",
            dim_style,
        )]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Esc", value_style),
            Span::styled(" close", dim_style),
        ]));
        return lines;
    }

    let capacity = panel_capacity(inner.width);
    let mut start = state
        .games
        .match_history
        .column_offset
        .min(total.saturating_sub(1));
    if total > capacity {
        start = start.min(total - capacity);
    } else {
        start = 0;
    }
    let end = (start + capacity).min(total);
    let visible_start = start.max(loaded_start);
    let visible_end = end.min(loaded_end);
    let visible = if visible_start < visible_end {
        &entries[(visible_start - loaded_start)..(visible_end - loaded_start)]
    } else {
        &[]
    };
    let (aliases, alias_legend) = strategy_aliases(state, visible);
    let available_rows = (inner.height as usize).saturating_sub(RESERVED_LINES);
    let max_rounds_in_view = max_round_limit(visible);
    let default_limit = default_round_limit(max_rounds_in_view);
    let round_limit = state
        .games
        .match_history
        .round_limit
        .unwrap_or(default_limit)
        .min(max_rounds_in_view);
    let round_start = round_limit.saturating_sub(available_rows);

    lines.push(Line::from(vec![
        Span::styled("range: ", label_style),
        Span::styled(format!("{}-{} of {}", start + 1, end, total), value_style),
        Span::styled("  ", dim_style),
        Span::styled("layout: ", label_style),
        Span::styled("left → right", value_style),
    ]));
    if visible.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("status: ", label_style),
            Span::styled("loading visible slice…", dim_style),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Esc", value_style),
            Span::styled(" close | ", dim_style),
            Span::styled("←/→", value_style),
            Span::styled(" pan | ", dim_style),
            Span::styled("+/-", value_style),
            Span::styled(" rounds (default 30)", dim_style),
        ]));
        return lines;
    }
    if !alias_legend.is_empty() {
        let legend_text = format!(
            "types: {}",
            alias_legend
                .iter()
                .map(|(alias, id)| format!("{alias}={id}"))
                .collect::<Vec<_>>()
                .join("  ")
        );
        lines.push(Line::from(vec![Span::styled(
            truncate_text(&legend_text, inner.width.max(1) as usize),
            dim_style,
        )]));
    }
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
            pad_line(&truncate_text(
                &format!(
                    "{} vs {}",
                    aliases.get(entry.a.as_str()).unwrap_or(&entry.a),
                    aliases.get(entry.b.as_str()).unwrap_or(&entry.b)
                ),
                PANEL_WIDTH,
            )),
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

    for round_idx in round_start..round_limit {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (panel_idx, entry) in visible.iter().enumerate() {
            if panel_idx > 0 {
                spans.push(Span::raw(" ".repeat(PANEL_GAP)));
            }
            spans.push(Span::styled(format!("{:>3} ", round_idx + 1), dim_style));
            let (a_bit, b_bit) =
                decode_outcome(entry.preview_outcomes().as_bytes().get(round_idx).copied());
            spans.push(Span::styled(
                history_cell_text(a_bit),
                history_cell_style(a_bit, zero_style, one_style, empty_cell_style),
            ));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                history_cell_text(b_bit),
                history_cell_style(b_bit, zero_style, one_style, empty_cell_style),
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
        Span::styled(CELL_GLYPH, zero_style),
        Span::styled(" ", dim_style),
        Span::styled("white", value_style),
        Span::styled("  ", dim_style),
        Span::styled("1", dim_style),
        Span::styled("=", dim_style),
        Span::styled(CELL_GLYPH, one_style),
        Span::styled(" ", dim_style),
        Span::styled("accent", value_style),
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
        .any(|entry| entry.rounds_total as usize > entry.preview_rounds());
    let mut footer = String::from("Esc close | ←/→ pan");
    if round_limit > 0 {
        let shown = round_limit.saturating_sub(round_start);
        footer.push_str(&format!(
            " | rounds {}-{} of {} (shown {})",
            round_start + 1,
            round_limit.min(max_rounds_total),
            max_rounds_in_view.max(max_rounds_total),
            shown
        ));
    } else {
        footer.push_str(" | rounds shown: 0");
    }
    footer.push_str(" | +/- rounds (default 30)");
    if clipped_for_capture {
        footer.push_str(&format!(
            " | preview capped at {} rounds",
            nit_games::MatchHistoryPreview::DISPLAY_ROUND_CAP
        ));
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

fn strategy_aliases(
    state: &AppState,
    visible: &[nit_games::MatchHistoryPreview],
) -> (HashMap<String, String>, Vec<(String, String)>) {
    let mut strategy_ids = Vec::new();
    let mut seen_ids: HashSet<&str> = HashSet::new();
    for entry in visible {
        if seen_ids.insert(entry.a.as_str()) {
            strategy_ids.push(entry.a.as_str());
        }
        if seen_ids.insert(entry.b.as_str()) {
            strategy_ids.push(entry.b.as_str());
        }
    }
    let mut kind_by_id: HashMap<&str, &nit_games::config::StrategySpecKind> = HashMap::new();
    if let Some(run) = state.games.last_run.as_ref() {
        for def in &run.strategies {
            kind_by_id.insert(def.id.as_str(), &def.kind);
        }
    }
    let mut alias_map: HashMap<String, String> = HashMap::new();
    let mut legend = Vec::new();
    let mut used_aliases: HashSet<String> = HashSet::new();
    for id in strategy_ids {
        let family = family_code(id, kind_by_id.get(id).copied());
        let suffix = alias_suffix(id);
        let base = if suffix.is_empty() {
            family.to_string()
        } else {
            format!("{family}{suffix}")
        };
        let alias = dedupe_alias(base, &mut used_aliases);
        alias_map.insert(id.to_string(), alias.clone());
        legend.push((alias, id.to_string()));
    }
    (alias_map, legend)
}

fn family_code(id: &str, kind: Option<&nit_games::config::StrategySpecKind>) -> char {
    if let Some(kind) = kind {
        return match kind {
            nit_games::config::StrategySpecKind::Fsm { .. } => 'F',
            nit_games::config::StrategySpecKind::Ca { .. } => 'C',
            nit_games::config::StrategySpecKind::OneSidedTm { .. } => 'T',
        };
    }
    let lower = id.to_ascii_lowercase();
    if lower.starts_with("fsm") || lower.contains("_fsm") {
        'F'
    } else if lower.starts_with("ca") || lower.contains("_ca") {
        'C'
    } else if lower.starts_with("tm") || lower.contains("_tm") {
        'T'
    } else {
        lower
            .chars()
            .find(|ch| ch.is_ascii_alphanumeric())
            .map(|ch| ch.to_ascii_uppercase())
            .unwrap_or('S')
    }
}

fn alias_suffix(id: &str) -> String {
    let tokens: Vec<&str> = id
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() {
        return String::new();
    }
    let family_like = ["fsm", "ca", "tm", "auto", "rule"];
    let token_start = if family_like.contains(&tokens[0].to_ascii_lowercase().as_str()) {
        1
    } else {
        0
    };
    let mut out = String::new();
    for token in tokens.iter().skip(token_start) {
        out.push_str(&token_code(token));
        if out.chars().count() >= 6 {
            break;
        }
    }
    out.chars().take(6).collect()
}

fn token_code(token: &str) -> String {
    if token.is_empty() {
        return String::new();
    }
    if token.chars().all(|ch| ch.is_ascii_digit()) {
        return token.to_string();
    }
    if token.chars().all(|ch| ch.is_ascii_alphabetic()) {
        let mut chars = token.chars();
        let first = chars.next().unwrap_or('X').to_ascii_uppercase();
        let last = token.chars().last().unwrap_or(first).to_ascii_uppercase();
        if token.chars().count() <= 2 {
            return token.to_ascii_uppercase();
        }
        return format!("{first}{last}");
    }
    token.to_ascii_uppercase()
}

fn dedupe_alias(base: String, used: &mut HashSet<String>) -> String {
    if used.insert(base.clone()) {
        return base;
    }
    let mut suffix = 2usize;
    loop {
        let candidate = format!("{base}{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

fn history_cell_text(bit: Option<u8>) -> &'static str {
    if bit.is_some() {
        CELL_GLYPH
    } else {
        CELL_EMPTY
    }
}

fn history_cell_style(bit: Option<u8>, zero: Style, one: Style, empty: Style) -> Style {
    match bit {
        Some(0) => zero,
        Some(1) => one,
        _ => empty,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_round_limit_caps_preview_at_500() {
        let entries = vec![nit_games::MatchHistoryPreview {
            match_index: 1,
            total_matches: 1,
            a: "0".into(),
            b: "867".into(),
            rounds_total: 700,
            outcomes: "2".repeat(700),
        }];

        assert_eq!(
            max_round_limit(&entries),
            nit_games::MatchHistoryPreview::DISPLAY_ROUND_CAP
        );
        assert_eq!(
            entries[0].preview_outcomes().len(),
            nit_games::MatchHistoryPreview::DISPLAY_ROUND_CAP
        );
    }
}
