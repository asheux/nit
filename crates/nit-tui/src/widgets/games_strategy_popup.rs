use nit_core::{AppState, UiSelectionPane};
use nit_games::config::StrategySpecKind;
use nit_games::strategy::InputMode;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::theme::Theme;
use crate::widgets::games_visualizer_view::strategy_display_name_from_def;
use crate::widgets::text_selection::apply_ui_selection;

const MIN_WIDTH: u16 = 70;
const MIN_HEIGHT: u16 = 20;

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = screen.width.min(120).max(MIN_WIDTH);
    let height = screen.height.min(45).max(MIN_HEIGHT);
    (width, height)
}

fn trim_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut count = 0usize;
    for ch in text.chars() {
        if count >= max_width {
            break;
        }
        out.push(ch);
        count += 1;
    }
    out
}

fn input_mode_label(mode: InputMode) -> &'static str {
    match mode {
        InputMode::OpponentLastAction => "opponent_last_action",
        InputMode::SelfLastAction => "self_last_action",
        InputMode::JointLastAction => "joint_last_action",
    }
}

pub fn build_definition_lines(def: &nit_games::output::StrategyDefinition) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("id: {}", def.id));
    if let Some(name) = def.name.as_ref() {
        lines.push(format!("name: {}", name));
    }
    match &def.kind {
        StrategySpecKind::Builtin { builtin } => {
            lines.push(format!("kind: builtin ({builtin:?})"));
        }
        StrategySpecKind::Random { p_cooperate } => {
            lines.push(format!("kind: random"));
            lines.push(format!("p_cooperate: {:.3}", p_cooperate));
        }
        StrategySpecKind::Memory { n, initial, table } => {
            lines.push("kind: memory".to_string());
            lines.push(format!("n: {}", n));
            lines.push(format!("initial: {}", initial.as_char()));
            let table_str: String = table.iter().map(|a| a.as_char()).collect();
            lines.push(format!("table: {table_str}"));
        }
        StrategySpecKind::Fsm {
            num_states,
            start_state,
            outputs,
            input_mode,
            transitions,
        } => {
            let inferred = transitions
                .first()
                .map(|row| if row.len() == 4 { InputMode::JointLastAction } else { InputMode::OpponentLastAction })
                .unwrap_or(InputMode::OpponentLastAction);
            let mode = input_mode.unwrap_or(inferred);
            let state_count = if *num_states > 0 {
                *num_states
            } else {
                outputs.len()
            };
            lines.push("kind: fsm".to_string());
            lines.push(format!("states: {}", state_count));
            lines.push(format!("start_state: {}", start_state));
            lines.push(format!("input_mode: {}", input_mode_label(mode)));
            let outputs_str: String = outputs.iter().map(|a| a.as_char()).collect();
            lines.push(format!("outputs: {outputs_str}"));
            let headers: Vec<&'static str> = match mode {
                InputMode::OpponentLastAction => vec!["C", "D"],
                InputMode::SelfLastAction => vec!["C", "D"],
                InputMode::JointLastAction => vec!["CC", "CD", "DC", "DD"],
            };
            lines.push("transitions:".to_string());
            lines.push(format!("state | {}", headers.join(" ")));
            for (state_idx, row) in transitions.iter().enumerate() {
                let output = outputs
                    .get(state_idx)
                    .map(|a| a.as_char())
                    .unwrap_or('?');
                let row_str = row
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                lines.push(format!("{state_idx}({output}) | {row_str}"));
            }
        }
        StrategySpecKind::OneSidedTm {
            states,
            symbols,
            start_state,
            blank,
            fallback_symbol,
            max_steps_per_round,
            input_mode,
            output_map,
            transitions,
            rule_code,
        } => {
            lines.push("kind: one_sided_tm".to_string());
            lines.push(format!("states: {}", states));
            lines.push(format!("symbols: {}", symbols));
            lines.push(format!("start_state: {}", start_state));
            lines.push(format!("blank: {}", blank));
            let fallback = fallback_symbol.unwrap_or(*blank);
            lines.push(format!("fallback_symbol: {}", fallback));
            lines.push(format!("max_steps_per_round: {}", max_steps_per_round));
            lines.push(format!("input_mode: {}", input_mode_label(*input_mode)));
            if let Some(code) = rule_code {
                lines.push(format!("rule_code: {}", code));
            }
            let output_str: String = output_map.iter().map(|a| a.as_char()).collect();
            lines.push(format!("output_map: {output_str}"));
            lines.push("transitions:".to_string());
            for state in 1..=*states as usize {
                for read in 0..*symbols as usize {
                    let idx = (state - 1) * (*symbols as usize) + read;
                    if let Some(rule) = transitions.get(idx) {
                        let move_label = match rule.move_dir {
                            nit_games::strategy::TmMove::Left => "-1",
                            nit_games::strategy::TmMove::Right => "1",
                            nit_games::strategy::TmMove::Stay => "0",
                        };
                        lines.push(format!(
                            "(s{}, r{}) -> (next={}, write={}, move={})",
                            state, read, rule.next, rule.write, move_label
                        ));
                    }
                }
            }
        }
    }
    lines
}

