use nit_core::{AppState, UiSelectionPane};
use nit_games::config::StrategySpecKind;
use nit_games::game::Action;
use nit_games::output::StrategyDefinition;
use nit_games::strategy::InputMode;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::theme::Theme;
use crate::widgets::games_visualizer_view::strategy_display_name_from_def;
use crate::widgets::graph_render::{self, GraphEdge, GraphNode, GraphSpec};
use crate::widgets::text_selection::apply_ui_selection;

const MIN_WIDTH: u16 = 70;
const MIN_HEIGHT: u16 = 20;

const GRAPH_NODE_LIMIT: usize = 12;

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

fn color_for_symbol(ch: char, theme: &Theme) -> Option<Color> {
    match ch {
        '0' => Some(Color::Red),
        '1' => Some(Color::Green),
        '2' => Some(Color::Blue),
        '3' => Some(Color::Magenta),
        'C' => Some(Color::Green),
        'D' => Some(Color::Red),
        'H' => Some(theme.warning),
        _ => None,
    }
}

fn stylize_tokenized(content: &str, theme: &Theme, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buffer = String::new();
    for ch in content.chars() {
        if let Some(color) = color_for_symbol(ch, theme) {
            if !buffer.is_empty() {
                spans.push(Span::styled(buffer.clone(), base));
                buffer.clear();
            }
            spans.push(Span::styled(
                ch.to_string(),
                base.fg(color).add_modifier(Modifier::BOLD),
            ));
        } else {
            buffer.push(ch);
        }
    }
    if !buffer.is_empty() {
        spans.push(Span::styled(buffer, base));
    }
    spans
}

fn style_table_row(
    line: &str,
    theme: &Theme,
    value_style: Style,
    dim_style: Style,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let parts: Vec<&str> = line.split('|').collect();
    if parts.len() <= 1 {
        return Line::from(Span::styled(line.to_string(), value_style));
    }
    for (idx, part) in parts.iter().enumerate() {
        if idx == 0 {
            if !part.is_empty() {
                spans.push(Span::styled((*part).to_string(), dim_style));
            }
            continue;
        }
        spans.push(Span::styled("|".to_string(), dim_style));
        let cell = *part;
        let lead_len = cell.len() - cell.trim_start().len();
        let trail_len = cell.len() - cell.trim_end().len();
        if lead_len > 0 {
            spans.push(Span::styled(" ".repeat(lead_len), value_style));
        }
        let trimmed = cell.trim();
        if !trimmed.is_empty() {
            spans.extend(stylize_tokenized(trimmed, theme, value_style));
        }
        if trail_len > 0 {
            spans.push(Span::styled(" ".repeat(trail_len), value_style));
        }
    }
    Line::from(spans)
}

fn style_definition_line(
    line: &str,
    max_width: usize,
    theme: &Theme,
    label_style: Style,
    value_style: Style,
    dim_style: Style,
) -> Line<'static> {
    let trimmed = trim_to_width(line, max_width);
    if trimmed.starts_with('+') {
        return Line::from(Span::styled(trimmed, dim_style));
    }
    if trimmed.starts_with('|') {
        return style_table_row(&trimmed, theme, value_style, dim_style);
    }
    if let Some((label, rest)) = trimmed.split_once(':') {
        let mut spans = Vec::new();
        spans.push(Span::styled(format!("{label}:"), label_style));
        if !rest.is_empty() {
            spans.push(Span::styled(" ".to_string(), value_style));
            spans.extend(stylize_tokenized(rest.trim_start(), theme, value_style));
        }
        return Line::from(spans);
    }
    Line::from(Span::styled(trimmed, value_style))
}

fn input_mode_label(mode: InputMode) -> &'static str {
    match mode {
        InputMode::OpponentLastAction => "opponent_last_action",
        InputMode::SelfLastAction => "self_last_action",
        InputMode::JointLastAction => "joint_last_action",
    }
}

