use serde::{Deserialize, Serialize};

use crate::config::{BuiltinKind, StrategySpec, StrategySpecKind};
use crate::game::Action;
use crate::strategy::{InputMode, TmMove};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StrategyIntrospectionKind {
    Builtin,
    Random,
    Fsm,
    Memory,
    OneSidedTm,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategyIntrospection {
    pub id: String,
    pub kind: StrategyIntrospectionKind,
    pub parameters: StrategyIntrospectionParameters,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StrategyIntrospectionParameters {
    Builtin { builtin: BuiltinKind },
    Random { p_cooperate: f32 },
    Fsm {
        states: usize,
        start_state: usize,
        input_mode: InputMode,
        outputs: Vec<Action>,
        transitions: Vec<Vec<usize>>,
    },
    Memory {
        n: usize,
        initial: Action,
        table: Vec<Action>,
    },
    OneSidedTm {
        states: u16,
        symbols: u8,
        start_state: u16,
        blank: u8,
        fallback_symbol: u8,
        max_steps_per_round: u32,
        input_mode: InputMode,
        output_map: Vec<Action>,
        transitions: Vec<TmTransitionRecord>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rule_code: Option<u64>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TmTransitionRecord {
    pub state: u16,
    pub read: u8,
    pub write: u8,
    pub move_dir: TmMove,
    pub next: u16,
}

fn resolve_fsm_input_mode(input_mode: Option<InputMode>, transitions: &[Vec<usize>]) -> InputMode {
    if let Some(mode) = input_mode {
        return mode;
    }
    transitions
        .first()
        .map(|row| {
            if row.len() == 4 {
                InputMode::JointLastAction
            } else {
                InputMode::OpponentLastAction
            }
        })
        .unwrap_or(InputMode::OpponentLastAction)
}

pub fn introspect_strategy(spec: &StrategySpec) -> StrategyIntrospection {
    match &spec.kind {
        StrategySpecKind::Builtin { builtin } => StrategyIntrospection {
            id: spec.id.clone(),
            kind: StrategyIntrospectionKind::Builtin,
            parameters: StrategyIntrospectionParameters::Builtin { builtin: *builtin },
        },
        StrategySpecKind::Random { p_cooperate } => StrategyIntrospection {
            id: spec.id.clone(),
            kind: StrategyIntrospectionKind::Random,
            parameters: StrategyIntrospectionParameters::Random {
                p_cooperate: *p_cooperate,
            },
        },
        StrategySpecKind::Fsm {
            num_states,
            start_state,
            outputs,
            input_mode,
            transitions,
        } => {
            let resolved_mode = resolve_fsm_input_mode(*input_mode, transitions);
            let states = if *num_states > 0 {
                *num_states
            } else {
                outputs.len()
            };
            StrategyIntrospection {
                id: spec.id.clone(),
                kind: StrategyIntrospectionKind::Fsm,
                parameters: StrategyIntrospectionParameters::Fsm {
                    states,
                    start_state: *start_state,
                    input_mode: resolved_mode,
                    outputs: outputs.clone(),
                    transitions: transitions.clone(),
                },
            }
        }
        StrategySpecKind::Memory { n, initial, table } => StrategyIntrospection {
            id: spec.id.clone(),
            kind: StrategyIntrospectionKind::Memory,
            parameters: StrategyIntrospectionParameters::Memory {
                n: *n,
                initial: *initial,
                table: table.clone(),
            },
        },
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
            let fallback_symbol = fallback_symbol.unwrap_or(*blank);
            let mut normalized = Vec::new();
            let symbols_usize = *symbols as usize;
            for state in 1..=*states {
                for read in 0..*symbols {
                    let idx = (state as usize - 1)
                        .saturating_mul(symbols_usize)
                        .saturating_add(read as usize);
                    if let Some(rule) = transitions.get(idx) {
                        normalized.push(TmTransitionRecord {
                            state,
                            read,
                            write: rule.write,
                            move_dir: rule.move_dir,
                            next: rule.next,
                        });
                    }
                }
            }
            StrategyIntrospection {
                id: spec.id.clone(),
                kind: StrategyIntrospectionKind::OneSidedTm,
                parameters: StrategyIntrospectionParameters::OneSidedTm {
                    states: *states,
                    symbols: *symbols,
                    start_state: *start_state,
                    blank: *blank,
                    fallback_symbol,
                    max_steps_per_round: *max_steps_per_round,
                    input_mode: *input_mode,
                    output_map: output_map.clone(),
                    transitions: normalized,
                    rule_code: *rule_code,
                },
            }
        }
    }
}

fn input_mode_label(mode: InputMode) -> &'static str {
    match mode {
        InputMode::OpponentLastAction => "opponent_last_action",
        InputMode::SelfLastAction => "self_last_action",
        InputMode::JointLastAction => "joint_last_action",
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

fn builtin_as_fsm(builtin: BuiltinKind) -> (usize, InputMode, Vec<Action>, Vec<Vec<usize>>) {
    match builtin {
        BuiltinKind::AllC => (
            0,
            InputMode::JointLastAction,
            vec![Action::Cooperate],
            vec![vec![0, 0, 0, 0]],
        ),
        BuiltinKind::AllD => (
            0,
            InputMode::JointLastAction,
            vec![Action::Defect],
            vec![vec![0, 0, 0, 0]],
        ),
        BuiltinKind::TitForTat => (
            0,
            InputMode::JointLastAction,
            vec![Action::Cooperate, Action::Defect],
            vec![vec![0, 1, 0, 1], vec![0, 1, 0, 1]],
        ),
        BuiltinKind::GrimTrigger => (
            0,
            InputMode::JointLastAction,
            vec![Action::Cooperate, Action::Defect],
            vec![vec![0, 1, 0, 1], vec![1, 1, 1, 1]],
        ),
        BuiltinKind::WinStayLoseShift => (
            0,
            InputMode::JointLastAction,
            vec![Action::Cooperate, Action::Defect],
            vec![vec![0, 1, 1, 0], vec![1, 0, 0, 1]],
        ),
    }
}

fn build_fsm_graph_lines(
    states: usize,
    start_state: usize,
    input_mode: InputMode,
    outputs: &[Action],
    transitions: &[Vec<usize>],
) -> Vec<String> {
    let alphabet = input_mode.alphabet_size();
    let mut headers = Vec::with_capacity(alphabet + 1);
    headers.push("state".to_string());
    for idx in 0..alphabet {
        headers.push(idx.to_string());
    }
    let mut rows = Vec::new();
    for state_idx in 0..states {
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
    let mut lines = Vec::new();
    lines.push("graph:".to_string());
    lines.push(format!("legend: 0..{}", alphabet.saturating_sub(1)));
    lines.push(format!("start_state: {start_state}"));
    lines.extend(build_table(&headers, &rows));
    lines
}

fn build_tm_graph_lines(states: u16, symbols: u8, transitions: &[TmTransitionRecord]) -> Vec<String> {
    let mut headers = Vec::with_capacity(symbols as usize + 1);
    headers.push("state".to_string());
    for sym in 0..symbols {
        headers.push(sym.to_string());
    }
    let mut by_write: Vec<Vec<Vec<u16>>> =
        vec![vec![Vec::new(); symbols as usize]; states as usize];
    for rule in transitions {
        let write_idx = (rule.write as usize).min(symbols.saturating_sub(1) as usize);
        by_write[rule.state as usize - 1][write_idx].push(rule.next);
    }
    let mut rows = Vec::new();
    for state in 1..=states as usize {
        let mut row = Vec::with_capacity(symbols as usize + 1);
        row.push(state.to_string());
        for write in 0..symbols as usize {
            let mut targets = by_write[state - 1][write].clone();
            targets.sort_unstable();
            targets.dedup();
            let cell = if targets.is_empty() {
                "-".to_string()
            } else {
                targets
                    .into_iter()
                    .map(|next| if next == 0 { "H".to_string() } else { next.to_string() })
                    .collect::<Vec<_>>()
                    .join(",")
            };
            row.push(cell);
        }
        rows.push(row);
    }
    let mut lines = Vec::new();
    lines.push("graph:".to_string());
    lines.push("legend: edge label = write symbol (ap); H=HALT".to_string());
    lines.extend(build_table(&headers, &rows));
    lines
}

fn build_tm_encoding_table(transitions: &[TmTransitionRecord]) -> Vec<String> {
    let headers = vec!["s".to_string(), "ap".to_string(), "sp".to_string()];
    let mut rows = Vec::new();
    for rule in transitions {
        rows.push(vec![
            rule.state.to_string(),
            rule.write.to_string(),
            rule.next.to_string(),
        ]);
    }
    build_table(&headers, &rows)
}

fn build_tm_rules_table(transitions: &[TmTransitionRecord]) -> Vec<String> {
    let headers = vec![
        "state".to_string(),
        "read".to_string(),
        "next".to_string(),
        "write".to_string(),
        "move".to_string(),
    ];
    let mut rows = Vec::new();
    for rule in transitions {
        let move_label = match rule.move_dir {
            TmMove::Left => "L",
            TmMove::Right => "R",
            TmMove::Stay => "S",
        };
        rows.push(vec![
            rule.state.to_string(),
            rule.read.to_string(),
            rule.next.to_string(),
            rule.write.to_string(),
            move_label.to_string(),
        ]);
    }
    build_table(&headers, &rows)
}

fn build_memory_graph_lines(n: usize, initial: Action, table: &[Action]) -> Vec<String> {
    let states = 4usize.checked_pow(n as u32).unwrap_or(usize::MAX);
    let mut lines = Vec::new();
    lines.push("graph:".to_string());
    lines.push("legend: 0=CC 1=CD 2=DC 3=DD".to_string());
    if states > 64 {
        lines.push(format!("graph omitted ({} states)", states));
        lines.push(format!("note: initial action = {}", initial.as_char()));
        return lines;
    }
    let headers = vec![
        "state".to_string(),
        "0".to_string(),
        "1".to_string(),
        "2".to_string(),
        "3".to_string(),
    ];
    let mask = if n == 0 { 0u64 } else { (1u64 << (2 * n)) - 1 };
    let mut rows = Vec::new();
    for idx in 0..states {
        let output = table.get(idx).copied().unwrap_or(initial);
        let mut row = Vec::new();
        row.push(format!("{idx}({})", output.as_char()));
        for input in 0..4usize {
            let next = if n == 0 {
                0
            } else {
                (((idx as u64) << 2) | input as u64) & mask
            };
            row.push(next.to_string());
        }
        rows.push(row);
    }
    lines.extend(build_table(&headers, &rows));
    lines.push(format!("note: initial action = {}", initial.as_char()));
    lines
}

pub fn format_strategy_introspection(intro: &StrategyIntrospection) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("id: {}", intro.id));
    lines.push(format!(
        "kind: {}",
        match intro.kind {
            StrategyIntrospectionKind::Builtin => "builtin",
            StrategyIntrospectionKind::Random => "random",
            StrategyIntrospectionKind::Fsm => "fsm",
            StrategyIntrospectionKind::Memory => "memory",
            StrategyIntrospectionKind::OneSidedTm => "one_sided_tm",
        }
    ));
    match &intro.parameters {
        StrategyIntrospectionParameters::Builtin { builtin } => {
            lines.push(format!("builtin: {:?}", builtin));
            let (start_state, input_mode, outputs, transitions) = builtin_as_fsm(*builtin);
            lines.push(String::new());
            lines.extend(build_fsm_graph_lines(
                outputs.len(),
                start_state,
                input_mode,
                &outputs,
                &transitions,
            ));
        }
        StrategyIntrospectionParameters::Random { p_cooperate } => {
            lines.push(format!("p_cooperate: {:.3}", p_cooperate));
            lines.push(String::new());
            lines.push("graph:".to_string());
            lines.push("note: stochastic output; single-node self-loop".to_string());
        }
        StrategyIntrospectionParameters::Fsm {
            states,
            start_state,
            input_mode,
            outputs,
            transitions,
        } => {
            lines.push(format!("states: {}", states));
            lines.push(format!("start_state: {}", start_state));
            lines.push(format!("input_mode: {}", input_mode_label(*input_mode)));
            let outputs_str: String = outputs.iter().map(|a| a.as_char()).collect();
            lines.push(format!("outputs: {outputs_str}"));
            lines.push(String::new());
            lines.extend(build_fsm_graph_lines(
                *states,
                *start_state,
                *input_mode,
                outputs,
                transitions,
            ));
        }
        StrategyIntrospectionParameters::Memory { n, initial, table } => {
            lines.push(format!("n: {}", n));
            lines.push(format!("initial: {}", initial.as_char()));
            let table_str: String = table.iter().map(|a| a.as_char()).collect();
            lines.push(format!("table: {table_str}"));
            lines.push(String::new());
            lines.extend(build_memory_graph_lines(*n, *initial, table));
        }
        StrategyIntrospectionParameters::OneSidedTm {
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
            lines.push(format!("states: {}", states));
            lines.push(format!("symbols: {}", symbols));
            lines.push(format!("start_state: {}", start_state));
            lines.push(format!("blank: {}", blank));
            lines.push(format!("fallback_symbol: {}", fallback_symbol));
            lines.push(format!("max_steps_per_round: {}", max_steps_per_round));
            lines.push(format!("input_mode: {}", input_mode_label(*input_mode)));
            if let Some(code) = rule_code {
                lines.push(format!("rule_code: {}", code));
            }
            let output_str: String = output_map.iter().map(|a| a.as_char()).collect();
            lines.push(format!("output_map: {output_str}"));
            lines.push("transition_encoding: {s, ap} -> sp".to_string());
            lines.extend(build_tm_encoding_table(transitions));
            lines.push(String::new());
            lines.extend(build_tm_graph_lines(*states, *symbols, transitions));
            lines.push(String::new());
            lines.push("transitions:".to_string());
            lines.extend(build_tm_rules_table(transitions));
        }
    }
    lines
}
