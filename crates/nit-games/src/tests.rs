use crate::config::{GamesConfig, StrategySpec, StrategySpecKind};
use crate::game::Action;
use crate::history::History;
use crate::fsm_enum::enumerate_fsms;
use crate::strategy::{
    FsmStrategy, InputMode, OneSidedTmStrategy, Strategy, TmMove, TmTransition,
};
use crate::tournament::{KernelRunMode, Parallelism, TournamentKernel, TournamentRunner};
use crate::{analyze_history, AnalysisConfig};
use std::collections::HashMap;
use nit_utils::hashing::XorShift64;

fn run_to_completion(mut runner: TournamentRunner) -> crate::output::TournamentResults {
    while !runner.is_done() {
        runner.step_rounds(1);
    }
    runner.results()
}

fn simulate_match(
    a: &mut dyn Strategy,
    b: &mut dyn Strategy,
    rounds: u32,
) -> (i64, i64) {
    let payoff = crate::game::PayoffMatrix::default_pd();
    let mut history = History::new(1);
    let mut a_total = 0i64;
    let mut b_total = 0i64;
    for _ in 0..rounds {
        let a_action = a.next_action(&history, true);
        let b_action = b.next_action(&history, false);
        let (a_payoff, b_payoff) = payoff.payoffs(a_action, b_action);
        a_total += a_payoff as i64;
        b_total += b_payoff as i64;
        history.push(a_action, b_action);
    }
    (a_total, b_total)
}

fn drive_strategy(strategy: &mut dyn Strategy, opponent: &[Action]) -> Vec<Action> {
    let mut history = History::new(1);
    let mut out = Vec::new();
    for &opp_action in opponent {
        let action = strategy.next_action(&history, true);
        out.push(action);
        history.push(action, opp_action);
    }
    out
}

#[test]
fn tft_vs_alld_scores_match_expectation() {
    let cfg = r#"
schema_version = 1
game = "ipd"
rounds = 5
repetitions = 1
self_play = false
seed = 1

[[strategy]]
id = "tft"
type = "builtin"
name = "Tit For Tat"

[[strategy]]
id = "alld"
type = "builtin"
name = "Always Defect"
"#;
    let config = GamesConfig::from_toml(cfg).expect("config parse");
    let results = run_to_completion(TournamentRunner::new(config));
    let pair = results
        .pairwise
        .iter()
        .find(|p| p.a == "tft" && p.b == "alld")
        .expect("pairwise");
    assert_eq!(pair.a_total, 4);
    assert_eq!(pair.b_total, 9);
}

#[test]
fn fsm_transition_updates_state() {
    let mut fsm = FsmStrategy::new(
        "fsm",
        0,
        vec![Action::Cooperate, Action::Defect],
        InputMode::JointLastAction,
        vec![vec![0, 1, 0, 0], vec![1, 1, 1, 1]],
    );
    let mut history = History::new(1);
    let first = fsm.next_action(&history, true);
    assert_eq!(first, Action::Cooperate);
    history.push(first, Action::Defect);
    let second = fsm.next_action(&history, true);
    assert_eq!(second, Action::Defect);
}

#[test]
fn fsm_enumeration_counts_small_cases() {
    let count_one = enumerate_fsms(1, InputMode::OpponentLastAction, None, false, |_| {});
    assert_eq!(count_one, 2);

    let count_two = enumerate_fsms(2, InputMode::OpponentLastAction, None, false, |_| {});
    assert_eq!(count_two, 64);

    let canonical_two = enumerate_fsms(2, InputMode::OpponentLastAction, None, true, |_| {});
    assert!(canonical_two > 0);
    assert!(canonical_two <= count_two);
}

#[test]
fn memory_indexing_uses_bit_packed_window() {
    let mut history = History::new(2);
    history.push(Action::Cooperate, Action::Cooperate);
    history.push(Action::Cooperate, Action::Defect);
    let idx_a = history.memory_index(true, 2).expect("index a");
    let idx_b = history.memory_index(false, 2).expect("index b");
    assert_eq!(idx_a, 1);
    assert_eq!(idx_b, 2);
}

