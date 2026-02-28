use nit_core::{AppState, UiSelectionPane};
use nit_games::config::StrategySpecKind;
use nit_games::game::Action;
use nit_games::strategy::{run_one_sided_tm_from_integer, TmMove, TmStopReason, TmTransition};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use crate::theme::Theme;
use crate::widgets::text_selection::apply_ui_selection;

const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 20;
const HEAD_DOT: char = '●';
static TM_SIM_CACHE: OnceLock<Mutex<Option<SimCache>>> = OnceLock::new();
static TM_SIM_PENDING: OnceLock<Arc<SimResult>> = OnceLock::new();

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct SimKey {
    def_hash: u64,
    input: u64,
    step_limit: u32,
}

struct SimCache {
    key: SimKey,
    state: SimCacheState,
}

fn tm_sim_cache() -> &'static Mutex<Option<SimCache>> {
    TM_SIM_CACHE.get_or_init(|| Mutex::new(None))
}

fn tm_sim_pending() -> Arc<SimResult> {
    TM_SIM_PENDING
        .get_or_init(|| {
            Arc::new(SimResult {
                log_lines: vec!["computing...".to_string()],
                steps: Vec::new(),
                frames: Vec::new(),
                halted: false,
                output_value: None,
                output_symbol: None,
            })
        })
        .clone()
}

