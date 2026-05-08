//! Tournament progress + batch-runner construction.

use crate::config::GamesConfig;
use crate::game::Action;
use crate::tournament::{TournamentProgress, TournamentRunner};

#[derive(Clone, Copy)]
enum EngineMode {
    Sequential,
    Batch { with_accelerator_cpu: bool },
}

fn fixture_toml(rounds: u32, self_play: bool, mode: EngineMode, strategies: &str) -> String {
    let engine_block = match mode {
        EngineMode::Sequential => String::new(),
        EngineMode::Batch {
            with_accelerator_cpu: true,
        } => "[engine]\nmode = \"batch\"\nfast_eval = true\naccelerator = \"cpu\"\n\n".into(),
        EngineMode::Batch {
            with_accelerator_cpu: false,
        } => "[engine]\nmode = \"batch\"\n\n".into(),
    };
    format!(
        "schema_version = 1\ngame = \"ipd\"\nrounds = {rounds}\nrepetitions = 1\nself_play = {self_play}\n\n{engine_block}{strategies}",
    )
}

const SOLO_INDEX_1: &str = r#"[[strategy]]
id = "solo"
type = "auto"
states = 2
k = 2
index = 1
"#;

const THREE_AUTO_FSMS: &str = r#"[[strategy]]
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

const ALL_C_FSM: &str = r#"[[strategy]]
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
    let toml = fixture_toml(5, false, EngineMode::Sequential, SOLO_INDEX_1);
    let progress = progress_after_steps(&toml, 0);
    assert_eq!(progress.match_index, 0);
    assert_eq!(progress.total_matches, 0);
    assert_eq!(progress.round, 0);
    assert_eq!(progress.rounds, 5);
    assert_eq!(progress.total_payoff_a, 0);
    assert_eq!(progress.total_payoff_b, 0);
}

#[test]
fn tournament_progress_advances_to_next_match_after_boundary() {
    let toml = fixture_toml(4, false, EngineMode::Sequential, THREE_AUTO_FSMS);
    let progress = progress_after_steps(&toml, 4);
    assert_eq!(progress.match_index, 2);
    assert_eq!(progress.round, 0);
    assert_eq!(progress.rounds, 4);
}

#[test]
fn fast_forward_progress_keeps_last_round_snapshot() {
    let toml = fixture_toml(4, true, EngineMode::Sequential, ALL_C_FSM);
    let progress = progress_after_steps(&toml, 4);
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
    let toml = fixture_toml(
        4,
        false,
        EngineMode::Batch {
            with_accelerator_cpu: true,
        },
        THREE_AUTO_FSMS,
    );
    let progress = progress_after_steps(&toml, 4);
    assert_eq!(progress.match_index, 1);
    assert_eq!(progress.round, 4);
    assert_eq!(progress.rounds, 4);
    assert_eq!(progress.last_action_a, Some(Action::Defect));
    assert_eq!(progress.last_action_b, Some(Action::Cooperate));
}

#[test]
fn batch_runner_construction_handles_large_strategy_sets() {
    let toml = fixture_toml(
        2,
        true,
        EngineMode::Batch {
            with_accelerator_cpu: false,
        },
        ALL_C_FSM,
    );
    let mut cfg = GamesConfig::from_toml(&toml).expect("parse batch config");
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
