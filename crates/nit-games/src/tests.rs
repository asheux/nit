use crate::config::GamesConfig;
use crate::game::Action;
use crate::history::History;
use crate::strategy::{FsmStrategy, Strategy};
use crate::tournament::{KernelRunMode, Parallelism, TournamentKernel, TournamentRunner};
use crate::{analyze_history, AnalysisConfig};

fn run_to_completion(mut runner: TournamentRunner) -> crate::output::TournamentResults {
    while !runner.is_done() {
        runner.step_rounds(1);
    }
    runner.results()
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
        vec![[0, 1, 0, 0], [1, 1, 1, 1]],
    );
    let mut history = History::new(1);
    let first = fsm.next_action(&history, true);
    assert_eq!(first, Action::Cooperate);
    history.push(first, Action::Defect);
    let second = fsm.next_action(&history, true);
    assert_eq!(second, Action::Defect);
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
