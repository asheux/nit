use crate::config::{FsmGroupingMode, GamesConfig, StrategySpecKind};
use crate::game::{payoffs_with_timeouts, Action, PayoffMatrix};
use crate::history::History;
use crate::strategy::{
    decode_fsm_notebook_index, decode_tm_rule_code_wolfram, history_to_input_u64, CaStrategy,
    FsmStrategy, InputMode, OneSidedTmStrategy, Strategy, TmMove, TmTransition,
};
use crate::{
    canonical_fsm_indices, group_canonical_fsm_indices_by_behavior,
    group_canonical_fsm_indices_by_behavior_with_mode, unique_fsm_behavior_representatives,
    unique_fsm_behavior_representatives_with_mode, TournamentRunner,
};

fn push_round(history: &mut History, a: Action, b: Action) {
    history.push(a, b);
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
    let payoff = PayoffMatrix::default_pd(); // min=0, max=5
    let both_halted = payoffs_with_timeouts(payoff, Action::Cooperate, Action::Defect, true, true);
    assert_eq!(
        both_halted,
        payoff.payoffs(Action::Cooperate, Action::Defect)
    );

    let a_timeout = payoffs_with_timeouts(payoff, Action::Cooperate, Action::Defect, false, true);
    assert_eq!(a_timeout, (0, 5));

    let b_timeout = payoffs_with_timeouts(payoff, Action::Cooperate, Action::Defect, true, false);
    assert_eq!(b_timeout, (5, 0));

    let both_timeout =
        payoffs_with_timeouts(payoff, Action::Cooperate, Action::Defect, false, false);
    assert_eq!(both_timeout, (0, 0));
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
