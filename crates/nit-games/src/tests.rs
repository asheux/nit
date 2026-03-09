use crate::config::{
    AcceleratorMode, EngineMode, FsmGroupingMode, GamesConfig, ScoreAggregation, StrategySpecKind,
};
use crate::game::{payoffs_with_timeouts, Action, PayoffMatrix};
use crate::history::History;
use crate::output::RuntimeAcceleratorBackend;
use crate::strategy::{
    decode_fsm_notebook_index, decode_tm_rule_code_wolfram, history_to_input_u64, CaStrategy,
    FsmStrategy, InputMode, OneSidedTmStrategy, Strategy, TmMove, TmTransition,
};
use crate::{
    accelerator_preflight, canonical_fsm_indices, group_canonical_fsm_indices_by_behavior,
    group_canonical_fsm_indices_by_behavior_with_mode, unique_fsm_behavior_representatives,
    unique_fsm_behavior_representatives_with_mode, KernelRunMode, TournamentKernel,
    TournamentRunner,
};

fn push_round(history: &mut History, a: Action, b: Action) {
    history.push(a, b);
}

fn strategy_from_spec(spec: &crate::config::StrategySpec) -> Box<dyn Strategy> {
    match &spec.kind {
        StrategySpecKind::Fsm {
            start_state,
            outputs,
            input_mode,
            transitions,
            ..
        } => Box::new(FsmStrategy::new(
            spec.id.clone(),
            *start_state,
            outputs.clone(),
            input_mode.unwrap_or(InputMode::OpponentLastAction),
            transitions.clone(),
        )),
        StrategySpecKind::Ca { n, k, r, t } => Box::new(CaStrategy::new(
            spec.id.clone(),
            *n,
            *k,
            (*r * 2.0).round() as u32,
            *t,
        )),
        StrategySpecKind::OneSidedTm {
            symbols,
            start_state,
            blank,
            fallback_symbol,
            max_steps_per_round,
            input_mode,
            output_map,
            transitions,
            ..
        } => Box::new(OneSidedTmStrategy::new(
            spec.id.clone(),
            *symbols,
            *start_state,
            *blank,
            fallback_symbol.unwrap_or(*blank),
            *max_steps_per_round,
            *input_mode,
            output_map.clone(),
            transitions.clone(),
        )),
    }
}

fn simulate_match_from_specs(
    a_spec: &crate::config::StrategySpec,
    b_spec: &crate::config::StrategySpec,
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
        let (a_payoff, b_payoff) =
            payoffs_with_timeouts(payoff, a_action, b_action, a.last_halted(), b.last_halted());
        a_total += a_payoff as i64;
        b_total += b_payoff as i64;
        history.push(a_action, b_action);
    }
    (a_total, b_total)
}

#[cfg(target_os = "macos")]
fn metal_totals_or_skip(
    cfg: &crate::config::NormalizedConfig,
    pairs: &[(usize, usize)],
) -> Option<Vec<(i64, i64)>> {
    match crate::tournament::metal_batch_totals_for_test(cfg, pairs) {
        Ok(Some(totals)) => Some(totals),
        Ok(None) => None,
        Err(err) if err.contains("Metal device unavailable") => None,
        Err(err) => panic!("metal eval: {err}"),
    }
}

#[cfg(target_os = "macos")]
fn simple_four_state_fsm_spec(id: String) -> crate::config::StrategySpec {
    crate::config::StrategySpec {
        id,
        name: None,
        kind: StrategySpecKind::Fsm {
            num_states: 4,
            start_state: 0,
            outputs: vec![
                Action::Cooperate,
                Action::Defect,
                Action::Cooperate,
                Action::Defect,
            ],
            input_mode: Some(InputMode::OpponentLastAction),
            transitions: vec![vec![0, 1], vec![2, 3], vec![0, 1], vec![2, 3]],
            index: None,
        },
    }
}