fn input_symbol_legend(mode: InputMode) -> &'static str {
    match mode {
        InputMode::OpponentLastAction => "0=C 1=D (opponent_last_action)",
        InputMode::SelfLastAction => "0=C 1=D (self_last_action)",
        InputMode::JointLastAction => "0=CC 1=CD 2=DC 3=DD",
    }
}

fn table_border(widths: &[usize]) -> String {
    let mut line = String::from("+");
    for width in widths {
        line.push_str(&"-".repeat(width.saturating_add(2)));
        line.push('+');
    }
    line
}

fn table_row(cells: &[String], widths: &[usize]) -> String {
    let mut line = String::from("|");
    for (idx, width) in widths.iter().enumerate() {
        let cell = cells.get(idx).cloned().unwrap_or_default();
        line.push(' ');
        line.push_str(&format!("{cell:<width$}", width = *width));
        line.push(' ');
        line.push('|');
    }
    line
}

fn build_table(headers: &[String], rows: &[Vec<String>]) -> Vec<String> {
    if headers.is_empty() {
        return Vec::new();
    }
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (idx, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(idx) {
                *width = (*width).max(cell.chars().count());
            }
        }
    }
    let border = table_border(&widths);
    let mut lines = Vec::new();
    lines.push(border.clone());
    lines.push(table_row(headers, &widths));
    lines.push(border.clone());
    for row in rows {
        lines.push(table_row(row, &widths));
    }
    lines.push(border);
    lines
}

fn split_graph_sections(lines: &[String]) -> (Vec<String>, Vec<String>) {
    if lines.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let mut text = Vec::new();
    let mut graph = Vec::new();
    let mut in_graph = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("graph:") {
            in_graph = true;
            graph.push(line.clone());
            continue;
        }
        if in_graph {
            if trimmed.is_empty() {
                in_graph = false;
                continue;
            }
            graph.push(line.clone());
            continue;
        }
        text.push(line.clone());
    }

    (text, graph)
}

fn graph_from_definition(def: &StrategyDefinition) -> Option<GraphSpec> {
    match &def.kind {
        StrategySpecKind::Fsm {
            num_states,
            start_state,
            outputs,
            input_mode,
            transitions,
            ..
        } => {
            let inferred = transitions
                .first()
                .map(|row| {
                    if row.len() == 4 {
                        InputMode::JointLastAction
                    } else {
                        InputMode::OpponentLastAction
                    }
                })
                .unwrap_or(InputMode::OpponentLastAction);
            let mode = input_mode.unwrap_or(inferred);
            let state_count = if *num_states > 0 {
                *num_states
            } else {
                outputs.len()
            };
            let spec = fsm_graph_spec(state_count, *start_state, mode, outputs, transitions);
            if spec.nodes.len() > GRAPH_NODE_LIMIT {
                None
            } else {
                Some(spec)
            }
        }
        StrategySpecKind::Ca { .. } => None,
        StrategySpecKind::OneSidedTm {
            states,
            symbols,
            start_state,
            transitions,
            ..
        } => tm_graph_spec(*states, *symbols, *start_state, transitions),
    }
}

fn fsm_graph_spec(
    state_count: usize,
    start_state: usize,
    mode: InputMode,
    outputs: &[Action],
    transitions: &[Vec<usize>],
) -> GraphSpec {
    let alphabet = mode.alphabet_size();
    let mut nodes = Vec::with_capacity(state_count);
    for state in 0..state_count {
        nodes.push(GraphNode {
            id: state,
            label: state.to_string(),
            output: outputs.get(state).map(|a| a.as_char()),
        });
    }
    let mut edges = Vec::new();
    for state in 0..state_count {
        if let Some(row) = transitions.get(state) {
            for input in 0..alphabet {
                if let Some(&next) = row.get(input) {
                    edges.push(GraphEdge {
                        from: state,
                        to: next,
                        label: input.to_string(),
                    });
                }
            }
        }
    }
    GraphSpec {
        nodes,
        edges,
        start: Some(start_state),
    }
}