#[test]
fn deterministic_rng_reproducibility() {
    let cfg = r#"
schema_version = 1
game = "ipd"
rounds = 12
repetitions = 1
self_play = false
seed = 42

[[strategy]]
id = "rand"
type = "random"
p_cooperate = 0.5

[[strategy]]
id = "allc"
type = "builtin"
name = "Always Cooperate"
"#;
    let config = GamesConfig::from_toml(cfg).expect("config parse");
    let results_a = run_to_completion(TournamentRunner::new(config.clone()));
    let results_b = run_to_completion(TournamentRunner::new(config));
    assert_eq!(
        results_a.ranking[0].total_payoff,
        results_b.ranking[0].total_payoff
    );
    assert_eq!(
        results_a.ranking[1].total_payoff,
        results_b.ranking[1].total_payoff
    );
}

#[test]
fn fsm_input_index_base_one_subtracts_state_indices() {
    let cfg = r#"
schema_version = 1
game = "ipd"
rounds = 3
repetitions = 1
self_play = false

[[strategy]]
id = "fsm1"
type = "fsm"
input_index_base = 1
start_state = 1
output = ["C", "D"]
transitions = [
  [1, 2, 1, 2],
  [2, 2, 1, 1],
]

[[strategy]]
id = "allc"
type = "builtin"
name = "Always Cooperate"
"#;
    let config = GamesConfig::from_toml(cfg).expect("config parse");
    let fsm = config
        .strategies
        .iter()
        .find(|s| s.id == "fsm1")
        .expect("fsm spec");
    match &fsm.kind {
        crate::config::StrategySpecKind::Fsm {
            start_state,
            transitions,
            ..
        } => {
            assert_eq!(*start_state, 0);
            assert_eq!(transitions[0][0], 0);
            assert_eq!(transitions[0][1], 1);
            assert_eq!(transitions[1][0], 1);
            assert_eq!(transitions[1][2], 0);
        }
        _ => panic!("expected fsm"),
    }
}

#[test]
fn fast_eval_matches_simulation_for_deterministic_strategies() {
    let cfg = r#"
schema_version = 1
game = "ipd"
rounds = 250
repetitions = 2
self_play = false
seed = 12345
noise = 0.0

[history]
enabled = false

[engine]
mode = "batch"
parallelism = "off"
fast_eval = true

[[strategy]]
id = "allc"
type = "builtin"

[[strategy]]
id = "tft"
type = "builtin"

[[strategy]]
id = "grim"
type = "builtin"

[[strategy]]
id = "wsls"
type = "builtin"

[[strategy]]
id = "mem1"
type = "memory"
n = 1
initial = "C"
table = ["C", "D", "D", "C"]

[[strategy]]
id = "fsm"
type = "fsm"
start_state = 0
output = ["C", "D"]
transitions = [[0, 1, 0, 1], [1, 1, 1, 1]]
"#;
    let mut fast_config = GamesConfig::from_toml(cfg).expect("config parse");
    fast_config.engine.fast_eval = true;
    let fast_results = TournamentKernel::new(fast_config.clone()).run(KernelRunMode::Sequential {
        event_writer: None,
        history_writer: None,
    });

    fast_config.engine.fast_eval = false;
    let slow_results = TournamentKernel::new(fast_config).run(KernelRunMode::Sequential {
        event_writer: None,
        history_writer: None,
    });

    let fast_rank: HashMap<_, _> = fast_results
        .ranking
        .iter()
        .map(|r| (r.id.clone(), r.clone()))
        .collect();
    for slow in &slow_results.ranking {
        let fast = fast_rank.get(&slow.id).expect("fast rank");
        assert_eq!(fast.total_payoff, slow.total_payoff);
        assert_eq!(fast.wins, slow.wins);
        assert_eq!(fast.losses, slow.losses);
        assert_eq!(fast.draws, slow.draws);
    }

    let fast_pairs: HashMap<_, _> = fast_results
        .pairwise
        .iter()
        .map(|p| ((p.a.clone(), p.b.clone()), p.clone()))
        .collect();
    for slow in &slow_results.pairwise {
        let fast = fast_pairs
            .get(&(slow.a.clone(), slow.b.clone()))
            .expect("fast pair");
        assert_eq!(fast.a_total, slow.a_total);
        assert_eq!(fast.b_total, slow.b_total);
        assert_eq!(fast.a_wins, slow.a_wins);
        assert_eq!(fast.b_wins, slow.b_wins);
        assert_eq!(fast.draws, slow.draws);
    }
}