fn notebook_buggy_state_outputs(
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

fn notebook_buggy_initial_action(outputs: &[Action], transitions: &[Vec<usize>]) -> Action {
    notebook_buggy_state_outputs(outputs, transitions)
        .first()
        .and_then(|value| *value)
        .unwrap_or(Action::Defect)
}

fn notebook_rules_from_outputs_and_transitions(
    outputs: &[Action],
    transitions: &[Vec<usize>],
) -> Vec<((usize, usize), (usize, usize))> {
    let states = outputs.len();
    let actions = transitions.first().map(|row| row.len()).unwrap_or(0);
    let mut rules = Vec::new();
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

fn notebook_buggy_fsm_to_index_from_rules(
    rules: &[((usize, usize), (usize, usize))],
    states: usize,
    actions: usize,
) -> u64 {
    let mut rhs = vec![(1usize, 0usize); states.saturating_mul(actions)];
    for &((state, input), value) in rules {
        let idx = (state - 1).saturating_mul(actions).saturating_add(input);
        rhs[idx] = value;
    }
    let nxt = rhs.iter().map(|(next, _)| next - 1).collect::<Vec<_>>();
    let mut out = vec![0usize; states];
    for &(next, output) in &rhs {
        out[next - 1] = output;
    }

    let transitions_code = nxt.into_iter().fold(0u64, |acc, digit| {
        acc.saturating_mul(states as u64)
            .saturating_add(digit as u64)
    });
    let outputs_code = out.into_iter().fold(0u64, |acc, digit| {
        acc.saturating_mul(actions as u64)
            .saturating_add(digit as u64)
    });

    1 + transitions_code.saturating_mul((actions as u64).pow(states as u32)) + outputs_code
}

#[test]
fn fsm_notebook_index_s1_k2_all_d_and_all_c() {
    let (outputs_0, transitions_0) = decode_fsm_notebook_index(0, 1, 2).expect("decode idx 0");
    let (outputs_1, transitions_1) = decode_fsm_notebook_index(1, 1, 2).expect("decode idx 1");

    assert_eq!(outputs_0, vec![Action::Defect]);
    assert_eq!(outputs_1, vec![Action::Cooperate]);
    assert_eq!(transitions_0, vec![vec![0, 0]]);
    assert_eq!(transitions_1, vec![vec![0, 0]]);

    let mut all_d = FsmStrategy::new(
        "all_d",
        0,
        outputs_0,
        InputMode::OpponentLastAction,
        transitions_0,
    );
    let mut all_c = FsmStrategy::new(
        "all_c",
        0,
        outputs_1,
        InputMode::OpponentLastAction,
        transitions_1,
    );

    let mut history = History::new(0);
    assert_eq!(all_d.next_action(&history, true), Action::Defect);
    assert_eq!(all_c.next_action(&history, true), Action::Cooperate);

    push_round(&mut history, Action::Cooperate, Action::Defect);
    assert_eq!(all_d.next_action(&history, true), Action::Defect);
    assert_eq!(all_c.next_action(&history, true), Action::Cooperate);
}

#[test]
fn notebook_fsmstrategyfunction_drops_unreferenced_start_state_output() {
    let outputs = vec![Action::Cooperate, Action::Defect];
    let transitions = vec![vec![1, 1], vec![1, 1]];

    assert_eq!(outputs[0], Action::Cooperate);
    assert_eq!(
        notebook_buggy_state_outputs(&outputs, &transitions),
        vec![None, Some(Action::Defect)]
    );
    assert_eq!(
        notebook_buggy_initial_action(&outputs, &transitions),
        Action::Defect
    );
}

#[test]
fn notebook_fsm_to_index_is_not_inverse_when_a_state_has_no_incoming_edges() {
    let transitions = vec![vec![1, 1], vec![1, 1]];
    let outputs_a = vec![Action::Cooperate, Action::Defect];
    let outputs_b = vec![Action::Defect, Action::Defect];

    let rules_a = notebook_rules_from_outputs_and_transitions(&outputs_a, &transitions);
    let rules_b = notebook_rules_from_outputs_and_transitions(&outputs_b, &transitions);

    assert_eq!(rules_a, rules_b);
    assert_eq!(
        notebook_buggy_fsm_to_index_from_rules(&rules_a, 2, 2),
        notebook_buggy_fsm_to_index_from_rules(&rules_b, 2, 2)
    );
}

#[test]
fn user_reported_code_02_top_strategies_include_buggy_start_output_cases() {
    let affected = unique_fsm_behavior_representatives(3, 2)
        .expect("fsm behavior representatives")
        .into_iter()
        .filter(|&index| {
            let Ok((outputs, transitions)) = decode_fsm_notebook_index(index, 3, 2) else {
                return false;
            };
            outputs.first() == Some(&Action::Cooperate)
                && notebook_buggy_initial_action(&outputs, &transitions) != outputs[0]
        })
        .collect::<Vec<_>>();

    assert_eq!(affected.len(), 36);
    assert!(affected.contains(&3563));
    assert!(affected.contains(&3234));
    assert!(affected.contains(&3882));
}

#[test]
fn fsm_uses_opponent_last_action_like_tft() {
    let outputs = vec![Action::Cooperate, Action::Defect];
    let transitions = vec![vec![0, 1], vec![0, 1]];

    let mut a_fsm = FsmStrategy::new(
        "tft_a",
        0,
        outputs.clone(),
        InputMode::OpponentLastAction,
        transitions.clone(),
    );
    let mut b_fsm = FsmStrategy::new(
        "tft_b",
        0,
        outputs,
        InputMode::OpponentLastAction,
        transitions,
    );

    let mut history = History::new(0);
    assert_eq!(a_fsm.next_action(&history, true), Action::Cooperate);
    assert_eq!(b_fsm.next_action(&history, false), Action::Cooperate);

    push_round(&mut history, Action::Cooperate, Action::Defect);
    assert_eq!(a_fsm.next_action(&history, true), Action::Defect);
    assert_eq!(b_fsm.next_action(&history, false), Action::Cooperate);

    push_round(&mut history, Action::Defect, Action::Cooperate);
    assert_eq!(a_fsm.next_action(&history, true), Action::Cooperate);
    assert_eq!(b_fsm.next_action(&history, false), Action::Defect);
}

#[test]
fn ca_rule_zero_and_255_match_notebook_behavior() {
    let mut history = History::new(0);
    push_round(&mut history, Action::Defect, Action::Cooperate);
    push_round(&mut history, Action::Defect, Action::Defect);

    let mut ca_zero = CaStrategy::new("ca0", 0, 2, 2, 1);
    let mut ca_255 = CaStrategy::new("ca255", 255, 2, 2, 1);

    assert_eq!(
        ca_zero.next_action(&History::new(0), true),
        Action::Cooperate
    );
    assert_eq!(
        ca_255.next_action(&History::new(0), true),
        Action::Cooperate
    );

    assert_eq!(ca_zero.next_action(&history, true), Action::Cooperate);
    assert_eq!(ca_255.next_action(&history, true), Action::Defect);
}

#[test]
fn ca_nontrivial_rule_matches_hand_computation() {
    // r=1 => neighborhood width 3. Rule n=85 maps each neighborhood to its rightmost bit.
    let mut history = History::new(0);
    // bits = [1,0,1,1]
    push_round(&mut history, Action::Defect, Action::Cooperate);
    push_round(&mut history, Action::Defect, Action::Defect);

    let mut ca = CaStrategy::new("ca85", 85, 2, 2, 1);
    // windows: [1,0,1] -> 1, [0,1,1] -> 1, final row [1,1], last cell 1 => Defect
    assert_eq!(ca.next_action(&history, true), Action::Defect);
}

#[test]
fn tm_always_move_right_write_zero_cooperates_and_halts() {
    let transitions = vec![
        TmTransition {
            write: 0,
            move_dir: TmMove::Right,
            next: 1,
        },
        TmTransition {
            write: 0,
            move_dir: TmMove::Right,
            next: 1,
        },
    ];
    let mut tm = OneSidedTmStrategy::new(
        "tm",
        2,
        1,
        0,
        0,
        4,
        InputMode::OpponentLastAction,
        vec![Action::Cooperate, Action::Defect],
        transitions,
    );

    let mut history = History::new(0);
    for _ in 0..3 {
        let action = tm.next_action(&history, true);
        assert_eq!(action, Action::Cooperate);
        assert!(tm.last_halted());
        push_round(&mut history, action, Action::Defect);
    }
}

#[test]
fn tm_rule_code_zero_times_out_and_defects() {
    let (transitions, _) = decode_tm_rule_code_wolfram(0, 1, 2);
    let mut tm = OneSidedTmStrategy::new(
        "tm_zero",
        2,
        1,
        0,
        0,
        4,
        InputMode::OpponentLastAction,
        vec![Action::Cooperate, Action::Defect],
        transitions,
    );
    let history = History::new(0);
    let action = tm.next_action(&history, true);
    assert_eq!(action, Action::Defect);
    assert!(!tm.last_halted());
}

#[test]
fn history_to_input_uses_flattened_pairs_binary_order() {
    let mut history = History::new(0);
    push_round(&mut history, Action::Cooperate, Action::Defect); // bits [0,1]
    assert_eq!(history_to_input_u64(&history), Some(1));
}

#[test]
fn timeout_scoring_matches_notebook_min_max_logic() {
    let payoff = PayoffMatrix::default_pd(); // min=-3, max=0
    let both_halted = payoffs_with_timeouts(payoff, Action::Cooperate, Action::Defect, true, true);
    assert_eq!(
        both_halted,
        payoff.payoffs(Action::Cooperate, Action::Defect)
    );

    let a_timeout = payoffs_with_timeouts(payoff, Action::Cooperate, Action::Defect, false, true);
    assert_eq!(a_timeout, (-3, 0));

    let b_timeout = payoffs_with_timeouts(payoff, Action::Cooperate, Action::Defect, true, false);
    assert_eq!(b_timeout, (0, -3));

    let both_timeout =
        payoffs_with_timeouts(payoff, Action::Cooperate, Action::Defect, false, false);
    assert_eq!(both_timeout, (-3, -3));
}

#[test]
fn tournament_progress_reports_zero_match_runs() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 5
repetitions = 1
self_play = false

[[strategy]]
id = "solo"
type = "auto"
states = 2
k = 2
index = 1
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse single-strategy config");
    let runner = TournamentRunner::new(cfg);
    let progress = runner
        .progress()
        .expect("progress should exist for empty schedule");
    assert_eq!(progress.match_index, 0);
    assert_eq!(progress.total_matches, 0);
    assert_eq!(progress.round, 0);
    assert_eq!(progress.rounds, 5);
    assert_eq!(progress.total_payoff_a, 0);
    assert_eq!(progress.total_payoff_b, 0);
}

#[test]
fn tournament_progress_advances_to_next_match_after_boundary() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 4
repetitions = 1
self_play = false

[[strategy]]
id = "s0"
type = "auto"
states = 2
k = 2
index = 0

[[strategy]]
id = "s1"
type = "auto"
states = 2
k = 2
index = 1

[[strategy]]
id = "s2"
type = "auto"
states = 2
k = 2
index = 18
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse three-strategy config");
    let mut runner = TournamentRunner::new(cfg);
    runner.step_rounds(4);

    let progress = runner
        .progress()
        .expect("progress should point to the next match");
    assert_eq!(progress.match_index, 2);
    assert_eq!(progress.round, 0);
    assert_eq!(progress.rounds, 4);
}

