//! Shared helpers used by the per-topic test modules under `tests`.

use crate::config::StrategySpec;
use crate::game::{Action, PayoffMatrix};
use crate::history::History;
use crate::output::{StrategyResult, TournamentResults};
use crate::strategy::Strategy;
use crate::tournament::{KernelRunMode, TournamentKernel};

#[cfg(target_os = "macos")]
use crate::config::{NormalizedConfig, StrategySpecKind};
#[cfg(target_os = "macos")]
use crate::strategy::InputMode;

pub(super) fn record_round(history: &mut History, a: Action, b: Action) {
    history.push(a, b);
}

pub(super) fn strategy_from_spec(spec: &StrategySpec) -> Box<dyn Strategy> {
    crate::tournament::build_strategy(spec, 0)
}

pub(super) fn simulate_match_from_specs(
    a_spec: &StrategySpec,
    b_spec: &StrategySpec,
    payoff: PayoffMatrix,
    rounds: u32,
) -> (i64, i64) {
    let mut a = strategy_from_spec(a_spec);
    let mut b = strategy_from_spec(b_spec);
    let mut history = History::new(usize::MAX);
    let mut a_total = 0i64;
    let mut b_total = 0i64;
    for _ in 0..rounds {
        let a_action = a.next_action(&history, true);
        let b_action = b.next_action(&history, false);
        let (a_payoff, b_payoff) = crate::game::payoffs_with_timeouts(
            payoff,
            a_action,
            b_action,
            a.last_halted(),
            b.last_halted(),
        );
        a_total += a_payoff as i64;
        b_total += b_payoff as i64;
        history.push(a_action, b_action);
    }
    (a_total, b_total)
}

#[cfg(target_os = "macos")]
pub(super) fn metal_totals_or_skip(
    cfg: &NormalizedConfig,
    pairs: &[(usize, usize)],
) -> Option<Vec<(i64, i64)>> {
    // GitHub Actions macOS runners expose a Metal device but reject actual
    // compute submissions with "command buffer reported error status". Skip
    // metal eval entirely when NIT_GAMES_DISABLE_METAL is set so CI stays green
    // without losing local coverage.
    if std::env::var_os("NIT_GAMES_DISABLE_METAL").is_some() {
        return None;
    }
    match super::metal::metal_batch_totals_for_test(cfg, pairs) {
        Ok(Some(totals)) => Some(totals),
        Ok(None) => None,
        Err(err)
            if err.contains("Metal device unavailable")
                || err.contains("command buffer reported error status") =>
        {
            None
        }
        Err(err) => panic!("metal eval: {err}"),
    }
}

#[cfg(target_os = "macos")]
pub(super) fn simple_four_state_fsm_spec(id: String) -> StrategySpec {
    use Action::{Cooperate, Defect};
    StrategySpec {
        id,
        name: None,
        kind: StrategySpecKind::Fsm {
            num_states: 4,
            start_state: 0,
            outputs: vec![Cooperate, Defect, Cooperate, Defect],
            input_mode: Some(InputMode::OpponentLastAction),
            transitions: vec![vec![0, 1], vec![2, 3], vec![0, 1], vec![2, 3]],
            index: None,
        },
    }
}

// The notebook reference treats state 0's output as the "initial action" even
// when no transition leads back to state 0 — the resulting `Vec<Option<Action>>`
// preserves that quirk so downstream tests can detect strategies whose buggy
// initial action diverges from the canonical decoder.
pub(super) fn notebook_buggy_state_outputs(
    outputs: &[Action],
    transitions: &[Vec<usize>],
) -> Vec<Option<Action>> {
    let mut recovered = vec![None; outputs.len()];
    for row in transitions {
        for &next in row {
            if let Some(slot) = recovered.get_mut(next) {
                *slot = outputs.get(next).copied();
            }
        }
    }
    recovered
}

