use crate::config::GamesConfig;
use crate::game::Action;
use crate::history::History;
use crate::strategy::{FsmStrategy, Strategy};
use crate::tournament::TournamentRunner;

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