fn tm_graph_spec(
    states: u16,
    symbols: u8,
    start_state: u16,
    transitions: &[nit_games::strategy::TmTransition],
) -> Option<GraphSpec> {
    let state_count = states as usize;
    let mut nodes = Vec::with_capacity(state_count + 1);
    for state in 1..=state_count {
        nodes.push(GraphNode {
            id: state - 1,
            label: state.to_string(),
            output: None,
        });
    }
    let mut edges = Vec::new();
    let symbols_usize = symbols as usize;
    let mut has_halt = false;
    for state in 1..=state_count {
        for read in 0..symbols_usize {
            let idx = (state - 1) * symbols_usize + read;
            if let Some(rule) = transitions.get(idx) {
                let to = if rule.next == 0 {
                    has_halt = true;
                    state_count
                } else {
                    rule.next.saturating_sub(1) as usize
                };
                edges.push(GraphEdge {
                    from: state - 1,
                    to,
                    // Label edges by READ symbol (input branch) to avoid duplicate WRITE collisions.
                    label: read.to_string(),
                });
            }
        }
    }
    if has_halt {
        nodes.push(GraphNode {
            id: state_count,
            label: "H".to_string(),
            output: None,
        });
    }
    if nodes.len() > GRAPH_NODE_LIMIT {
        return None;
    }
    Some(GraphSpec {
        nodes,
        edges,
        start: Some(start_state.saturating_sub(1) as usize),
    })
}

fn render_graph_canvas(frame: &mut Frame, area: Rect, theme: &Theme, graph: &GraphSpec) {
    graph_render::render(frame, area, theme, graph);
}

fn build_fsm_graph_lines(
    state_count: usize,
    start_state: usize,
    mode: InputMode,
    outputs: &[Action],
    transitions: &[Vec<usize>],
) -> Vec<String> {
    let alphabet = mode.alphabet_size();
    let mut lines = Vec::new();
    lines.push("graph:".to_string());
    lines.push(format!("legend: {}", input_symbol_legend(mode)));
    lines.push(format!("start_state: {start_state}"));

    let mut headers = Vec::with_capacity(alphabet + 1);
    headers.push("state".to_string());
    for idx in 0..alphabet {
        headers.push(idx.to_string());
    }

    let mut rows = Vec::new();
    for state_idx in 0..state_count {
        let output = outputs.get(state_idx).map(|a| a.as_char()).unwrap_or('?');
        let mut row = Vec::with_capacity(alphabet + 1);
        row.push(format!("{state_idx}({output})"));
        let trans_row = transitions.get(state_idx);
        for input_idx in 0..alphabet {
            let next = trans_row
                .and_then(|row| row.get(input_idx))
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".to_string());
            row.push(next);
        }
        rows.push(row);
    }
    lines.extend(build_table(&headers, &rows));
    lines
}

fn build_tm_graph_lines(
    states: u16,
    symbols: u8,
    transitions: &[nit_games::strategy::TmTransition],
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push("graph:".to_string());
    lines.push("legend: edge label = read symbol (input branch)".to_string());
    lines.push("note: cell = write+move+next (e.g. 1R2); H=HALT".to_string());

    let mut headers = Vec::with_capacity(symbols as usize + 1);
    headers.push("state".to_string());
    for sym in 0..symbols {
        headers.push(sym.to_string());
    }

    let mut rows = Vec::new();
    let symbols_usize = symbols as usize;
    for state in 1..=states as usize {
        let mut row = Vec::with_capacity(symbols_usize + 1);
        row.push(state.to_string());
        for read in 0..symbols_usize {
            let idx = (state - 1) * symbols_usize + read;
            let cell = if let Some(rule) = transitions.get(idx) {
                let mv = match rule.move_dir {
                    nit_games::strategy::TmMove::Left => "L",
                    nit_games::strategy::TmMove::Right => "R",
                    nit_games::strategy::TmMove::Stay => "S",
                };
                let next = if rule.next == 0 {
                    "H".to_string()
                } else {
                    rule.next.to_string()
                };
                format!("{}{}{}", rule.write, mv, next)
            } else {
                "-".to_string()
            };
            row.push(cell);
        }
        rows.push(row);
    }
    lines.extend(build_table(&headers, &rows));
    lines
}