#[test]
fn fast_eval_matches_simulation_for_random_fsms() {
    let mut rng = XorShift64::new(0x5a17_33aa_9c1f);
    let rounds = 200u32;
    for idx in 0..50 {
        let states_a = (rng.next_u64() % 3 + 1) as usize;
        let states_b = (rng.next_u64() % 3 + 1) as usize;
        let mode_a = match rng.next_u64() % 3 {
            0 => InputMode::OpponentLastAction,
            1 => InputMode::SelfLastAction,
            _ => InputMode::JointLastAction,
        };
        let mode_b = match rng.next_u64() % 3 {
            0 => InputMode::OpponentLastAction,
            1 => InputMode::SelfLastAction,
            _ => InputMode::JointLastAction,
        };
        let outputs_a = (0..states_a)
            .map(|_| if rng.next_u64() & 1 == 0 { Action::Cooperate } else { Action::Defect })
            .collect::<Vec<_>>();
        let outputs_b = (0..states_b)
            .map(|_| if rng.next_u64() & 1 == 0 { Action::Cooperate } else { Action::Defect })
            .collect::<Vec<_>>();
        let trans_a = (0..states_a)
            .map(|_| {
                (0..mode_a.alphabet_size())
                    .map(|_| (rng.next_u64() as usize) % states_a)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let trans_b = (0..states_b)
            .map(|_| {
                (0..mode_b.alphabet_size())
                    .map(|_| (rng.next_u64() as usize) % states_b)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        let spec_a = StrategySpec {
            id: format!("fsm_a_{idx}"),
            name: None,
            kind: StrategySpecKind::Fsm {
                num_states: states_a,
                start_state: 0,
                outputs: outputs_a.clone(),
                input_mode: Some(mode_a),
                transitions: trans_a.clone(),
            },
        };
        let spec_b = StrategySpec {
            id: format!("fsm_b_{idx}"),
            name: None,
            kind: StrategySpecKind::Fsm {
                num_states: states_b,
                start_state: 0,
                outputs: outputs_b.clone(),
                input_mode: Some(mode_b),
                transitions: trans_b.clone(),
            },
        };

        let mut sim_a = FsmStrategy::new(
            spec_a.id.clone(),
            0,
            outputs_a,
            mode_a,
            trans_a,
        );
        let mut sim_b = FsmStrategy::new(
            spec_b.id.clone(),
            0,
            outputs_b,
            mode_b,
            trans_b,
        );
        sim_a.reset();
        sim_b.reset();
        let (sim_a_total, sim_b_total) = simulate_match(&mut sim_a, &mut sim_b, rounds);

        let fast_a = crate::FastStrategyModel::from_spec(&spec_a).expect("fast model a");
        let fast_b = crate::FastStrategyModel::from_spec(&spec_b).expect("fast model b");
        let fast = crate::fast_eval::evaluate_match(
            &fast_a,
            &fast_b,
            rounds,
            crate::game::PayoffMatrix::default_pd(),
            false,
        );

        assert_eq!(sim_a_total, fast.a_total);
        assert_eq!(sim_b_total, fast.b_total);
    }
}

#[test]
fn fsm_encodings_match_builtin_behaviors() {
    let opponent = [
        Action::Cooperate,
        Action::Defect,
        Action::Cooperate,
        Action::Defect,
        Action::Defect,
        Action::Cooperate,
    ];

    let mut builtin_allc = crate::strategy::AlwaysCooperate::new("allc");
    let mut fsm_allc = FsmStrategy::new(
        "fsm_allc",
        0,
        vec![Action::Cooperate],
        InputMode::OpponentLastAction,
        vec![vec![0, 0]],
    );
    assert_eq!(
        drive_strategy(&mut builtin_allc, &opponent),
        drive_strategy(&mut fsm_allc, &opponent)
    );

    let mut builtin_alld = crate::strategy::AlwaysDefect::new("alld");
    let mut fsm_alld = FsmStrategy::new(
        "fsm_alld",
        0,
        vec![Action::Defect],
        InputMode::OpponentLastAction,
        vec![vec![0, 0]],
    );
    assert_eq!(
        drive_strategy(&mut builtin_alld, &opponent),
        drive_strategy(&mut fsm_alld, &opponent)
    );

    let mut builtin_tft = crate::strategy::TitForTat::new("tft");
    let mut fsm_tft = FsmStrategy::new(
        "fsm_tft",
        0,
        vec![Action::Cooperate, Action::Defect],
        InputMode::OpponentLastAction,
        vec![vec![0, 1], vec![0, 1]],
    );
    assert_eq!(
        drive_strategy(&mut builtin_tft, &opponent),
        drive_strategy(&mut fsm_tft, &opponent)
    );

    let mut builtin_wsls = crate::strategy::WinStayLoseShift::new("wsls");
    let mut fsm_wsls = FsmStrategy::new(
        "fsm_wsls",
        0,
        vec![Action::Cooperate, Action::Defect],
        InputMode::JointLastAction,
        vec![vec![0, 1, 1, 0], vec![1, 0, 0, 1]],
    );
    assert_eq!(
        drive_strategy(&mut builtin_wsls, &opponent),
        drive_strategy(&mut fsm_wsls, &opponent)
    );
}

#[test]
fn tm_rail_output_triggers() {
    let transitions = vec![
        TmTransition {
            write: 1,
            move_dir: TmMove::Right,
            next: 1,
        },
        TmTransition {
            write: 1,
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
        8,
        InputMode::OpponentLastAction,
        vec![Action::Cooperate, Action::Defect],
        transitions,
    );
    tm.reset();
    let action = tm.next_action(&History::new(1), true);
    assert_eq!(action, Action::Defect);
}

#[test]
fn tm_rail_output_triggers_when_next_zero() {
    let transitions = vec![
        TmTransition {
            write: 1,
            move_dir: TmMove::Right,
            next: 0,
        },
        TmTransition {
            write: 1,
            move_dir: TmMove::Right,
            next: 0,
        },
    ];
    let mut tm = OneSidedTmStrategy::new(
        "tm",
        2,
        1,
        0,
        0,
        8,
        InputMode::OpponentLastAction,
        vec![Action::Cooperate, Action::Defect],
        transitions,
    );
    tm.reset();
    let action = tm.next_action(&History::new(1), true);
    assert_eq!(action, Action::Defect);
}

#[test]
fn tm_next_zero_without_rail_falls_back() {
    let transitions = vec![
        TmTransition {
            write: 1,
            move_dir: TmMove::Left,
            next: 0,
        },
        TmTransition {
            write: 1,
            move_dir: TmMove::Left,
            next: 0,
        },
    ];
    let mut tm = OneSidedTmStrategy::new(
        "tm",
        2,
        1,
        0,
        0,
        8,
        InputMode::OpponentLastAction,
        vec![Action::Cooperate, Action::Defect],
        transitions,
    );
    tm.reset();
    let action = tm.next_action(&History::new(1), true);
    assert_eq!(action, Action::Cooperate);
}

#[test]
fn tm_left_boundary_safe() {
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
    let mut tm = OneSidedTmStrategy::new(
        "tm",
        2,
        1,
        0,
        0,
        1,
        InputMode::OpponentLastAction,
        vec![Action::Cooperate, Action::Defect],
        transitions,
    );
    tm.reset();
    let action = tm.next_action(&History::new(1), true);
    assert_eq!(action, Action::Cooperate);
}

#[test]
fn tm_fallback_symbol_overrides_blank() {
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
    let mut tm = OneSidedTmStrategy::new(
        "tm",
        2,
        1,
        0,
        1,
        0,
        InputMode::OpponentLastAction,
        vec![Action::Cooperate, Action::Defect],
        transitions,
    );
    tm.reset();
    let action = tm.next_action(&History::new(1), true);
    assert_eq!(action, Action::Defect);
}

#[test]
fn tm_table_transitions_parse_wl_style() {
    let cfg = r#"
schema_version = 1
game = "ipd"
rounds = 3
repetitions = 1
self_play = false

[[strategy]]
id = "tm"
type = "one_sided_tm"
states = 2
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 4
input_mode = "opponent_last_action"
output_map = ["C","D"]
transitions = [
  [ [1, 0, "R"], [2, 1, "S"] ],
  [ [0, 1, "L"], [2, 0, "R"] ],
]
"#;
    let config = GamesConfig::from_toml(cfg).expect("config parse");
    let spec = config
        .strategies
        .iter()
        .find(|s| s.id == "tm")
        .expect("tm spec");
    match &spec.kind {
        StrategySpecKind::OneSidedTm { transitions, .. } => {
            assert_eq!(transitions.len(), 4);
            assert_eq!(transitions[0].next, 1);
            assert_eq!(transitions[0].write, 0);
            assert!(matches!(transitions[0].move_dir, TmMove::Right));
            assert_eq!(transitions[1].next, 2);
            assert_eq!(transitions[1].write, 1);
            assert!(matches!(transitions[1].move_dir, TmMove::Stay));
            assert_eq!(transitions[2].next, 0);
            assert_eq!(transitions[2].write, 1);
            assert!(matches!(transitions[2].move_dir, TmMove::Left));
        }
        _ => panic!("expected one_sided_tm spec"),
    }
}

#[test]
fn tm_rule_code_matches_wolfram_example() {
    let cfg = r#"
schema_version = 1
game = "ipd"
rounds = 1
repetitions = 1
self_play = false

[[strategy]]
id = "tm"
type = "one_sided_tm"
states = 2
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 8
input_mode = "opponent_last_action"
output_map = ["C","D"]
rule_code = 3111
"#;
    let config = GamesConfig::from_toml(cfg).expect("config parse");
    let spec = config
        .strategies
        .iter()
        .find(|s| s.id == "tm")
        .expect("tm spec");
    match &spec.kind {
        StrategySpecKind::OneSidedTm { transitions, .. } => {
            let t = transitions;
            assert_eq!(t.len(), 4);
            // (1,0) -> (1,0,L)
            assert_eq!(t[0].next, 1);
            assert_eq!(t[0].write, 0);
            assert!(matches!(t[0].move_dir, TmMove::Left));
            // (1,1) -> (2,1,L)
            assert_eq!(t[1].next, 2);
            assert_eq!(t[1].write, 1);
            assert!(matches!(t[1].move_dir, TmMove::Left));
            // (2,0) -> (2,1,R)
            assert_eq!(t[2].next, 2);
            assert_eq!(t[2].write, 1);
            assert!(matches!(t[2].move_dir, TmMove::Right));
            // (2,1) -> (2,0,L)
            assert_eq!(t[3].next, 2);
            assert_eq!(t[3].write, 0);
            assert!(matches!(t[3].move_dir, TmMove::Left));
        }
        _ => panic!("expected one_sided_tm spec"),
    }
}

#[test]
fn tm_deterministic_reproducibility() {
    let transitions = vec![
        TmTransition {
            write: 1,
            move_dir: TmMove::Stay,
            next: 1,
        },
        TmTransition {
            write: 0,
            move_dir: TmMove::Stay,
            next: 1,
        },
    ];
    let mut tm_a = OneSidedTmStrategy::new(
        "tm",
        2,
        1,
        0,
        0,
        4,
        InputMode::OpponentLastAction,
        vec![Action::Cooperate, Action::Defect],
        transitions.clone(),
    );
    let mut tm_b = OneSidedTmStrategy::new(
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
    tm_a.reset();
    tm_b.reset();

    let opponent = [Action::Cooperate, Action::Defect, Action::Cooperate];
    let trace_a = drive_strategy(&mut tm_a, &opponent);
    let trace_b = drive_strategy(&mut tm_b, &opponent);
    assert_eq!(trace_a, trace_b);
}

#[test]
fn run_id_is_stable_for_seed_and_config() {
    let cfg = r#"
schema_version = 1
game = "ipd"
rounds = 5
repetitions = 1
self_play = false

[[strategy]]
id = "allc"
type = "builtin"
name = "Always Cooperate"
"#;
    let run_a = crate::output::run_id_from_seed_config(42, cfg);
    let run_b = crate::output::run_id_from_seed_config(42, cfg);
    let run_c = crate::output::run_id_from_seed_config(43, cfg);
    assert_eq!(run_a, run_b);
    assert_ne!(run_a, run_c);
}

#[test]
fn analysis_summarizes_outcomes_and_tail() {
    let history = r#"{"match_id":0,"match_index":1,"total_matches":1,"a":"alpha","b":"beta","repetition":1,"rounds":4,"score_idx":"0123","a_score":10,"b_score":12,"a_initial":null,"b_initial":null}"#;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let history_path = std::env::temp_dir().join(format!(
        "nit_games_history_analysis_test_{stamp}.ndjson"
    ));
    let out_dir = std::env::temp_dir().join(format!("nit_games_analysis_out_{stamp}"));
    std::fs::write(&history_path, format!("{history}\n")).expect("write history");

    let config = AnalysisConfig {
        tail_rounds: 2,
        trajectory_samples: 2,
        ..AnalysisConfig::default()
    };
    let analysis = analyze_history(&history_path, &out_dir, config).expect("analyze");
    assert_eq!(analysis.summary.total_matches, 1);
    assert_eq!(analysis.summary.total_rounds, 4);

    let alpha = analysis
        .summary
        .strategies
        .iter()
        .find(|s| s.id == "alpha")
        .expect("alpha summary");
    let beta = analysis
        .summary
        .strategies
        .iter()
        .find(|s| s.id == "beta")
        .expect("beta summary");

    assert!((alpha.coop_rate - 0.5).abs() < 1e-6);
    assert!((beta.coop_rate - 0.5).abs() < 1e-6);
    assert_eq!(alpha.tail_rounds, 2);
    assert_eq!(beta.tail_rounds, 2);
    assert_eq!(alpha.tail_coop_rounds, 0);
    assert_eq!(beta.tail_coop_rounds, 1);
}

#[test]
fn asymmetric_matrix_scoring_uses_cell_values() {
    let cfg = r#"
schema_version = 1
game = "ipd"
rounds = 1
repetitions = 1
self_play = false

[payoff]
matrix = [
  [[3,1],[0,5]],
  [[4,2],[1,0]],
]

[[strategy]]
id = "allc"
type = "builtin"
name = "Always Cooperate"

[[strategy]]
id = "alld"
type = "builtin"
name = "Always Defect"
"#;
    let config = GamesConfig::from_toml(cfg).expect("config parse");
    let results = run_to_completion(TournamentRunner::new(config));
    let pair = results
        .pairwise
        .iter()
        .find(|p| p.a == "allc" && p.b == "alld")
        .expect("pairwise");
    assert_eq!(pair.a_total, 0);
    assert_eq!(pair.b_total, 5);
}

fn totals_by_id(results: &crate::output::TournamentResults) -> Vec<(String, i64)> {
    let mut totals: Vec<(String, i64)> = results
        .ranking
        .iter()
        .map(|entry| (entry.id.clone(), entry.total_payoff))
        .collect();
    totals.sort_by(|a, b| a.0.cmp(&b.0));
    totals
}

#[test]
fn kernel_reproducibility_same_seed() {
    let cfg = r#"
schema_version = 1
game = "ipd"
rounds = 25
repetitions = 2
self_play = false
seed = 777
noise = 0.15

[[strategy]]
id = "rand"
type = "random"
p_cooperate = 0.4

[[strategy]]
id = "tft"
type = "builtin"
name = "Tit For Tat"

[[strategy]]
id = "alld"
type = "builtin"
name = "Always Defect"
"#;
    let config = GamesConfig::from_toml(cfg).expect("config parse");
    let kernel_a = TournamentKernel::new(config.clone());
    let kernel_b = TournamentKernel::new(config);
    let results_a = kernel_a.run(KernelRunMode::Sequential {
        event_writer: None,
        history_writer: None,
    });
    let results_b = kernel_b.run(KernelRunMode::Sequential {
        event_writer: None,
        history_writer: None,
    });
    assert_eq!(totals_by_id(&results_a), totals_by_id(&results_b));
}

#[test]
fn kernel_parallel_matches_sequential_results() {
    let cfg = r#"
schema_version = 1
game = "ipd"
rounds = 40
repetitions = 3
self_play = true
seed = 4242
noise = 0.25

[[strategy]]
id = "rand"
type = "random"
p_cooperate = 0.55

[[strategy]]
id = "wsls"
type = "builtin"
name = "Win Stay Lose Shift"

[[strategy]]
id = "grim"
type = "builtin"
name = "Grim Trigger"

[[strategy]]
id = "mem1"
type = "memory"
n = 1
initial = "C"
table = ["C", "D", "D", "C"]
"#;
    let config = GamesConfig::from_toml(cfg).expect("config parse");
    let kernel = TournamentKernel::new(config);
    let sequential = kernel.run(KernelRunMode::Sequential {
        event_writer: None,
        history_writer: None,
    });
    let parallel = kernel.run(KernelRunMode::Parallel {
        parallelism: Parallelism::Threads(4),
        event_sender: None,
        include_rounds: false,
        history_sender: None,
    });
    assert_eq!(totals_by_id(&sequential), totals_by_id(&parallel));
}