enum SimCacheState {
    Running,
    Ready(Arc<SimResult>),
}

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = screen.width.min(140).max(MIN_WIDTH);
    let height = screen.height.min(45).max(MIN_HEIGHT);
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

    let status = if state.games.tm_sim.last_error.is_some() {
        "ERROR"
    } else if state.games.tm_sim.definition.is_some() && state.games.tm_sim.input.is_some() {
        "READY"
    } else {
        "IDLE"
    };
    left_lines.push(Line::from(vec![
        Span::styled("status: ", label_style),
        Span::styled(
            status,
            if state.games.tm_sim.last_error.is_some() {
                warn_style
            } else {
                number_style
            },
        ),
    ]));

    if let Some(source) = state.games.tm_sim.source_label.as_deref() {
        left_lines.push(Line::from(vec![
            Span::styled("source: ", label_style),
            Span::styled(source.to_string(), value_style),
        ]));
    }

    if let Some(err) = state.games.tm_sim.last_error.as_ref() {
        left_lines.push(Line::from(vec![
            Span::styled("error: ", warn_style),
            Span::styled(trim_to_width(err, left_width), value_style),
        ]));
    }

    let (Some(def), Some(input)) = (
        state.games.tm_sim.definition.as_ref(),
        state.games.tm_sim.input,
    ) else {
        left_lines.push(Line::from(""));
        left_lines.push(Line::from(Span::styled(
            "Use :games tm [run|config] <input> [steps] [strategy_id]",
            dim_style,
        )));
        left_lines.push(Line::from(Span::styled(
            "or :games tm {rule_code, states, symbols} <input> [steps]",
            dim_style,
        )));
        return (left_lines, right_lines);
    };

    left_lines.push(Line::from(""));
    left_lines.push(Line::from(vec![
        Span::styled("strategy: ", label_style),
        Span::styled(def.id.clone(), value_style),
    ]));
    left_lines.push(Line::from(vec![
        Span::styled("input: ", label_style),
        Span::styled(input.to_string(), value_style),
    ]));

    let spec = match &def.kind {
        StrategySpecKind::OneSidedTm { .. } => &def.kind,
        _ => {
            left_lines.push(Line::from(Span::styled(
                "Selected strategy is not a one-sided TM.",
                warn_style,
            )));
            return (left_lines, right_lines);
        }
    };

    let StrategySpecKind::OneSidedTm {
        states,
        symbols,
        start_state,
        blank,
        fallback_symbol,
        max_steps_per_round,
        input_mode: _input_mode,
        output_map,
        transitions,
        rule_code,
    } = spec
    else {
        return (left_lines, right_lines);
    };

    let fallback = fallback_symbol.unwrap_or(*blank);
    let steps_override = state.games.tm_sim.steps_override;
    let step_limit = steps_override
        .unwrap_or(*max_steps_per_round)
        .min(*max_steps_per_round);
    left_lines.push(Line::from(vec![
        Span::styled("states: ", label_style),
        Span::styled(states.to_string(), value_style),
        Span::styled("  symbols: ", label_style),
        Span::styled(symbols.to_string(), value_style),
        Span::styled("  start: ", label_style),
        Span::styled(start_state.to_string(), value_style),
    ]));
    left_lines.push(Line::from(vec![
        Span::styled("rule_space: ", label_style),
        Span::styled(rule_space_label(*states, *symbols), value_style),
    ]));
    left_lines.push(Line::from(vec![
        Span::styled("blank: ", label_style),
        Span::styled(blank.to_string(), value_style),
        Span::styled("  fallback: ", label_style),
        Span::styled(fallback.to_string(), value_style),
        Span::styled("  max_steps: ", label_style),
        Span::styled(max_steps_per_round.to_string(), value_style),
    ]));
    if let Some(override_steps) = steps_override {
        let mut spans = Vec::new();
        spans.push(Span::styled("steps: ", label_style));
        spans.push(Span::styled(step_limit.to_string(), value_style));
        if override_steps > *max_steps_per_round {
            spans.push(Span::styled(" (capped)", dim_style));
        }
        left_lines.push(Line::from(spans));
    }
    left_lines.push(Line::from(vec![
        Span::styled("input: ", label_style),
        Span::styled(
            "integer (gameplay uses FromDigits[Flatten[history], 2])".to_string(),
            value_style,
        ),
    ]));
    left_lines.push(Line::from(vec![
        Span::styled("output: ", label_style),
        Span::styled(
            "halted -> (tm_output mod symbols), timeout -> D".to_string(),
            value_style,
        ),
    ]));
    if let Some(code) = rule_code {
        left_lines.push(Line::from(vec![
            Span::styled("rule_code: ", label_style),
            Span::styled(code.to_string(), value_style),
        ]));
        if rule_code_has_unused_digits(*code, *states, *symbols) {
            left_lines.push(Line::from(Span::styled(
                "warning: rule_code exceeds rule space (unused higher digits)",
                warn_style,
            )));
        }
    }

    let rule_lines = build_rule_table_lines(
        *states,
        *symbols,
        transitions,
        label_style,
        value_style,
        if right_width > 0 {
            right_width.max(1)
        } else {
            left_width
        },
    );
    if !rule_lines.is_empty() {
        if right_width > 0 {
            right_lines.push(Line::from(Span::styled("Rules", header_style)));
            right_lines.extend(rule_lines);
            right_lines.push(Line::from(""));
        } else {
            left_lines.push(Line::from(""));
            left_lines.push(Line::from(Span::styled("Rules", header_style)));
            left_lines.extend(build_rule_table_lines(
                *states,
                *symbols,
                transitions,
                label_style,
                value_style,
                left_width,
            ));
        }
    }

    left_lines.push(Line::from(""));
    left_lines.push(Line::from(Span::styled("Simulation", header_style)));
    let def_hash = tm_spec_hash(
        *symbols,
        *start_state,
        *blank,
        fallback,
        transitions,
        output_map,
    );
    let sim = simulate_tm_cached(
        SimKey {
            def_hash,
            input,
            step_limit,
        },
        input,
        *symbols,
        *start_state,
        *blank,
        fallback,
        step_limit,
        transitions,
        output_map,
    );
    for line in sim.log_lines.iter() {
        left_lines.push(Line::from(Span::styled(
            trim_to_width(line, left_width),
            value_style,
        )));
    }

    if !sim.steps.is_empty() {
        left_lines.push(Line::from(""));
        left_lines.push(Line::from(Span::styled("Steps", header_style)));
        left_lines.extend(build_step_table_lines(
            &sim.steps,
            left_width,
            label_style,
            value_style,
        ));
    }

    if !sim.frames.is_empty() {
        if right_width > 0 {
            right_lines.push(Line::from(Span::styled("Evolution", header_style)));
            right_lines.extend(build_grid_lines(&sim.frames, right_width.max(1), theme));
            right_lines.push(Line::from(""));
            right_lines.extend(build_legend_lines(
                *symbols as usize,
                theme,
                sim.halted,
                sim.output_value,
                sim.output_symbol,
            ));
        } else {
            left_lines.push(Line::from(""));
            left_lines.push(Line::from(Span::styled("Evolution", header_style)));
            left_lines.extend(build_grid_lines(&sim.frames, left_width.max(1), theme));
            left_lines.push(Line::from(""));
            left_lines.extend(build_legend_lines(
                *symbols as usize,
                theme,
                sim.halted,
                sim.output_value,
                sim.output_symbol,
            ));
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
    if !state.games.tm_sim.open {
        return;
    }

    frame.render_widget(Clear, area);

    let border_style = Style::default().fg(theme.border_focused);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(theme.background))
        .title(Span::styled(
            " TM SIMULATOR ",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (left_area, right_area) = layout_for_tm_sim(inner);
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
        let scroll = state.games.tm_sim.scroll_offset.min(max_scroll);

        let left_visible: Vec<Line> = left_lines
            .into_iter()
            .skip(scroll)
            .take(left_area.height as usize)
            .collect();
        let left_visible = apply_ui_selection(
            left_visible,
            state.ui_selection.as_ref(),
            UiSelectionPane::GamesTmSimPopupLeft,
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
                UiSelectionPane::GamesTmSimPopupRight,
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
        let scroll = state.games.tm_sim.scroll_offset.min(max_scroll);
        let visible: Vec<Line> = lines.into_iter().skip(scroll).take(height).collect();
        let visible = apply_ui_selection(
            visible,
            state.ui_selection.as_ref(),
            UiSelectionPane::GamesTmSimPopupLeft,
            theme.selection_bg,
            scroll,
        );
        let paragraph = Paragraph::new(visible)
            .style(Style::default().fg(theme.foreground).bg(theme.background))
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, inner);
    }
}

struct SimFrame {
    tape: Vec<u8>,
    head: usize,
    origin: i32,
}

struct SimResult {
    log_lines: Vec<String>,
    steps: Vec<SimStep>,
    frames: Vec<SimFrame>,
    halted: bool,
    output_value: Option<u64>,
    output_symbol: Option<u8>,
}

struct SimStep {
    step: usize,
    state: u16,
    head_before: usize,
    read: u8,
    next: u16,
    write: u8,
    move_dir: TmMove,
    head_after: usize,
    tape: Vec<u8>,
}

fn tm_spec_hash(
    symbols: u8,
    start_state: u16,
    blank: u8,
    fallback_symbol: u8,
    transitions: &[TmTransition],
    output_map: &[Action],
) -> u64 {
    let mut hasher = DefaultHasher::new();
    symbols.hash(&mut hasher);
    start_state.hash(&mut hasher);
    blank.hash(&mut hasher);
    fallback_symbol.hash(&mut hasher);
    for action in output_map {
        action.as_char().hash(&mut hasher);
    }
    for trans in transitions {
        trans.next.hash(&mut hasher);
        trans.write.hash(&mut hasher);
        let dir = match trans.move_dir {
            TmMove::Left => 0u8,
            TmMove::Stay => 1u8,
            TmMove::Right => 2u8,
        };
        dir.hash(&mut hasher);
    }
    hasher.finish()
}

fn simulate_tm_cached(
    key: SimKey,
    input: u64,
    symbols: u8,
    start_state: u16,
    blank: u8,
    fallback_symbol: u8,
    step_limit: u32,
    transitions: &[TmTransition],
    output_map: &[Action],
) -> Arc<SimResult> {
    if let Ok(guard) = tm_sim_cache().lock() {
        if let Some(cache) = guard.as_ref() {
            if cache.key == key {
                return match &cache.state {
                    SimCacheState::Ready(result) => result.clone(),
                    SimCacheState::Running => tm_sim_pending(),
                };
            }
        }
    }

    let mut guard = tm_sim_cache().lock().unwrap_or_else(|err| err.into_inner());
    *guard = Some(SimCache {
        key,
        state: SimCacheState::Running,
    });
    drop(guard);

    let transitions = transitions.to_vec();
    let output_map = output_map.to_vec();
    let _ = thread::Builder::new()
        .name("nit-tm-sim".into())
        .spawn(move || {
            let result = Arc::new(simulate_tm(
                input,
                symbols,
                start_state,
                blank,
                fallback_symbol,
                step_limit,
                &transitions,
                &output_map,
            ));
            let mut guard = tm_sim_cache().lock().unwrap_or_else(|err| err.into_inner());
            if let Some(cache) = guard.as_mut() {
                if cache.key == key {
                    cache.state = SimCacheState::Ready(result);
                }
            }
        });

    tm_sim_pending()
}

fn simulate_tm(
    input: u64,
    symbols: u8,
    start_state: u16,
    blank: u8,
    _fallback_symbol: u8,
    step_limit: u32,
    transitions: &[TmTransition],
    _output_map: &[Action],
) -> SimResult {
    let mut lines = Vec::new();
    let reserve = step_limit.min(10_000) as usize;
    let mut frames = Vec::with_capacity(reserve.saturating_add(2));
    let mut fixed_point_step: Option<usize> = None;
    if symbols < 2 {
        lines.push("error: symbols must be >= 2 for input decoding".to_string());
        return SimResult {
            log_lines: lines,
            steps: Vec::new(),
            frames,
            halted: false,
            output_value: None,
            output_symbol: None,
        };
    }
    if start_state == 0 {
        lines.push("error: start_state must be >= 1".to_string());
        return SimResult {
            log_lines: lines,
            steps: Vec::new(),
            frames,
            halted: false,
            output_value: None,
            output_symbol: None,
        };
    }
    let run = run_one_sided_tm_from_integer(
        transitions,
        symbols,
        start_state,
        blank,
        input,
        step_limit,
        true,
    );

    let mut steps: Vec<SimStep> = Vec::with_capacity(reserve);
    let origin: i32 = 0;
    if let Some(trace) = run.trace.as_ref() {
        let digits_str: String = trace.input_digits.iter().map(|&d| symbol_char(d)).collect();
        lines.push(format!(
            "input digits (base {}): {}",
            symbols,
            if digits_str.is_empty() {
                "0".into()
            } else {
                digits_str
            }
        ));
        lines.push(format!(
            "initial tape: {}",
            tape_to_string(&trace.initial_tape)
        ));
        lines.push(format!("head at index {}", trace.initial_head));
        frames.push(SimFrame {
            tape: trace.initial_tape.clone(),
            head: trace.initial_head,
            origin,
        });
        let mut prev_after_state: Option<u16> = None;
        let mut prev_head_after: Option<usize> = None;
        let mut prev_tape: Option<Vec<u8>> = None;
        for step in &trace.steps {
            steps.push(SimStep {
                step: step.step,
                state: step.state,
                head_before: step.head_before,
                read: step.read,
                next: step.next,
                write: step.write,
                move_dir: step.move_dir,
                head_after: step.head_after,
                tape: step.tape.clone(),
            });
            frames.push(SimFrame {
                tape: step.tape.clone(),
                head: step.head_after,
                origin,
            });
            if !run.halted {
                let reached_fixed_point = prev_after_state == Some(step.next)
                    && prev_head_after == Some(step.head_after)
                    && prev_tape.as_deref() == Some(step.tape.as_slice());
                if reached_fixed_point {
                    fixed_point_step = Some(step.step);
                    break;
                }
                prev_after_state = Some(step.next);
                prev_head_after = Some(step.head_after);
                prev_tape = Some(step.tape.clone());
            }
        }
    } else {
        let digits = digits_in_base(input, symbols);
        let digits_str: String = digits.iter().map(|&d| symbol_char(d)).collect();
        lines.push(format!(
            "input digits (base {}): {}",
            symbols,
            if digits_str.is_empty() {
                "0".into()
            } else {
                digits_str
            }
        ));
    }

    let action = if let Some(output) = run.output_value {
        let symbol = (output % symbols.max(1) as u64) as u8;
        if symbol == 0 {
            Action::Cooperate
        } else {
            Action::Defect
        }
    } else {
        Action::Defect
    };
    let reason = match run.stop_reason {
        TmStopReason::Output => "halted",
        TmStopReason::MaxSteps => "timeout",
        TmStopReason::MissingTransition => "missing transition",
        TmStopReason::InvalidState => "invalid state",
    };
    if let Some(output) = run.output_value {
        let symbol = output % symbols.max(1) as u64;
        lines.push(format!(
            "result: {} -> out={} mod{}={} -> {}",
            reason,
            output,
            symbols,
            symbol,
            action.as_char()
        ));
    } else {
        lines.push(format!("result: {} -> {}", reason, action.as_char()));
    }
    if let Some(step) = fixed_point_step {
        lines.push(format!(
            "note: fixed point at step {} (evolution truncated)",
            step
        ));
    }
    if !run.halted && matches!(run.stop_reason, TmStopReason::MaxSteps) {
        lines.push(format!("note: max_steps={}", step_limit));
    }
    if let Some(last) = steps.last() {
        let move_label = match last.move_dir {
            TmMove::Left => "-1",
            TmMove::Right => "1",
            TmMove::Stay => "0",
        };
        lines.push(format!(
            "last transition: (state={}, read={}) -> (next={}, write={}, move={})",
            last.state, last.read, last.next, last.write, move_label
        ));
    }

    SimResult {
        log_lines: lines,
        steps,
        frames,
        halted: run.halted,
        output_value: run.output_value,
        output_symbol: run.output_symbol,
    }
}

fn digits_in_base(input: u64, base: u8) -> Vec<u8> {
    if input == 0 {
        return vec![0];
    }
    let base_u64 = base.max(2) as u64;
    let mut digits = Vec::new();
    let mut value = input;
    while value > 0 {
        digits.push((value % base_u64) as u8);
        value /= base_u64;
    }
    digits.reverse();
    digits
}

fn tape_to_string(tape: &[u8]) -> String {
    tape.iter().map(|&s| symbol_char(s)).collect()
}

fn symbol_char(symbol: u8) -> char {
    if symbol < 10 {
        (b'0' + symbol) as char
    } else {
        (b'A' + (symbol - 10)) as char
    }
}

fn rule_space_label(states: u16, symbols: u8) -> String {
    let base = (states as u128) * (symbols as u128) * 2;
    let exp = (states as u32).saturating_mul(symbols as u32);
    if base == 0 || exp == 0 {
        return "n/a".into();
    }
    if let Some(value) = pow_u128_checked(base, exp) {
        format!("{base}^{exp} = {value}")
    } else {
        let approx = (exp as f64) * (base as f64).log10();
        format!("{base}^{exp} ~= 10^{approx:.2}")
    }
}

fn rule_code_has_unused_digits(rule_code: u64, states: u16, symbols: u8) -> bool {
    let base = (states as u128) * (symbols as u128) * 2;
    let exp = (states as u32).saturating_mul(symbols as u32);
    if base == 0 || exp == 0 {
        return false;
    }
    if let Some(space) = pow_u128_checked(base, exp) {
        (rule_code as u128) >= space
    } else {
        false
    }
}

fn pow_u128_checked(base: u128, exp: u32) -> Option<u128> {
    let mut value: u128 = 1;
    for _ in 0..exp {
        value = value.checked_mul(base)?;
    }
    Some(value)
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

pub fn layout_for_tm_sim(inner: Rect) -> (Rect, Option<Rect>) {
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
        let left_len = line_width(&left_line);
        spans.extend(left_line.spans);
        let pad = left_width.saturating_sub(left_len);
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

fn centered_cell(ch: char, width: usize) -> String {
    if width <= 1 {
        return ch.to_string();
    }
    let left = width.saturating_sub(1) / 2;
    let right = width.saturating_sub(left + 1);
    format!("{}{}{}", " ".repeat(left), ch, " ".repeat(right))
}

fn table_total_width(widths: &[usize]) -> usize {
    if widths.is_empty() {
        return 0;
    }
    widths.iter().sum::<usize>() + widths.len() * 3 + 1
}

fn shrink_widths_to_fit(widths: &mut [usize], max_width: usize) {
    if widths.is_empty() {
        return;
    }
    let mut total = table_total_width(widths);
    if total <= max_width {
        return;
    }
    while total > max_width {
        let mut max_idx = None;
        let mut max_width_value = 0usize;
        for (idx, width) in widths.iter().enumerate() {
            if *width > max_width_value {
                max_width_value = *width;
                max_idx = Some(idx);
            }
        }
        let Some(idx) = max_idx else {
            break;
        };
        if widths[idx] <= 1 {
            break;
        }
        widths[idx] = widths[idx].saturating_sub(1);
        total = total.saturating_sub(1);
    }
}

fn shrink_widths_to_fit_with_min(widths: &mut [usize], min_widths: &[usize], max_width: usize) {
    if widths.is_empty() {
        return;
    }
    let mut total = table_total_width(widths);
    if total <= max_width {
        return;
    }
    loop {
        if total <= max_width {
            break;
        }
        let mut max_idx = None;
        let mut max_width_value = 0usize;
        for (idx, width) in widths.iter().enumerate() {
            let min_width = min_widths.get(idx).copied().unwrap_or(1);
            if *width > min_width && *width >= max_width_value {
                max_width_value = *width;
                max_idx = Some(idx);
            }
        }
        let Some(idx) = max_idx else {
            break;
        };
        widths[idx] = widths[idx].saturating_sub(1);
        total = total.saturating_sub(1);
    }
}

fn build_table_border_line(widths: &[usize]) -> String {
    let mut line = String::from("+");
    for width in widths {
        line.push_str(&"-".repeat(width.saturating_add(2)));
        line.push('+');
    }
    line
}

fn build_table_row_line(cells: &[String], widths: &[usize], align_right: &[bool]) -> String {
    let mut line = String::from("|");
    for (idx, (cell, width)) in cells.iter().zip(widths.iter()).enumerate() {
        let trimmed = trim_to_width(cell, *width);
        let formatted = if *align_right.get(idx).unwrap_or(&false) {
            format!("{trimmed:>width$}", width = *width)
        } else {
            format!("{trimmed:<width$}", width = *width)
        };
        line.push(' ');
        line.push_str(&formatted);
        line.push(' ');
        line.push('|');
    }
    line
}

fn build_rule_table_lines(
    states: u16,
    symbols: u8,
    transitions: &[TmTransition],
    label_style: Style,
    value_style: Style,
    max_width: usize,
) -> Vec<Line<'static>> {
    if states == 0 || symbols == 0 || transitions.is_empty() || max_width == 0 {
        return Vec::new();
    }
    let total = transitions.len();
    let max_entries = 32usize;
    if total > max_entries {
        let message = format!("table omitted ({total} entries)");
        let mut widths = vec![message.len().max(8)];
        shrink_widths_to_fit(&mut widths, max_width);
        let border = build_table_border_line(&widths);
        let row = build_table_row_line(&[message], &widths, &[false]);
        return vec![
            Line::from(Span::styled(trim_to_width(&border, max_width), label_style)),
            Line::from(Span::styled(trim_to_width(&row, max_width), label_style)),
            Line::from(Span::styled(trim_to_width(&border, max_width), label_style)),
        ];
    }
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut max_left = "{s, r}".len();
    let mut max_right = "{n, w, m}".len();
    for state in 1..=states {
        for read in 0..symbols {
            let idx = (state as usize - 1) * symbols as usize + read as usize;
            if let Some(trans) = transitions.get(idx) {
                let move_label = match trans.move_dir {
                    TmMove::Left => "-1",
                    TmMove::Right => "1",
                    TmMove::Stay => "0",
                };
                let left = format!("{{{}, {}}}", state, read);
                let right = format!("{{{}, {}, {}}}", trans.next, trans.write, move_label);
                max_left = max_left.max(left.len());
                max_right = max_right.max(right.len());
                rows.push((left, right));
            }
        }
    }

    let mut widths = vec![max_left, max_right];
    let min_widths = vec!["{s, r}".len(), "{n, w, m}".len()];
    shrink_widths_to_fit_with_min(&mut widths, &min_widths, max_width);
    let mut lines = Vec::new();
    let border = build_table_border_line(&widths);
    let header = build_table_row_line(
        &["{s, r}".to_string(), "{n, w, m}".to_string()],
        &widths,
        &[false, false],
    );
    lines.push(Line::from(Span::styled(
        trim_to_width(&border, max_width),
        label_style,
    )));
    lines.push(Line::from(Span::styled(
        trim_to_width(&header, max_width),
        label_style,
    )));
    lines.push(Line::from(Span::styled(
        trim_to_width(&border, max_width),
        label_style,
    )));
    for (left, right) in rows {
        let row = build_table_row_line(&[left, right], &widths, &[false, false]);
        lines.push(Line::from(Span::styled(
            trim_to_width(&row, max_width),
            value_style,
        )));
    }
    lines.push(Line::from(Span::styled(
        trim_to_width(&border, max_width),
        label_style,
    )));
    lines
}

fn build_step_table_lines(
    steps: &[SimStep],
    max_width: usize,
    label_style: Style,
    value_style: Style,
) -> Vec<Line<'static>> {
    if steps.is_empty() || max_width == 0 {
        return Vec::new();
    }
    let mut max_step = 0usize;
    let mut max_state = 0u16;
    let mut max_head = 0usize;
    let mut max_read = 0u8;
    let mut max_next = 0u16;
    let mut max_write = 0u8;
    for step in steps {
        max_step = max_step.max(step.step);
        max_state = max_state.max(step.state);
        max_head = max_head.max(step.head_before);
        max_read = max_read.max(step.read);
        max_next = max_next.max(step.next);
        max_write = max_write.max(step.write);
    }

    let mut widths = vec![
        "step".len().max(max_step.to_string().len()),
        "s(state)".len().max(max_state.to_string().len()),
        "h(head)".len().max(max_head.to_string().len()),
        "r(read)".len().max(max_read.to_string().len()),
        "next".len().max(max_next.to_string().len()),
        "write".len().max(max_write.to_string().len()),
        "move".len().max(2),
    ];

    let base_table_width = table_total_width(&widths);
    let available_for_tape = max_width.saturating_sub(base_table_width.saturating_add(3));
    let mut show_tape = available_for_tape >= 6;
    let mut tape_width = 0usize;
    if show_tape {
        tape_width = available_for_tape.max(6);
        widths.push(tape_width);
    }
    let mut min_widths = vec![
        "step".len(),
        "s(state)".len(),
        "h(head)".len(),
        "r(read)".len(),
        "next".len(),
        "write".len(),
        "move".len(),
    ];
    if show_tape {
        min_widths.push("tape".len().max(6));
    }
    if show_tape && table_total_width(&min_widths) > max_width {
        show_tape = false;
        widths.pop();
        min_widths.pop();
    }
    shrink_widths_to_fit_with_min(&mut widths, &min_widths, max_width);
    if show_tape {
        tape_width = *widths.last().unwrap_or(&tape_width);
    }

    let mut lines = Vec::new();
    let border = build_table_border_line(&widths);
    let mut header_cells = vec![
        "step".to_string(),
        "s(state)".to_string(),
        "h(head)".to_string(),
        "r(read)".to_string(),
        "next".to_string(),
        "write".to_string(),
        "move".to_string(),
    ];
    let mut header_align = vec![false; header_cells.len()];
    if show_tape {
        header_cells.push("tape".to_string());
        header_align.push(false);
    }
    let header = build_table_row_line(&header_cells, &widths, &header_align);
    lines.push(Line::from(Span::styled(
        trim_to_width(&border, max_width),
        label_style,
    )));
    lines.push(Line::from(Span::styled(
        trim_to_width(&header, max_width),
        label_style,
    )));
    lines.push(Line::from(Span::styled(
        trim_to_width(&border, max_width),
        label_style,
    )));
    for step in steps {
        let move_label = match step.move_dir {
            TmMove::Left => "-1",
            TmMove::Right => "1",
            TmMove::Stay => "0",
        };
        let mut cells = vec![
            step.step.to_string(),
            step.state.to_string(),
            step.head_before.to_string(),
            step.read.to_string(),
            step.next.to_string(),
            step.write.to_string(),
            move_label.to_string(),
        ];
        let mut aligns = vec![true; cells.len()];
        if show_tape {
            let tape_str = tape_with_head_snippet(&step.tape, step.head_after, tape_width);
            cells.push(tape_str);
            aligns.push(false);
        }
        let row = build_table_row_line(&cells, &widths, &aligns);
        lines.push(Line::from(Span::styled(
            trim_to_width(&row, max_width),
            value_style,
        )));
    }
    lines.push(Line::from(Span::styled(
        trim_to_width(&border, max_width),
        label_style,
    )));
    lines
}

fn tape_with_head_snippet(tape: &[u8], head: usize, max_len: usize) -> String {
    if tape.is_empty() || max_len == 0 {
        return String::new();
    }
    let mut full = String::new();
    let mut head_char_idx = 0usize;
    for (idx, &cell) in tape.iter().enumerate() {
        if idx == head {
            head_char_idx = full.len();
            full.push(HEAD_DOT);
        } else {
            full.push(symbol_char(cell));
        }
    }
    if head >= tape.len() {
        head_char_idx = full.len();
        full.push(HEAD_DOT);
    }
    if full.len() <= max_len {
        return full;
    }
    let mut start = head_char_idx.saturating_sub(max_len / 2);
    if start + max_len > full.len() {
        start = full.len().saturating_sub(max_len);
    }
    let mut snippet = full.chars().skip(start).take(max_len).collect::<String>();
    if max_len >= 6 {
        if start > 0 && snippet.len() >= 3 {
            snippet.replace_range(0..3, "...");
        }
        if start + max_len < full.len() && snippet.len() >= 3 {
            let end = snippet.len();
            snippet.replace_range(end - 3..end, "...");
        }
    }
    snippet
}

fn build_grid_lines(frames: &[SimFrame], max_width: usize, theme: &Theme) -> Vec<Line<'static>> {
    if frames.is_empty() || max_width == 0 {
        return Vec::new();
    }
    let mut min_coord: isize = 0;
    let mut max_coord: isize = 0;
    let mut initialized = false;
    for frame in frames {
        let origin = frame.origin as isize;
        let right = origin + frame.tape.len() as isize;
        if !initialized {
            min_coord = origin;
            max_coord = right;
            initialized = true;
        } else {
            min_coord = min_coord.min(origin);
            max_coord = max_coord.max(right);
        }
    }
    let display_width = (max_coord - min_coord + 1).max(1) as usize;
    let label_width = 4usize;
    let cell_width = 2usize;
    let ellipsis_width = 3usize;
    let available_no_ellipsis = max_width.saturating_sub(label_width);
    let max_cells_no_ellipsis = available_no_ellipsis / cell_width;
    if max_cells_no_ellipsis == 0 {
        return Vec::new();
    }
    let needs_ellipsis = display_width > max_cells_no_ellipsis;
    let available = if needs_ellipsis {
        max_width.saturating_sub(label_width + ellipsis_width)
    } else {
        available_no_ellipsis
    };
    let max_cells = available / cell_width;
    if max_cells == 0 {
        return Vec::new();
    }
    let view_width = display_width.min(max_cells).max(1);
    let (halt_style, halt_hit_style) = halt_styles(theme);
    let start = if needs_ellipsis {
        display_width.saturating_sub(view_width)
    } else {
        0
    };
    let end = start + view_width;
    let mut lines = Vec::new();
    for (idx, frame) in frames.iter().enumerate() {
        let mut spans = Vec::new();
        spans.push(Span::styled(
            format!("t{:02} ", idx),
            Style::default().fg(theme.border),
        ));
        if needs_ellipsis {
            spans.push(Span::styled("...", Style::default().fg(theme.border)));
        }
        let origin = frame.origin as isize;
        let head_coord = origin + frame.head as isize;
        let halt_coord = origin + frame.tape.len() as isize;
        let head_at_halt = head_coord == halt_coord;
        for cell_idx in start..end {
            let global_coord = min_coord + cell_idx as isize;
            let is_halt_cell = global_coord == halt_coord;
            let symbol = if global_coord >= origin && global_coord < halt_coord {
                let idx = (global_coord - origin) as usize;
                frame.tape.get(idx).copied().unwrap_or(0)
            } else {
                0
            };
            let mut style = if is_halt_cell {
                if head_at_halt && global_coord == halt_coord {
                    halt_hit_style
                } else {
                    halt_style
                }
            } else {
                symbol_style(symbol, theme)
            };
            if global_coord == head_coord {
                let head_fg = if !is_halt_cell && symbol == 0 {
                    Color::Black
                } else {
                    Color::White
                };
                style = style.fg(head_fg).add_modifier(Modifier::BOLD);
                spans.push(Span::styled(centered_cell(HEAD_DOT, cell_width), style));
            } else {
                spans.push(Span::styled(" ".repeat(cell_width), style));
            }
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn build_legend_lines(
    symbols: usize,
    theme: &Theme,
    halted: bool,
    output_value: Option<u64>,
    output_symbol: Option<u8>,
) -> Vec<Line<'static>> {
    let mut spans = Vec::new();
    spans.push(Span::styled(
        "legend: ",
        Style::default().fg(theme.title).add_modifier(Modifier::DIM),
    ));
    for symbol in 0..symbols {
        if symbol > 0 {
            spans.push(Span::raw(" "));
        }
        let label = symbol_char(symbol as u8).to_string();
        let fg = if symbol == 0 {
            Color::Black
        } else {
            theme.foreground
        };
        let style = symbol_style(symbol as u8, theme)
            .fg(fg)
            .add_modifier(Modifier::BOLD);
        spans.push(Span::styled(format!(" {label} "), style));
    }
    let (halt_style, halt_hit_style) = halt_styles(theme);
    let mut halt_line = Vec::new();
    halt_line.push(Span::styled(
        "halt: ",
        Style::default().fg(theme.title).add_modifier(Modifier::DIM),
    ));
    halt_line.push(Span::styled("  ", halt_style));
    halt_line.push(Span::raw(" "));
    halt_line.push(Span::styled(
        if halted { "true" } else { "false" },
        Style::default().fg(theme.foreground),
    ));
    halt_line.push(Span::raw("  "));
    halt_line.push(Span::styled(
        "output: ",
        Style::default().fg(theme.title).add_modifier(Modifier::DIM),
    ));
    halt_line.push(Span::styled("  ", halt_hit_style));
    let output_text = match (output_value, output_symbol) {
        (Some(value), Some(symbol)) => {
            let action = if symbol == 0 {
                Action::Cooperate.as_char()
            } else {
                Action::Defect.as_char()
            };
            format!(" {} (mod {symbols} = {symbol} = {action})", value)
        }
        (Some(value), None) => format!(" {value}"),
        (None, _) if !halted => " timeout -> D".to_string(),
        (None, _) => " n/a".to_string(),
    };
    halt_line.push(Span::styled(
        output_text,
        Style::default().fg(theme.foreground),
    ));
    vec![Line::from(spans), Line::from(halt_line)]
}

fn symbol_style(symbol: u8, theme: &Theme) -> Style {
    let bg = match symbol {
        0 => Color::White,
        1 => theme.accent,
        2 => theme.warning,
        3 => theme.title,
        _ => theme.selection_bg,
    };
    Style::default().bg(bg)
}

fn halt_styles(_theme: &Theme) -> (Style, Style) {
    let halt = Style::default()
        .bg(Color::DarkGray)
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM);
    let halt_hit = Style::default()
        .bg(Color::Gray)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);
    (halt, halt_hit)
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
mod tests {
    use super::*;

    fn line_to_string(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn rule_table_renders_with_borders() {
        let transitions = vec![
            TmTransition {
                write: 0,
                move_dir: TmMove::Right,
                next: 1,
            },
            TmTransition {
                write: 1,
                move_dir: TmMove::Left,
                next: 1,
            },
            TmTransition {
                write: 1,
                move_dir: TmMove::Stay,
                next: 0,
            },
            TmTransition {
                write: 0,
                move_dir: TmMove::Right,
                next: 2,
            },
        ];
        let lines =
            build_rule_table_lines(2, 2, &transitions, Style::default(), Style::default(), 80);
        assert_eq!(lines.len(), 8);
        let top = line_to_string(&lines[0]);
        let header = line_to_string(&lines[1]);
        let mid = line_to_string(&lines[2]);
        let bottom = line_to_string(lines.last().unwrap());
        assert!(top.starts_with('+') && top.ends_with('+'));
        assert!(mid.starts_with('+') && mid.ends_with('+'));
        assert!(bottom.starts_with('+') && bottom.ends_with('+'));
        assert_eq!(top.len(), bottom.len());
        assert_eq!(top.len(), header.len());
        assert!(header.contains("| {s, r} "));
        assert!(header.contains("| {n, w, m} "));
        let row = line_to_string(&lines[3]);
        assert!(row.contains("{1, 0}"));
    }

    #[test]
    fn step_table_renders_with_borders_and_tape() {
        let steps = vec![SimStep {
            step: 1,
            state: 2,
            head_before: 1,
            read: 1,
            next: 2,
            write: 0,
            move_dir: TmMove::Right,
            head_after: 2,
            tape: vec![0, 1, 1, 0],
        }];
        let lines = build_step_table_lines(&steps, 120, Style::default(), Style::default());
        assert_eq!(lines.len(), 5);
        let top = line_to_string(&lines[0]);
        let header = line_to_string(&lines[1]);
        let row = line_to_string(&lines[3]);
        let bottom = line_to_string(lines.last().unwrap());
        assert!(top.starts_with('+') && top.ends_with('+'));
        assert!(bottom.starts_with('+') && bottom.ends_with('+'));
        assert_eq!(top.len(), bottom.len());
        assert_eq!(top.len(), header.len());
        assert!(header.contains("| tape "));
        assert!(row.contains('●'));
    }

    #[test]
    fn evolution_head_clamps_at_left_edge() {
        let transitions = vec![
            TmTransition {
                write: 0,
                move_dir: TmMove::Left,
                next: 1,
            },
            TmTransition {
                write: 0,
                move_dir: TmMove::Left,
                next: 1,
            },
        ];
        let output_map = vec![Action::Cooperate, Action::Defect];
        let sim = simulate_tm(0, 2, 1, 0, 0, 8, &transitions, &output_map);
        assert!(sim.frames.iter().all(|frame| frame.origin == 0));
        let mut clamped = false;
        for window in sim.frames.windows(2) {
            if window[0].head == 0 && window[1].head == 0 {
                clamped = true;
                break;
            }
        }
        assert!(clamped, "expected head to clamp at left boundary");
    }

    #[test]
    fn non_halting_evolution_truncates_at_fixed_point() {
        let transitions = vec![
            TmTransition {
                write: 0,
                move_dir: TmMove::Stay,
                next: 1,
            },
            TmTransition {
                write: 1,
                move_dir: TmMove::Stay,
                next: 1,
            },
        ];
        let output_map = vec![Action::Cooperate, Action::Defect];
        let sim = simulate_tm(0, 2, 1, 0, 0, 64, &transitions, &output_map);
        assert!(!sim.halted);
        assert!(sim.frames.len() < 65);
        assert_eq!(sim.frames.len(), 3);
        assert!(sim
            .log_lines
            .iter()
            .any(|line| line.contains("fixed point at step 2")));
    }

    #[test]
    fn rules_table_border_aligns_in_right_column() {
        let transitions = vec![
            TmTransition {
                write: 0,
                move_dir: TmMove::Right,
                next: 1,
            },
            TmTransition {
                write: 1,
                move_dir: TmMove::Left,
                next: 1,
            },
            TmTransition {
                write: 1,
                move_dir: TmMove::Stay,
                next: 0,
            },
            TmTransition {
                write: 0,
                move_dir: TmMove::Right,
                next: 2,
            },
        ];
        let right_width = 32usize;
        let right_lines = build_rule_table_lines(
            2,
            2,
            &transitions,
            Style::default(),
            Style::default(),
            right_width,
        );
        assert!(!right_lines.is_empty());
        let left_width = 20usize;
        let gap = 2usize;
        let left_lines = vec![Line::from(""); right_lines.len()];
        let merged = merge_columns(left_lines, right_lines, left_width, right_width, gap);
        let border = line_to_string(&merged[0]);
        let idx = border.find('+').unwrap_or(usize::MAX);
        assert_eq!(idx, left_width + gap);
    }

    #[test]
    fn steps_table_border_aligns_in_left_column() {
        let steps = vec![SimStep {
            step: 1,
            state: 2,
            head_before: 1,
            read: 1,
            next: 2,
            write: 0,
            move_dir: TmMove::Right,
            head_after: 2,
            tape: vec![0, 1, 1, 0],
        }];
        let right_width = 48usize;
        let right_lines =
            build_step_table_lines(&steps, right_width, Style::default(), Style::default());
        assert!(!right_lines.is_empty());
        let left_width = 20usize;
        let gap = 2usize;
        let merged = merge_columns(right_lines, Vec::new(), left_width, right_width, gap);
        let border = line_to_string(&merged[0]);
        let idx = border.find('+').unwrap_or(usize::MAX);
        assert_eq!(idx, 0);
    }

    #[test]
    fn legend_shows_halt_and_output_value() {
        let lines = build_legend_lines(2, &Theme::default(), true, Some(7), Some(1));
        let summary = line_to_string(&lines[1]);
        assert!(summary.contains("halt:"));
        assert!(summary.contains("true"));
        assert!(summary.contains("output:"));
        assert!(summary.contains("7"));
        assert!(summary.contains("mod 2 = 1"));
        assert!(summary.contains("= D"));
    }

    #[test]
    fn legend_shows_timeout_for_non_halting_run() {
        let lines = build_legend_lines(2, &Theme::default(), false, None, None);
        let summary = line_to_string(&lines[1]);
        assert!(summary.contains("false"));
        assert!(summary.contains("timeout -> D"));
    }
}
