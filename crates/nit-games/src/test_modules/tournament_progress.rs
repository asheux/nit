//! Tournament progress + batch-runner construction.

use crate::config::GamesConfig;
use crate::game::Action;
use crate::tournament::{TournamentProgress, TournamentRunner};

const SOLO_STRATEGY_TOML: &str = r#"
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

const THREE_STRATEGY_TOML: &str = r#"
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

const ALL_C_SELFPLAY_TOML: &str = r#"
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

const BATCH_THREE_STRATEGY_TOML: &str = r#"
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

const BATCH_ALL_C_TEMPLATE_TOML: &str = r#"
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

fn progress_after_steps(toml_src: &str, rounds: u32) -> TournamentProgress {
    let cfg = GamesConfig::from_toml(toml_src).expect("parse config");
    let mut runner = TournamentRunner::new(cfg).with_match_history_previews(false);
    if rounds > 0 {
        runner.step_rounds(rounds);
    }
    runner.progress().expect("progress should exist")
}

#[test]
fn tournament_progress_reports_zero_match_runs() {
    let progress = progress_after_steps(SOLO_STRATEGY_TOML, 0);
    assert_eq!(progress.match_index, 0);
    assert_eq!(progress.total_matches, 0);
    assert_eq!(progress.round, 0);
    assert_eq!(progress.rounds, 5);
    assert_eq!(progress.total_payoff_a, 0);
    assert_eq!(progress.total_payoff_b, 0);
}

#[test]
fn tournament_progress_advances_to_next_match_after_boundary() {
    let progress = progress_after_steps(THREE_STRATEGY_TOML, 4);
    assert_eq!(progress.match_index, 2);
    assert_eq!(progress.round, 0);
    assert_eq!(progress.rounds, 4);
}

#[test]
fn fast_forward_progress_keeps_last_round_snapshot() {
    let progress = progress_after_steps(ALL_C_SELFPLAY_TOML, 4);
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
    let progress = progress_after_steps(BATCH_THREE_STRATEGY_TOML, 4);
    assert_eq!(progress.match_index, 1);
    assert_eq!(progress.round, 4);
    assert_eq!(progress.rounds, 4);
    assert_eq!(progress.last_action_a, Some(Action::Defect));
    assert_eq!(progress.last_action_b, Some(Action::Cooperate));
}

#[test]
fn batch_runner_construction_handles_large_strategy_sets() {
    let mut cfg = GamesConfig::from_toml(BATCH_ALL_C_TEMPLATE_TOML).expect("parse batch config");
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