pub fn build_definition_lines(def: &nit_games::output::StrategyDefinition) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("id: {}", def.id));
    if let Some(name) = def.name.as_ref() {
        lines.push(format!("name: {}", name));
    }
    match &def.kind {
        StrategySpecKind::Fsm {
            num_states,
            start_state,
            outputs,
            input_mode,
            transitions,
            ..
        } => {
            let inferred = transitions
                .first()
                .map(|row| {
                    if row.len() == 4 {
                        InputMode::JointLastAction
                    } else {
                        InputMode::OpponentLastAction
                    }
                })
                .unwrap_or(InputMode::OpponentLastAction);
            let mode = input_mode.unwrap_or(inferred);
            let state_count = if *num_states > 0 {
                *num_states
            } else {
                outputs.len()
            };
            lines.push("kind: fsm".to_string());
            let outputs_str: String = outputs.iter().map(|a| a.as_char()).collect();
            lines.push(format!(
                "params: states={} start={} input={} outputs={}",
                state_count,
                start_state,
                input_mode_label(mode),
                outputs_str
            ));
            lines.push(String::new());
            lines.extend(build_fsm_graph_lines(
                state_count,
                *start_state,
                mode,
                outputs,
                transitions,
            ));
        }
        StrategySpecKind::Ca { n, k, r, t } => {
            lines.push("kind: ca".to_string());
            lines.push(format!("params: n={} k={} r={} t={}", n, k, r, t));
            lines.push("input: Flatten[history] (global A,B order)".to_string());
            lines.push("output: last cell of ShrinkingCA final row".to_string());
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
            lines.push("kind: tm".to_string());
            let fallback = fallback_symbol.unwrap_or(*blank);
            if let Some(code) = rule_code {
                lines.push(format!("rule_code: {}", code));
            }
            let output_str: String = output_map.iter().map(|a| a.as_char()).collect();
            lines.push(format!(
                "params: states={} symbols={} start={} blank={} fallback={} max_steps={}",
                states, symbols, start_state, blank, fallback, max_steps_per_round
            ));
            lines.push(format!(
                "io: input={} output_map={output_str}",
                input_mode_label(*input_mode)
            ));
            lines.push(String::new());
            lines.extend(build_tm_graph_lines(*states, *symbols, transitions));
        }
    }
    lines
}

fn strategy_list(state: &AppState) -> &[nit_games::output::StrategyDefinition] {
    &state.games.strategy_inspect.definitions
}