#[test]
fn fast_forward_progress_keeps_last_round_snapshot() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 4
repetitions = 1
self_play = true

[[strategy]]
id = "all_c"
type = "fsm"
index = 1
num_states = 1
k = 2
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse self-play config");
    let mut runner = TournamentRunner::new(cfg).with_match_history_previews(false);
    runner.step_rounds(4);

    let progress = runner.progress().expect("progress should exist");
    assert_eq!(progress.match_index, 1);
    assert_eq!(progress.round, 4);
    assert_eq!(progress.last_action_a, Some(Action::Cooperate));
    assert_eq!(progress.last_action_b, Some(Action::Cooperate));
    assert_eq!(progress.last_payoff_a, Some(-1));
    assert_eq!(progress.last_payoff_b, Some(-1));
    assert_eq!(progress.last_halted_a, Some(true));
    assert_eq!(progress.last_halted_b, Some(true));
}

#[test]
fn batch_progress_keeps_last_completed_match_instead_of_next_pending_match() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 4
repetitions = 1
self_play = false

[engine]
mode = "batch"
fast_eval = true
accelerator = "cpu"

[[strategy]]
id = "s0"
type = "auto"
states = 2
k = 2
index = 0

[[strategy]]
id = "s1"
type = "auto"
states = 2
k = 2
index = 1

[[strategy]]
id = "s2"
type = "auto"
states = 2
k = 2
index = 18
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse batch config");
    let mut runner = TournamentRunner::new(cfg).with_match_history_previews(false);
    runner.step_rounds(4);

    let progress = runner.progress().expect("progress should exist");
    assert_eq!(progress.match_index, 1);
    assert_eq!(progress.round, 4);
    assert_eq!(progress.rounds, 4);
    assert_eq!(progress.last_action_a, Some(Action::Defect));
    assert_eq!(progress.last_action_b, Some(Action::Cooperate));
}

#[test]
fn batch_runner_construction_handles_large_strategy_sets() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1
self_play = true

[engine]
mode = "batch"

[[strategy]]
id = "all_c"
type = "fsm"
index = 1
num_states = 1
k = 2
"#;

    let mut cfg = GamesConfig::from_toml(src).expect("parse batch config");
    let template = cfg
        .strategies
        .first()
        .cloned()
        .expect("template strategy should exist");
    cfg.strategies = (0..5_000)
        .map(|idx| {
            let mut spec = template.clone();
            spec.id = format!("s{idx}");
            spec
        })
        .collect();

    let runner = TournamentRunner::new(cfg);
    assert_eq!(runner.definitions().len(), 5_000);
    assert_eq!(runner.total_matches(), 25_000_000);
}

#[cfg(target_os = "macos")]
#[test]
fn metal_fsm_batch_matches_cpu_baseline() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 12
self_play = false

[engine]
mode = "batch"
fast_eval = true

[[strategy]]
id = "all_c"
type = "fsm"
index = 0
num_states = 1
k = 2

[[strategy]]
id = "all_d"
type = "fsm"
index = 1
num_states = 1
k = 2
"#;
    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let Some(totals) = metal_totals_or_skip(&cfg, &[(0, 1), (1, 0)]) else {
        return;
    };
    let expected = vec![
        simulate_match_from_specs(
            &cfg.strategies[0],
            &cfg.strategies[1],
            cfg.payoff,
            cfg.rounds,
        ),
        simulate_match_from_specs(
            &cfg.strategies[1],
            &cfg.strategies[0],
            cfg.payoff,
            cfg.rounds,
        ),
    ];
    assert_eq!(totals, expected);
}

#[cfg(target_os = "macos")]
#[test]
fn metal_ca_batch_matches_cpu_baseline() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 10
self_play = false

[engine]
mode = "batch"
fast_eval = true

[[strategy]]
id = "ca_30"
type = "ca"
n = 30
k = 2
r = 1.0
t = 4

[[strategy]]
id = "ca_110"
type = "ca"
n = 110
k = 2
r = 1.0
t = 4
"#;
    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let Some(totals) = metal_totals_or_skip(&cfg, &[(0, 1), (1, 0)]) else {
        return;
    };
    let expected = vec![
        simulate_match_from_specs(
            &cfg.strategies[0],
            &cfg.strategies[1],
            cfg.payoff,
            cfg.rounds,
        ),
        simulate_match_from_specs(
            &cfg.strategies[1],
            &cfg.strategies[0],
            cfg.payoff,
            cfg.rounds,
        ),
    ];
    assert_eq!(totals, expected);
}

#[cfg(target_os = "macos")]
#[test]
fn metal_tm_batch_matches_cpu_baseline() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 8
self_play = false