fn strategy_list(state: &AppState) -> &[nit_games::output::StrategyDefinition] {
    &state.games.strategy_inspect.definitions
}

fn line_count(state: &AppState) -> usize {
    let mut count = 1; // status line
    if state.games.strategy_inspect.last_error.is_some() {
        count += 1;
    }
    if !state.games.strategy_inspect.lines.is_empty() {
        if state.games.strategy_inspect.title.is_some() {
            count += 2;
        }
        count += 1;
        count += state.games.strategy_inspect.lines.len();
        count += 2;
        return count;
    }
    count += 1;
    let list = strategy_list(state);
    if list.is_empty() {
        count += 1;
    } else {
        count += 1;
        count += list.len();
    }
    count += 2;
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

    let status = if state.games.strategy_inspect.last_error.is_some() {
        "ERROR"
    } else if !state.games.strategy_inspect.lines.is_empty() {
        "READY"
    } else {
        "SELECT"
    };
    push(
        Line::from(vec![
            Span::styled("status: ", label_style),
            Span::styled(
                status,
                if state.games.strategy_inspect.last_error.is_some() {
                    warn_style
                } else {
                    value_style
                },
            ),
        ]),
        &mut idx,
        &mut lines,
    );

    if let Some(err) = state.games.strategy_inspect.last_error.as_ref() {
        push(
            Line::from(vec![
                Span::styled("error: ", warn_style),
                Span::styled(trim_to_width(err, max_width), value_style),
            ]),
            &mut idx,
            &mut lines,
        );
    }

    if !state.games.strategy_inspect.lines.is_empty() {
        if let Some(title) = state.games.strategy_inspect.title.as_ref() {
            push(Line::from(""), &mut idx, &mut lines);
            push(
                Line::from(Span::styled(
                    trim_to_width(title, max_width),
                    value_style,
                )),
                &mut idx,
                &mut lines,
            );
        }
        push(Line::from(""), &mut idx, &mut lines);

        let lines_start = idx;
        let total_lines = state.games.strategy_inspect.lines.len();
        let lines_end = lines_start.saturating_add(total_lines);
        if end > lines_start && start < lines_end {
            let slice_start = start.saturating_sub(lines_start).min(total_lines);
            let slice_end = end.saturating_sub(lines_start).min(total_lines);
            for line in &state.games.strategy_inspect.lines[slice_start..slice_end] {
                push(
                    Line::from(Span::styled(
                        trim_to_width(line, max_width),
                        value_style,
                    )),
                    &mut idx,
                    &mut lines,
                );
            }
        }
        idx = lines_end;

        push(Line::from(""), &mut idx, &mut lines);
        push(
            Line::from(Span::styled("Esc close · ↑/↓ scroll · R reset", dim_style)),
            &mut idx,
            &mut lines,
        );
        return lines;
    }

    push(Line::from(""), &mut idx, &mut lines);
    let list = strategy_list(state);
    if list.is_empty() {
        push(
            Line::from(Span::styled(
                "No strategies available.",
                dim_style,
            )),
            &mut idx,
            &mut lines,
        );
    } else {
        let label = if let Some(source) = state.games.strategy_inspect.source_label.as_deref() {
            format!("Select a strategy ({source}):")
        } else {
            "Select a strategy:".to_string()
        };
        push(Line::from(Span::styled(label, label_style)), &mut idx, &mut lines);
        let list_start = idx;
        let list_end = list_start.saturating_add(list.len());
        if end > list_start && start < list_end {
            let slice_start = start.saturating_sub(list_start).min(list.len());
            let slice_end = end.saturating_sub(list_start).min(list.len());
            for (offset, def) in list[slice_start..slice_end].iter().enumerate() {
                let item_idx = slice_start + offset;
                let style = if item_idx == state.games.strategy_inspect.selected_index {
                    selected_style
                } else {
                    value_style
                };
                let prefix = if item_idx == state.games.strategy_inspect.selected_index {
                    "›"
                } else {
                    " "
                };
                let label = format!(
                    "{prefix} {} — {}",
                    def.id,
                    strategy_display_name_from_def(def)
                );
                push(
                    Line::from(Span::styled(trim_to_width(&label, max_width), style)),
                    &mut idx,
                    &mut lines,
                );
            }
        }
        idx = list_end;
    }

    push(Line::from(""), &mut idx, &mut lines);
    push(
        Line::from(Span::styled(
            "Enter inspect · Esc close · R reset",
            dim_style,
        )),
        &mut idx,
        &mut lines,
    );
    lines
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
            " STRATEGY INSPECT ",
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
    let scroll = state.games.strategy_inspect.scroll_offset.min(max_scroll);
    let mut lines = build_lines_window(
        state,
        theme,
        inner.width,
        scroll,
        inner.height as usize,
    );
    lines = apply_ui_selection(
        lines,
        state.ui_selection.as_ref(),
        UiSelectionPane::GamesStrategyPopup,
        theme.selection_bg,
        scroll,
    );
    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));
    frame.render_widget(paragraph, inner);
}