fn line_count(state: &AppState) -> usize {
    let (display_lines, _) = split_graph_sections(&state.games.strategy_inspect.lines);
    let mut count = 1; // status line
    if state.games.strategy_inspect.last_error.is_some() {
        count += 1;
    }
    if !state.games.strategy_inspect.lines.is_empty() {
        if state.games.strategy_inspect.title.is_some() {
            count += 2;
        }
        count += 1;
        count += display_lines.len();
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
    display_lines: &[String],
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
                Line::from(Span::styled(trim_to_width(title, max_width), value_style)),
                &mut idx,
                &mut lines,
            );
        }
        push(Line::from(""), &mut idx, &mut lines);

        let lines_start = idx;
        let total_lines = display_lines.len();
        let lines_end = lines_start.saturating_add(total_lines);
        if end > lines_start && start < lines_end {
            let slice_start = start.saturating_sub(lines_start).min(total_lines);
            let slice_end = end.saturating_sub(lines_start).min(total_lines);
            for line in &display_lines[slice_start..slice_end] {
                let styled = style_definition_line(
                    line,
                    max_width,
                    theme,
                    label_style,
                    value_style,
                    dim_style,
                );
                push(styled, &mut idx, &mut lines);
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
            Line::from(Span::styled("No strategies available.", dim_style)),
            &mut idx,
            &mut lines,
        );
    } else {
        let label = if let Some(source) = state.games.strategy_inspect.source_label.as_deref() {
            format!("Select a strategy ({source}):")
        } else {
            "Select a strategy:".to_string()
        };
        push(
            Line::from(Span::styled(label, label_style)),
            &mut idx,
            &mut lines,
        );
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
    let (display_lines, _) = split_graph_sections(&state.games.strategy_inspect.lines);
    let total = line_count(state);
    build_lines_window(state, theme, inner_width, 0, total, &display_lines)
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

    let (display_lines, graph_lines) = split_graph_sections(&state.games.strategy_inspect.lines);
    let graph_spec = state
        .games
        .strategy_inspect
        .definition
        .as_ref()
        .and_then(graph_from_definition);
    let show_graph = !state.games.strategy_inspect.lines.is_empty()
        && (graph_spec.is_some() || !graph_lines.is_empty());
    if show_graph {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(50),
                Constraint::Length(1),
                Constraint::Percentage(50),
            ])
            .split(inner);

        let left_block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
                " DETAILS ",
                Style::default()
                    .fg(theme.title)
                    .add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(theme.border))
            .style(Style::default().bg(theme.background));
        let right_block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
                " GRAPH ",
                Style::default()
                    .fg(theme.title)
                    .add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(theme.border))
            .style(Style::default().bg(theme.background));

        let left_inner = left_block.inner(chunks[0]);
        let right_inner = right_block.inner(chunks[2]);

        frame.render_widget(left_block, chunks[0]);
        frame.render_widget(right_block, chunks[2]);

        let total_lines = line_count(state);
        let max_scroll = total_lines.saturating_sub(left_inner.height as usize);
        let scroll = state.games.strategy_inspect.scroll_offset.min(max_scroll);
        let mut left_lines = build_lines_window(
            state,
            theme,
            left_inner.width,
            scroll,
            left_inner.height as usize,
            &display_lines,
        );
        left_lines = apply_ui_selection(
            left_lines,
            state.ui_selection.as_ref(),
            UiSelectionPane::GamesStrategyPopup,
            theme.selection_bg,
            scroll,
        );
        let left_paragraph = Paragraph::new(left_lines)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0));
        frame.render_widget(left_paragraph, left_inner);

        if let Some(graph) = graph_spec {
            frame.render_widget(
                Block::default().style(Style::default().bg(theme.background)),
                right_inner,
            );
            render_graph_canvas(frame, right_inner, theme, &graph);
        } else {
            let label_style = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
            let value_style = Style::default().fg(theme.foreground);
            let dim_style = Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM);
            let max_width = right_inner.width.max(1) as usize;
            let mut graph_styled = Vec::new();
            let mut idx = 0usize;
            for line in &graph_lines {
                if idx >= right_inner.height as usize {
                    break;
                }
                let styled = style_definition_line(
                    line,
                    max_width,
                    theme,
                    label_style,
                    value_style,
                    dim_style,
                );
                graph_styled.push(styled);
                idx = idx.saturating_add(1);
            }
            let right_paragraph = Paragraph::new(graph_styled)
                .style(Style::default().fg(theme.foreground).bg(theme.background))
                .wrap(Wrap { trim: false })
                .scroll((0, 0));
            frame.render_widget(right_paragraph, right_inner);
        }
        return;
    }

    let total_lines = line_count(state);
    let max_scroll = total_lines.saturating_sub(inner.height as usize);
    let scroll = state.games.strategy_inspect.scroll_offset.min(max_scroll);
    let mut lines = build_lines_window(
        state,
        theme,
        inner.width,
        scroll,
        inner.height as usize,
        &display_lines,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fsm_graph_spec_builds_edges() {
        let def = StrategyDefinition {
            id: "t".to_string(),
            name: None,
            kind: StrategySpecKind::Fsm {
                num_states: 2,
                start_state: 0,
                outputs: vec![Action::Cooperate, Action::Defect],
                input_mode: Some(InputMode::OpponentLastAction),
                transitions: vec![vec![0, 1], vec![1, 0]],
                index: None,
            },
            rng_seed_a: None,
            rng_seed_b: None,
        };

        let graph = graph_from_definition(&def).expect("graph");
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 4);
        assert_eq!(graph.start, Some(0));
    }
}