[engine]
mode = "batch"
fast_eval = true

[[strategy]]
id = "tm_0"
type = "tm"
states = 2
symbols = 2
blank = 0
max_steps_per_round = 16
rule_code = 0

[[strategy]]
id = "tm_3"
type = "tm"
states = 2
symbols = 2
blank = 0
max_steps_per_round = 16
rule_code = 3
"#;
    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let Some(totals) = metal_totals_or_skip(&cfg, &[(0, 1), (1, 0)]) else {
        return;
    };
    let expected = vec![
        simulate_match_from_specs(
            &cfg.strategies[0],
            &cfg.strategies[1],
            cfg.payoff,
            cfg.rounds,
        ),
        simulate_match_from_specs(
            &cfg.strategies[1],
            &cfg.strategies[0],
            cfg.payoff,
            cfg.rounds,
        ),
    ];
    assert_eq!(totals, expected);
}

#[test]
fn config_infers_fsm_from_fields_and_states_alias() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse fsm auto strategy");
    assert_eq!(cfg.strategies.len(), 1);
    match &cfg.strategies[0].kind {
        StrategySpecKind::Fsm {
            num_states,
            start_state,
            ..
        } => {
            assert_eq!(*num_states, 2);
            assert_eq!(*start_state, 0);
        }
        other => panic!("expected fsm, got {other:?}"),
    }
}

#[test]
fn config_infers_tm_from_auto_type() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[[strategy]]
id = "tm_auto"
type = "auto"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 8
rule_code = 1
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse tm auto strategy");
    assert_eq!(cfg.strategies.len(), 1);
    match &cfg.strategies[0].kind {
        StrategySpecKind::OneSidedTm {
            states,
            symbols,
            max_steps_per_round,
            ..
        } => {
            assert_eq!(*states, 1);
            assert_eq!(*symbols, 2);
            assert_eq!(*max_steps_per_round, 8);
        }
        other => panic!("expected one_sided_tm, got {other:?}"),
    }
}

#[test]
fn fsm_canonical_indices_s1_k2_match_allc_alld() {
    let canonical = canonical_fsm_indices(1, 2).expect("canonical indices");
    assert_eq!(canonical, vec![0, 1]);

    let groups = group_canonical_fsm_indices_by_behavior(1, 2)
        .expect("grouped canonical indices by behavior");
    assert_eq!(groups, vec![vec![0], vec![1]]);

    let reps = unique_fsm_behavior_representatives(1, 2).expect("behavior representatives");
    assert_eq!(reps, vec![0, 1]);
}

#[test]
fn fsm_unique_behavior_reps_s2_k2_match_notebook_reference_set_by_default() {
    let canonical = unique_fsm_behavior_representatives(2, 2).expect("behavior representatives");
    let expected = vec![
        0, 1, 18, 19, 22, 23, 26, 27, 30, 31, 34, 35, 38, 39, 46, 47, 50, 51, 54, 55, 58, 59,
    ];
    assert_eq!(canonical, expected);
}

#[test]
fn fsm_unique_behavior_reps_s3_k2_match_notebook_reference_count_by_default() {
    let canonical = unique_fsm_behavior_representatives(3, 2).expect("behavior representatives");
    assert_eq!(canonical.len(), 956);
}

#[test]
fn fsm_unique_behavior_reps_s3_k2_match_saved_code_02_reference_prefix_and_suffix() {
    let canonical = unique_fsm_behavior_representatives(3, 2).expect("behavior representatives");
    let expected_prefix = vec![
        0, 1, 651, 653, 723, 725, 794, 795, 796, 797, 798, 799, 802, 807, 810, 811, 814, 815, 818,
        819, 820, 821, 822, 823, 826, 827, 828, 829, 830, 831, 834, 835, 836, 837, 838, 839, 842,
        843, 844, 845,
    ];
    let expected_suffix = vec![
        3836, 3837, 3838, 3839, 3842, 3844, 3845, 3847, 3850, 3851, 3866, 3867, 3868, 3869, 3870,
        3871, 3874, 3875, 3882, 3883,
    ];
    assert_eq!(
        &canonical[..expected_prefix.len()],
        expected_prefix.as_slice()
    );
    assert_eq!(
        &canonical[canonical.len() - expected_suffix.len()..],
        expected_suffix.as_slice()
    );
}

#[test]
fn fsm_exact_grouping_mode_matches_exact_minimization_reference() {
    let reps_22 = unique_fsm_behavior_representatives_with_mode(2, 2, FsmGroupingMode::Moorem)
        .expect("exact behavior representatives");
    let expected_22 = vec![
        0, 1, 18, 19, 22, 23, 26, 27, 30, 31, 34, 35, 38, 39, 42, 43, 46, 47, 50, 51, 54, 55, 58,
        59, 62, 63,
    ];
    assert_eq!(reps_22, expected_22);

    let reps_32 = unique_fsm_behavior_representatives_with_mode(3, 2, FsmGroupingMode::Moorem)
        .expect("exact behavior representatives");
    assert_eq!(reps_32.len(), 1054);
}

#[test]
fn fsm_canonical_groupings_are_consistent_for_multiple_s_k() {
    for (states, actions) in [(2usize, 2usize), (3, 2)] {
        let canonical =
            canonical_fsm_indices(states, actions).expect("canonical indices for (s, k)");
        assert!(!canonical.is_empty());

        let mut sorted = canonical.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(canonical, sorted);

        for mode in [FsmGroupingMode::Wnbm, FsmGroupingMode::Moorem] {
            let groups = group_canonical_fsm_indices_by_behavior_with_mode(states, actions, mode)
                .expect("grouped canonical indices by behavior");
            assert!(!groups.is_empty());
            assert!(groups.iter().all(|group| !group.is_empty()));

            let mut flattened = groups.iter().flatten().copied().collect::<Vec<_>>();
            flattened.sort_unstable();
            assert_eq!(flattened, canonical);

            let reps = unique_fsm_behavior_representatives_with_mode(states, actions, mode)
                .expect("behavior representatives");
            assert_eq!(reps.len(), groups.len());
            assert!(groups
                .iter()
                .zip(reps.iter())
                .all(|(group, rep)| group.first() == Some(rep)));
        }
    }
}

#[test]
fn config_defaults_fsm_grouping_to_wnbm() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.fsm_grouping, FsmGroupingMode::Wnbm);
}

#[test]
fn config_parses_moorem_fsm_grouping_mode() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
fsm_grouping = "moorem"

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.fsm_grouping, FsmGroupingMode::Moorem);
}

#[test]
fn config_defaults_score_aggregation_to_mean() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.score_aggregation, ScoreAggregation::Mean);
}