pub(super) fn notebook_buggy_initial_action(
    outputs: &[Action],
    transitions: &[Vec<usize>],
) -> Action {
    notebook_buggy_state_outputs(outputs, transitions)
        .first()
        .and_then(|value| *value)
        .unwrap_or(Action::Defect)
}

pub(super) fn notebook_rules_from_outputs_and_transitions(
    outputs: &[Action],
    transitions: &[Vec<usize>],
) -> Vec<((usize, usize), (usize, usize))> {
    let states = outputs.len();
    let actions = transitions.first().map(Vec::len).unwrap_or(0);
    let mut rules = Vec::with_capacity(states * actions);
    for state in 0..states {
        for input in 0..actions {
            let next = transitions
                .get(state)
                .and_then(|row| row.get(input))
                .copied()
                .unwrap_or(state);
            let output_digit = match outputs.get(next).copied().unwrap_or(Action::Cooperate) {
                Action::Cooperate => 0,
                Action::Defect => 1,
            };
            rules.push(((state + 1, input), (next + 1, output_digit)));
        }
    }
    rules
}

#[allow(clippy::type_complexity)]
pub(super) fn notebook_buggy_fsm_to_index_from_rules(
    rules: &[((usize, usize), (usize, usize))],
    states: usize,
    actions: usize,
) -> u64 {
    let mut rhs = vec![(1usize, 0usize); states.saturating_mul(actions)];
    for &((state, input), value) in rules {
        let idx = (state - 1).saturating_mul(actions).saturating_add(input);
        rhs[idx] = value;
    }

    let transitions_code = rhs
        .iter()
        .map(|(next, _)| next - 1)
        .fold(0u64, |acc, digit| {
            acc.saturating_mul(states as u64)
                .saturating_add(digit as u64)
        });
    let mut outputs_per_state = vec![0usize; states];
    for &(next, output) in &rhs {
        outputs_per_state[next - 1] = output;
    }
    let outputs_code = outputs_per_state.into_iter().fold(0u64, |acc, digit| {
        acc.saturating_mul(actions as u64)
            .saturating_add(digit as u64)
    });

    1 + transitions_code.saturating_mul((actions as u64).pow(states as u32)) + outputs_code
}

pub(super) fn run_tournament_from_toml(src: &str) -> TournamentResults {
    let cfg = crate::config::GamesConfig::from_toml(src).expect("parse tournament config");
    TournamentKernel::new(cfg).run(KernelRunMode::Sequential {
        event_writer: None,
        history_writer: None,
    })
}

pub(super) fn halting_tm_tournament_toml(include_bad: bool) -> String {
    let mut src = String::from(
        r#"
schema_version = 1
game = "ipd"
rounds = 3
repetitions = 1
self_play = true

[[strategy]]
id = "tm_c"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 4
transitions = [
  { state=1, read=0, write=0, move="R", next=1 },
  { state=1, read=1, write=0, move="R", next=1 },
]

[[strategy]]
id = "tm_d"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 4
transitions = [
  { state=1, read=0, write=1, move="R", next=1 },
  { state=1, read=1, write=1, move="R", next=1 },
]
"#,
    );
    if include_bad {
        src.push_str(
            r#"
[[strategy]]
id = "tm_bad"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 4
rule_code = 0
"#,
        );
    }
    src
}

pub(super) fn tm_family_1x2_reference_toml(rounds: u32) -> String {
    let mut src = format!(
        r#"schema_version = 1
game = "ipd"
rounds = {rounds}
repetitions = 1
self_play = true

[engine]
score_aggregation = "mean"

"#
    );
    for rule_code in 0..=15 {
        src.push_str(&format!(
            r#"[[strategy]]
id = "tm_{rule_code}"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 1000
rule_code = {rule_code}

"#
        ));
    }
    src
}

pub(super) fn ranked_strategy<'a>(results: &'a TournamentResults, id: &str) -> &'a StrategyResult {
    results
        .ranking
        .iter()
        .find(|entry| entry.id == id)
        .unwrap_or_else(|| panic!("missing strategy result for {id}"))
}
