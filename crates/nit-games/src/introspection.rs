//! Strategy introspection and human-readable formatting.

use serde::{Deserialize, Serialize};

use crate::config::{StrategySpec, StrategySpecKind};
use crate::game::Action;
use crate::strategy::TmMove;

/// Strategy family discriminant.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StrategyIntrospectionKind {
    Fsm,
    Ca,
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
    Fsm {
        states: usize,
        start_state: usize,
        outputs: Vec<Action>,
        transitions: Vec<Vec<usize>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        index: Option<u64>,
    },
    Ca {
        n: u64,
        k: u8,
        r: f32,
        t: u32,
    },
    OneSidedTm {
        states: u16,
        symbols: u8,
        start_state: u16,
        blank: u8,
        fallback_symbol: u8,
        max_steps_per_round: u32,
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

/// Expand a flat TM transition table into per-`(state, read)` records.
fn expand_tm_transitions(
    states: u16,
    symbols: u8,
    transitions: &[crate::strategy::TmTransition],
) -> Vec<TmTransitionRecord> {
    let symbols_usize = symbols as usize;
    let mut out = Vec::new();
    for state in 1..=states {
        for read in 0..symbols {
            let idx = (state as usize - 1)
                .saturating_mul(symbols_usize)
                .saturating_add(read as usize);
            if let Some(rule) = transitions.get(idx) {
                out.push(TmTransitionRecord {
                    state,
                    read,
                    write: rule.write,
                    move_dir: rule.move_dir,
                    next: rule.next,
                });
            }
        }
    }
    out
}

pub fn introspect_strategy(spec: &StrategySpec) -> StrategyIntrospection {
    let id = spec.id.clone();
    match &spec.kind {
        StrategySpecKind::Fsm {
            num_states,
            start_state,
            outputs,
            transitions,
            index,
            ..
        } => {
            let states = if *num_states > 0 {
                *num_states
            } else {
                outputs.len()
            };
            StrategyIntrospection {
                id,
                kind: StrategyIntrospectionKind::Fsm,
                parameters: StrategyIntrospectionParameters::Fsm {
                    states,
                    start_state: *start_state,
                    outputs: outputs.clone(),
                    transitions: transitions.clone(),
                    index: *index,
                },
            }
        }
        StrategySpecKind::Ca { n, k, r, t } => StrategyIntrospection {
            id,
            kind: StrategyIntrospectionKind::Ca,
            parameters: StrategyIntrospectionParameters::Ca {
                n: *n,
                k: *k,
                r: *r,
                t: *t,
            },
        },
        StrategySpecKind::OneSidedTm {
            states,
            symbols,
            start_state,
            blank,
            fallback_symbol,
            max_steps_per_round,
            transitions,
            rule_code,
            ..
        } => {
            let fallback_symbol = fallback_symbol.unwrap_or(*blank);
            let normalized = expand_tm_transitions(*states, *symbols, transitions);
            StrategyIntrospection {
                id,
                kind: StrategyIntrospectionKind::OneSidedTm,
                parameters: StrategyIntrospectionParameters::OneSidedTm {
                    states: *states,
                    symbols: *symbols,
                    start_state: *start_state,
                    blank: *blank,
                    fallback_symbol,
                    max_steps_per_round: *max_steps_per_round,
                    transitions: normalized,
                    rule_code: *rule_code,
                },
            }
        }
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

fn format_fsm_lines(
    lines: &mut Vec<String>,
    states: usize,
    start_state: usize,
    outputs: &[Action],
    transitions: &[Vec<usize>],
    index: Option<u64>,
) {
    lines.push(format!("states: {states}"));
    lines.push(format!("start_state: {}", start_state.saturating_add(1)));
    if let Some(index) = index {
        lines.push(format!("notebook_index: {index}"));
    }
    let outputs_str: String = outputs.iter().map(|a| a.as_char()).collect();
    lines.push(format!("outputs: {outputs_str}"));
    lines.push("input_semantics: opponent_last_action".to_string());
    lines.push(String::new());
    lines.push("graph:".to_string());
    lines.push("legend: 0=C, 1=D (opponent last action)".to_string());
    let headers = vec!["state".to_string(), "0".to_string(), "1".to_string()];
    let mut rows = Vec::new();
    for state_idx in 0..states {
        let output = outputs.get(state_idx).map(|a| a.as_char()).unwrap_or('?');
        let mut row = Vec::new();
        row.push(format!("{}({output})", state_idx + 1));
        let trans_row = transitions.get(state_idx);
        for input in 0..2 {
            row.push(
                trans_row
                    .and_then(|r| r.get(input))
                    .map(|n| (n + 1).to_string())
                    .unwrap_or_else(|| "-".to_string()),
            );
        }
        rows.push(row);
    }
    lines.extend(build_table(&headers, &rows));
}

fn format_ca_lines(lines: &mut Vec<String>, n: u64, k: u8, r: f32, t: u32) {
    lines.push(format!("rule_code: {n}"));
    lines.push(format!("symbols: {k}"));
    lines.push(format!("radius: {r}"));
    lines.push(format!("steps: {t}"));
    lines.push("input_semantics: Flatten[history] (global A,B order)".to_string());
    lines.push("output: last cell of ShrinkingCA final row".to_string());
}

fn format_tm_lines(lines: &mut Vec<String>, params: &StrategyIntrospectionParameters) {
    let StrategyIntrospectionParameters::OneSidedTm {
        states,
        symbols,
        start_state,
        blank,
        fallback_symbol,
        max_steps_per_round,
        transitions,
        rule_code,
    } = params
    else {
        return;
    };
    lines.push(format!("states: {states}"));
    lines.push(format!("symbols: {symbols}"));
    lines.push(format!("start_state: {start_state}"));
    lines.push(format!("blank: {blank}"));
    lines.push(format!("fallback_symbol: {fallback_symbol}"));
    lines.push(format!("max_steps_per_round: {max_steps_per_round}"));
    if let Some(code) = rule_code {
        lines.push(format!("rule_code: {code}"));
    }
    lines.push(
        "input_semantics: input = FromDigits[Flatten[history], 2]; head starts on the least-significant digit".to_string(),
    );
    lines.push(
        "output_semantics: empty history -> C; halted -> output_symbol 0 => C, non-zero => D; timeout -> Defect".to_string(),
    );
    lines.push(String::new());
    lines.push("transitions:".to_string());
    lines.extend(build_tm_rules_table(transitions));
}

pub fn format_strategy_introspection(intro: &StrategyIntrospection) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("id: {}", intro.id));
    lines.push(format!(
        "kind: {}",
        match intro.kind {
            StrategyIntrospectionKind::Fsm => "fsm",
            StrategyIntrospectionKind::Ca => "ca",
            StrategyIntrospectionKind::OneSidedTm => "tm",
        }
    ));
    match &intro.parameters {
        StrategyIntrospectionParameters::Fsm {
            states,
            start_state,
            outputs,
            transitions,
            index,
        } => format_fsm_lines(
            &mut lines,
            *states,
            *start_state,
            outputs,
            transitions,
            *index,
        ),
        StrategyIntrospectionParameters::Ca { n, k, r, t } => {
            format_ca_lines(&mut lines, *n, *k, *r, *t)
        }
        params @ StrategyIntrospectionParameters::OneSidedTm { .. } => {
            format_tm_lines(&mut lines, params)
        }
    }
    lines
}