#[test]
fn config_defaults_accelerator_to_auto() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.accelerator, AcceleratorMode::Auto);
}

#[test]
fn config_parses_cpu_accelerator() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
accelerator = "cpu"

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.accelerator, AcceleratorMode::Cpu);
}

#[test]
fn config_parses_metal_accelerator() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
accelerator = "metal"

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.accelerator, AcceleratorMode::Metal);
}

#[test]
fn metal_accelerator_preflight_rejects_fast_eval_false() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
accelerator = "metal"
fast_eval = false

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let err = accelerator_preflight(&cfg).expect_err("metal should require fast_eval");
    assert!(err.contains("fast_eval"));
}

#[test]
fn metal_accelerator_preflight_rejects_tm_step_limit_overflow() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
accelerator = "metal"
fast_eval = true

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 512
rule_code = 0
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let err = accelerator_preflight(&cfg).expect_err("metal should reject oversized TM runs");
    assert!(err.contains("max_steps_per_round"));
}

#[test]
fn config_parses_total_score_aggregation() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
score_aggregation = "total"

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.score_aggregation, ScoreAggregation::Total);
}

#[test]
fn metal_batch_path_can_be_disabled_in_config() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 4
repetitions = 1
self_play = false

[engine]
mode = "batch"
fast_eval = true
accelerator = "cpu"

[[strategy]]
id = "fsm_0"
type = "fsm"
index = 0
num_states = 1
k = 2

[[strategy]]
id = "fsm_1"
type = "fsm"
index = 1
num_states = 1
k = 2
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let totals = crate::tournament::metal_batch_totals_for_test(&cfg, &[(0, 1)])
        .expect("metal helper should not error");
    assert!(totals.is_none());
}

#[test]
fn kernel_runtime_stats_report_cpu_backend() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 4
repetitions = 1
self_play = false

[engine]
mode = "batch"
fast_eval = true
accelerator = "cpu"

[[strategy]]
id = "fsm_0"
type = "fsm"
index = 0
num_states = 1
k = 2

[[strategy]]
id = "fsm_1"
type = "fsm"
index = 1
num_states = 1
k = 2
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let kernel = TournamentKernel::new(cfg);
    let (_results, runtime) = kernel.run_with_runtime(KernelRunMode::Sequential {
        event_writer: None,
        history_writer: None,
    });

    assert_eq!(runtime.requested, AcceleratorMode::Cpu);
    assert_eq!(runtime.backend, RuntimeAcceleratorBackend::Cpu);
    assert_eq!(runtime.metal_matches, 0);
    assert_eq!(runtime.cpu_matches, 2);
}

#[test]
fn kernel_runtime_stats_report_metal_usage_when_available() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 4
repetitions = 1
self_play = false

[engine]
mode = "batch"
fast_eval = true
accelerator = "metal"

[[strategy]]
id = "fsm_0"
type = "fsm"
index = 0
num_states = 1
k = 2

[[strategy]]
id = "fsm_1"
type = "fsm"
index = 1
num_states = 1
k = 2
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let Some(_) = metal_totals_or_skip(&cfg, &[(0, 1), (1, 0)]) else {
        return;
    };
    let kernel = TournamentKernel::new(cfg);
    let (_results, runtime) = kernel.run_with_runtime(KernelRunMode::Sequential {
        event_writer: None,
        history_writer: None,
    });

    assert_eq!(runtime.requested, AcceleratorMode::Metal);
    assert_eq!(runtime.backend, RuntimeAcceleratorBackend::Metal);
    assert_eq!(runtime.metal_matches, 2);
    assert!(runtime.cpu_matches == 0);
    assert!(runtime.metal_policy_source.is_some());
    assert!(runtime.metal_policy_cache_key.is_some());
    assert!(runtime.metal_policy_cache_path.is_some());
}

#[cfg(target_os = "macos")]
#[test]
fn metal_large_homogeneous_four_state_fsm_roster_probe() {
    let mut cfg = GamesConfig::from_toml(
        r#"
schema_version = 1
game = "ipd"
rounds = 10
repetitions = 1
self_play = false
noise = 0.0

[engine]
mode = "batch"
fast_eval = true
accelerator = "metal"

[[strategy]]
id = "placeholder"
type = "fsm"
num_states = 1
outputs = ["C"]
transitions = [[0, 0]]
"#,
    )
    .expect("parse config");

    cfg.strategies = (0..52_000)
        .map(|idx| simple_four_state_fsm_spec(format!("fsm_{idx}")))
        .collect();

    let Some(totals) = metal_totals_or_skip(&cfg, &[(0, 1), (51_998, 51_999)]) else {
        return;
    };
    assert_eq!(totals.len(), 2);
}

#[cfg(target_os = "macos")]
#[test]
fn metal_large_homogeneous_four_state_fsm_roster_probe_with_self_play() {
    let mut cfg = GamesConfig::from_toml(
        r#"
schema_version = 1
game = "ipd"
rounds = 10
repetitions = 1
self_play = true
noise = 0.0

[engine]
mode = "batch"
fast_eval = true
accelerator = "metal"

[[strategy]]
id = "placeholder"
type = "fsm"
num_states = 1
outputs = ["C"]
transitions = [[0, 0]]
"#,
    )
    .expect("parse config");

    cfg.strategies = (0..52_000)
        .map(|idx| simple_four_state_fsm_spec(format!("fsm_{idx}")))
        .collect();

    let Some(totals) = metal_totals_or_skip(&cfg, &[(0, 0), (0, 1), (51_999, 51_999)]) else {
        return;
    };
    assert_eq!(totals.len(), 3);
}

#[cfg(target_os = "macos")]
#[test]
fn metal_large_homogeneous_four_state_fsm_full_chunk_probe() {
    let mut cfg = GamesConfig::from_toml(
        r#"
schema_version = 1
game = "ipd"
rounds = 10
repetitions = 1
self_play = false
noise = 0.0

[engine]
mode = "batch"
fast_eval = true
accelerator = "metal"

[[strategy]]
id = "placeholder"
type = "fsm"
num_states = 1
outputs = ["C"]
transitions = [[0, 0]]
"#,
    )
    .expect("parse config");

    cfg.strategies = (0..52_000)
        .map(|idx| simple_four_state_fsm_spec(format!("fsm_{idx}")))
        .collect();

    let pairs = (0..16_384usize)
        .map(|idx| (idx, 51_999usize.saturating_sub(idx)))
        .collect::<Vec<_>>();
    let Some(totals) = metal_totals_or_skip(&cfg, &pairs) else {
        return;
    };
    assert_eq!(totals.len(), pairs.len());
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "local Metal throughput profiling"]
fn metal_policy_profiles_four_state_fsm_on_local_device() {
    let mut cfg = GamesConfig::from_toml(
        r#"
schema_version = 1
game = "ipd"
rounds = 10
repetitions = 1
self_play = false
noise = 0.0

[engine]
mode = "batch"
fast_eval = true
accelerator = "metal"

[[strategy]]
id = "placeholder"
type = "fsm"
num_states = 1
outputs = ["C"]
transitions = [[0, 0]]
"#,
    )
    .expect("parse config");

    cfg.strategies = (0..52_000)
        .map(|idx| simple_four_state_fsm_spec(format!("fsm_{idx}")))
        .collect();

    let pairs = (0..524_288usize)
        .map(|idx| (idx % 52_000, 51_999usize.saturating_sub(idx % 52_000)))
        .collect::<Vec<_>>();

    let candidates = [
        (65_536usize, 3usize),
        (65_536usize, 4usize),
        (98_304usize, 4usize),
        (131_072usize, 3usize),
        (131_072usize, 4usize),
        (131_072usize, 5usize),
        (196_608usize, 4usize),
        (262_144usize, 3usize),
        (262_144usize, 4usize),
    ];

    const SAMPLES_PER_CANDIDATE: usize = 3;

    let mut baseline = None;
    let mut fastest = None;
    for (matches_per_batch, inflight_batches) in candidates {
        let mut total_elapsed = 0.0f64;
        let mut best_elapsed = f64::INFINITY;
        let mut checksum = (0i64, 0i64);
        for _ in 0..SAMPLES_PER_CANDIDATE {
            let Some((totals, elapsed)) = crate::tournament::metal_policy_probe_for_test(
                &cfg,
                &pairs,
                matches_per_batch,
                inflight_batches,
            )
            .expect("policy probe") else {
                return;
            };
            let sample_checksum = totals.iter().fold((0i64, 0i64), |acc, value| {
                (acc.0 + value.0, acc.1 + value.1)
            });
            if let Some(reference) = baseline.as_ref() {
                assert_eq!(&totals, reference, "policy changed Metal results");
            } else {
                baseline = Some(totals);
            }
            checksum = sample_checksum;
            let elapsed_secs = elapsed.as_secs_f64();
            total_elapsed += elapsed_secs;
            best_elapsed = best_elapsed.min(elapsed_secs);
        }
        let average_elapsed = total_elapsed / SAMPLES_PER_CANDIDATE as f64;
        let average_matches_per_second = pairs.len() as f64 / average_elapsed;
        let best_matches_per_second = pairs.len() as f64 / best_elapsed;
        println!(
            "metal_policy batch={} inflight={} avg={:.3}s avg_rate={:.0} best={:.3}s best_rate={:.0} checksum=({}, {})",
            matches_per_batch,
            inflight_batches,
            average_elapsed,
            average_matches_per_second,
            best_elapsed,
            best_matches_per_second,
            checksum.0,
            checksum.1
        );
        if fastest
            .as_ref()
            .map(|(_, _, best_average): &(usize, usize, f64)| average_elapsed < *best_average)
            .unwrap_or(true)
        {
            fastest = Some((matches_per_batch, inflight_batches, average_elapsed));
        }
    }
    if let Some((matches_per_batch, inflight_batches, average_elapsed)) = fastest {
        println!(
            "metal_policy best batch={} inflight={} avg={:.3}s",
            matches_per_batch, inflight_batches, average_elapsed
        );
    }
}

fn run_tournament_from_toml(src: &str) -> crate::output::TournamentResults {
    let cfg = GamesConfig::from_toml(src).expect("parse tournament config");
    TournamentKernel::new(cfg).run(KernelRunMode::Sequential {
        event_writer: None,
        history_writer: None,
    })
}

fn ranked_strategy<'a>(
    results: &'a crate::output::TournamentResults,
    id: &str,
) -> &'a crate::output::StrategyResult {
    results
        .ranking
        .iter()
        .find(|entry| entry.id == id)
        .unwrap_or_else(|| panic!("missing strategy result for {id}"))
}

#[test]
fn tournament_mean_aggregation_uses_per_round_payoff() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1
self_play = false

[[strategy]]
id = "all_c_a"
type = "fsm"
index = 1
num_states = 1
k = 2

[[strategy]]
id = "all_c_b"
type = "fsm"
index = 1
num_states = 1
k = 2
"#;

    let results = run_tournament_from_toml(src);
    let a = ranked_strategy(&results, "all_c_a");
    let b = ranked_strategy(&results, "all_c_b");

    assert_eq!(a.total_payoff, -4);
    assert_eq!(a.matches, 2);
    assert!((a.average_payoff - -1.0).abs() < 1e-9);
    assert!((a.total_payoff_for_scoreboard(ScoreAggregation::Mean, false) - -2.0).abs() < 1e-9);
    assert_eq!(b.total_payoff, -4);
    assert_eq!(b.matches, 2);
    assert!((b.average_payoff - -1.0).abs() < 1e-9);
    assert!((b.total_payoff_for_scoreboard(ScoreAggregation::Mean, false) - -2.0).abs() < 1e-9);
}

#[test]
fn tournament_mean_aggregation_handles_self_play_as_two_roles() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1
self_play = true

[[strategy]]
id = "all_c"
type = "fsm"
index = 1
num_states = 1
k = 2
"#;

    let results = run_tournament_from_toml(src);
    let entry = ranked_strategy(&results, "all_c");

    assert_eq!(entry.matches, 2);
    assert_eq!(entry.draws, 2);
    assert_eq!(entry.total_payoff, -4);
    assert!((entry.average_payoff - -1.0).abs() < 1e-9);
    assert!((entry.total_payoff_for_scoreboard(ScoreAggregation::Mean, false) - -2.0).abs() < 1e-9);
}

#[test]
fn match_history_preview_uses_machine_indices_and_outcome_digits() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 4
repetitions = 1
self_play = false

[[strategy]]
id = "fsm_0"
type = "fsm"
index = 0
num_states = 1
k = 2

[[strategy]]
id = "fsm_1"
type = "fsm"
index = 1
num_states = 1
k = 2
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let mut runner = TournamentRunner::new(cfg);
    runner.step_rounds(4);
    let previews = runner.drain_match_history_previews();
    let preview = previews.first().expect("match history preview");

    assert_eq!(preview.a, "0");
    assert_eq!(preview.b, "1");
    assert_eq!(preview.rounds_total, 4);
    assert_eq!(preview.outcomes, "2222");
    assert_eq!(preview.preview_outcomes(), "2222");
}

#[test]
fn leaderboard_snapshot_skips_pairwise_rebuilds() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 4
repetitions = 1
self_play = false

[[strategy]]
id = "fsm_0"
type = "fsm"
index = 0
num_states = 1
k = 2

[[strategy]]
id = "fsm_1"
type = "fsm"
index = 1
num_states = 1
k = 2
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let mut runner = TournamentRunner::new(cfg);
    runner.step_rounds(4);
    let leaderboard = runner.leaderboard();

    assert_eq!(leaderboard.ranking.len(), 2);
    assert!(leaderboard.pairwise.is_empty());
    assert!(leaderboard.dominance.is_empty());
}

#[test]
fn batch_runner_fast_forward_matches_kernel_results() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 12
repetitions = 2
self_play = true

[engine]
mode = "batch"
parallelism = "off"
fast_eval = true

[[strategy]]
id = "fsm_0"
type = "fsm"
index = 0
num_states = 1
k = 2

[[strategy]]
id = "fsm_1"
type = "fsm"
index = 1
num_states = 1
k = 2

[[strategy]]
id = "fsm_18"
type = "fsm"
index = 18
num_states = 2
k = 2
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.mode, EngineMode::Batch);

    let mut runner = TournamentRunner::new(cfg.clone()).with_match_history_previews(false);
    runner.step_rounds(u32::MAX);
    assert!(runner.is_done());

    let runner_results = runner.results();
    let kernel_results = TournamentKernel::new(cfg).run(KernelRunMode::Sequential {
        event_writer: None,
        history_writer: None,
    });

    assert_eq!(runner_results.ranking.len(), kernel_results.ranking.len());
    assert_eq!(runner_results.pairwise.len(), kernel_results.pairwise.len());
    assert_eq!(
        runner_results.dominance.len(),
        kernel_results.dominance.len()
    );
    for (runner_row, kernel_row) in runner_results
        .ranking
        .iter()
        .zip(kernel_results.ranking.iter())
    {
        assert_eq!(runner_row.id, kernel_row.id);
        assert_eq!(runner_row.total_payoff, kernel_row.total_payoff);
        assert!((runner_row.average_payoff - kernel_row.average_payoff).abs() < 1e-9);
        assert_eq!(runner_row.matches, kernel_row.matches);
        assert_eq!(runner_row.wins, kernel_row.wins);
        assert_eq!(runner_row.losses, kernel_row.losses);
        assert_eq!(runner_row.draws, kernel_row.draws);
    }
}

#[test]
fn strategy_result_score_respects_aggregation_and_adjustment() {
    let result = crate::output::StrategyResult {
        id: "s".into(),
        name: None,
        total_payoff: 12,
        average_payoff: 4.0,
        adjusted_total_payoff: Some(9.75),
        adjusted_average_payoff: Some(3.25),
        matches: 3,
        wins: 0,
        losses: 0,
        draws: 0,
        crashed: false,
        crash_count: 0,
        tm_metrics: None,
    };

    assert_eq!(result.score(ScoreAggregation::Total, false), 12.0);
    assert_eq!(result.score(ScoreAggregation::Mean, false), 4.0);
    assert_eq!(result.score(ScoreAggregation::Total, true), 9.75);
    assert_eq!(result.score(ScoreAggregation::Mean, true), 3.25);
    assert_eq!(
        result.total_payoff_for_scoreboard(ScoreAggregation::Total, false),
        12.0
    );
    assert_eq!(
        result.total_payoff_for_scoreboard(ScoreAggregation::Mean, false),
        12.0
    );
    assert_eq!(
        result.total_payoff_for_scoreboard(ScoreAggregation::Total, true),
        9.75
    );
    assert_eq!(
        result.total_payoff_for_scoreboard(ScoreAggregation::Mean, true),
        9.75
    );
    assert_eq!(result.formatted_score(ScoreAggregation::Mean, false), "4");
    assert_eq!(result.formatted_score(ScoreAggregation::Mean, true), "3.25");
    assert_eq!(
        result.formatted_total_payoff(ScoreAggregation::Mean, false),
        "12"
    );
    assert_eq!(
        result.formatted_total_payoff(ScoreAggregation::Mean, true),
        "9.75"
    );
}

#[test]
fn config_defaults_self_play_to_true() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert!(cfg.self_play);
}

#[test]
fn tournament_matches_code_02_fsm_2x2_reference_scoreboard() {
    let reps = unique_fsm_behavior_representatives(2, 2).expect("fsm behavior representatives");
    let mut src = String::from(
        r#"schema_version = 1
game = "ipd"
rounds = 2000
repetitions = 1
self_play = true

[engine]
score_aggregation = "mean"

"#,
    );
    for idx in reps {
        src.push_str(&format!(
            r#"[[strategy]]
id = "fsm_{idx}"
type = "fsm"
index = {idx}
num_states = 2
k = 2

"#
        ));
    }

    let results = run_tournament_from_toml(&src);
    assert_eq!(results.ranking.len(), 22);
    assert_eq!(results.ranking[0].id, "fsm_30");
    assert_eq!(results.ranking[1].id, "fsm_0");
    assert_eq!(results.ranking[2].id, "fsm_19");

    let fsm_30 = ranked_strategy(&results, "fsm_30");
    assert_eq!(fsm_30.matches, 44);
    assert_eq!(fsm_30.wins, 26);
    assert_eq!(fsm_30.losses, 4);
    assert_eq!(fsm_30.draws, 14);
    assert!((fsm_30.average_payoff - -0.8644772727272727).abs() < 1e-9);
    assert!(
        (fsm_30.total_payoff_for_scoreboard(ScoreAggregation::Mean, false) - -38.037).abs() < 1e-9
    );

    let fsm_0 = ranked_strategy(&results, "fsm_0");
    assert_eq!(fsm_0.matches, 44);
    assert_eq!(fsm_0.wins, 36);
    assert_eq!(fsm_0.losses, 0);
    assert_eq!(fsm_0.draws, 8);
    assert!((fsm_0.average_payoff - -1.0).abs() < 1e-9);
    assert!(
        (fsm_0.total_payoff_for_scoreboard(ScoreAggregation::Mean, false) - -44.0).abs() < 1e-9
    );

    let fsm_23 = ranked_strategy(&results, "fsm_23");
    assert_eq!(fsm_23.matches, 44);
    assert_eq!(fsm_23.wins, 24);
    assert_eq!(fsm_23.losses, 10);
    assert_eq!(fsm_23.draws, 10);
    assert!((fsm_23.average_payoff - -1.2046363636363637).abs() < 1e-9);
    assert!(
        (fsm_23.total_payoff_for_scoreboard(ScoreAggregation::Mean, false) - -53.004).abs() < 1e-9
    );

    let fsm_47 = ranked_strategy(&results, "fsm_47");
    assert_eq!(fsm_47.matches, 44);
    assert_eq!(fsm_47.wins, 4);
    assert_eq!(fsm_47.losses, 26);
    assert_eq!(fsm_47.draws, 14);
    assert!((fsm_47.average_payoff - -2.090068181818182).abs() < 1e-9);
    assert!(
        (fsm_47.total_payoff_for_scoreboard(ScoreAggregation::Mean, false) - -91.963).abs() < 1e-9
    );
}

#[test]
fn tournament_matches_saved_code_02_fsm_3x2_reference_scoreboard() {
    let reps = unique_fsm_behavior_representatives(3, 2).expect("fsm behavior representatives");
    let mut src = String::from(
        r#"schema_version = 1
game = "ipd"
rounds = 10
repetitions = 1
self_play = true

[engine]
score_aggregation = "mean"

"#,
    );
    for idx in reps {
        src.push_str(&format!(
            r#"[[strategy]]
id = "fsm_{idx}"
type = "fsm"
index = {idx}
num_states = 3
k = 2

"#
        ));
    }

    let results = run_tournament_from_toml(&src);
    assert_eq!(results.ranking.len(), 956);

    let expected_top = vec![
        "fsm_799", "fsm_823", "fsm_807", "fsm_847", "fsm_2743", "fsm_1294", "fsm_831", "fsm_3495",
        "fsm_1015", "fsm_855", "fsm_2751", "fsm_2767", "fsm_2959", "fsm_0", "fsm_3279",
    ];
    let actual_top = results
        .ranking
        .iter()
        .take(expected_top.len())
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(actual_top, expected_top);
    let expected_top_20 = vec![
        (
            "fsm_799",
            1912,
            1544,
            164,
            204,
            -0.9411087866108787,
            -1799.4,
        ),
        (
            "fsm_823",
            1912,
            1518,
            182,
            212,
            -0.9492677824267782,
            -1815.0,
        ),
        ("fsm_807", 1912, 1544, 234, 134, -0.951673640167364, -1819.6),
        (
            "fsm_847",
            1912,
            1466,
            236,
            210,
            -0.9549163179916318,
            -1825.8,
        ),
        (
            "fsm_2743",
            1912,
            1534,
            182,
            196,
            -0.9657949790794979,
            -1846.6,
        ),
        ("fsm_1294", 1912, 1610, 96, 206, -0.97081589958159, -1856.2),
        (
            "fsm_831",
            1912,
            1476,
            300,
            136,
            -0.9711297071129707,
            -1856.8,
        ),
        (
            "fsm_3495",
            1912,
            1514,
            270,
            128,
            -0.9719665271966527,
            -1858.4,
        ),
        (
            "fsm_1015",
            1912,
            1550,
            176,
            186,
            -0.9786610878661088,
            -1871.2,
        ),
        (
            "fsm_855",
            1912,
            1394,
            362,
            156,
            -0.9847280334728033,
            -1882.8,
        ),
        (
            "fsm_2751",
            1912,
            1474,
            264,
            174,
            -0.9918410041841004,
            -1896.4,
        ),
        (
            "fsm_2767",
            1912,
            1476,
            216,
            220,
            -0.9927824267782427,
            -1898.2,
        ),
        (
            "fsm_2959",
            1912,
            1476,
            212,
            224,
            -0.9933054393305439,
            -1899.2,
        ),
        ("fsm_0", 1912, 1566, 0, 346, -0.996234309623431, -1904.8),
        (
            "fsm_3279",
            1912,
            1404,
            240,
            268,
            -1.0017782426778243,
            -1915.4,
        ),
        (
            "fsm_1023",
            1912,
            1362,
            268,
            282,
            -1.0024058577405858,
            -1916.6,
        ),
        (
            "fsm_1246",
            1912,
            1482,
            154,
            276,
            -1.006485355648536,
            -1924.4,
        ),
        (
            "fsm_3351",
            1912,
            1556,
            154,
            202,
            -1.0067991631799163,
            -1925.0,
        ),
        (
            "fsm_3543",
            1912,
            1522,
            106,
            284,
            -1.0069037656903767,
            -1925.2,
        ),
        (
            "fsm_1039",
            1912,
            1476,
            206,
            230,
            -1.0106694560669456,
            -1932.4,
        ),
    ];
    for (entry, expected) in results.ranking.iter().take(20).zip(expected_top_20.iter()) {
        assert_eq!(entry.id, expected.0);
        assert_eq!(entry.matches, expected.1);
        assert_eq!(entry.wins, expected.2);
        assert_eq!(entry.losses, expected.3);
        assert_eq!(entry.draws, expected.4);
        assert!((entry.average_payoff - expected.5).abs() < 1e-9);
        assert!(
            (entry.total_payoff_for_scoreboard(ScoreAggregation::Mean, false) - expected.6).abs()
                < 1e-9
        );
    }

    let fsm_799 = ranked_strategy(&results, "fsm_799");
    assert_eq!(fsm_799.matches, 1912);
    assert_eq!(fsm_799.wins, 1544);
    assert_eq!(fsm_799.losses, 164);
    assert_eq!(fsm_799.draws, 204);
    assert!((fsm_799.average_payoff - -0.9411087866108787).abs() < 1e-9);
    assert!(
        (fsm_799.total_payoff_for_scoreboard(ScoreAggregation::Mean, false) - -1799.4).abs() < 1e-9
    );

    let fsm_823 = ranked_strategy(&results, "fsm_823");
    assert_eq!(fsm_823.matches, 1912);
    assert_eq!(fsm_823.wins, 1518);
    assert_eq!(fsm_823.losses, 182);
    assert_eq!(fsm_823.draws, 212);
    assert!((fsm_823.average_payoff - -0.9492677824267782).abs() < 1e-9);
    assert!(
        (fsm_823.total_payoff_for_scoreboard(ScoreAggregation::Mean, false) - -1815.0).abs() < 1e-9
    );

    let fsm_0 = ranked_strategy(&results, "fsm_0");
    assert_eq!(fsm_0.matches, 1912);
    assert_eq!(fsm_0.wins, 1566);
    assert_eq!(fsm_0.losses, 0);
    assert_eq!(fsm_0.draws, 346);
    assert!((fsm_0.average_payoff - -0.996234309623431).abs() < 1e-9);
    assert!(
        (fsm_0.total_payoff_for_scoreboard(ScoreAggregation::Mean, false) - -1904.8).abs() < 1e-9
    );
}

#[test]
fn config_legacy_exact_alias_still_parses() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
fsm_grouping = "exact"

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.fsm_grouping, FsmGroupingMode::Moorem);
}
