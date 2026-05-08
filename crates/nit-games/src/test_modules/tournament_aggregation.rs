//! Aggregation, leaderboard, fast-forward, and per-strategy scoring tests.

use super::shared::{ranked_strategy, run_tournament_from_toml};
use crate::config::{EngineMode, GamesConfig, ScoreAggregation};
use crate::output::StrategyResult;
use crate::tournament::TournamentRunner;
use crate::{KernelRunMode, TournamentKernel};

const SCORE_TOL: f64 = 1e-9;

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < SCORE_TOL,
        "expected {expected}, got {actual}"
    );
}

fn assert_all_c_mean_aggregation(entry: &StrategyResult) {
    assert_eq!(entry.matches, 2);
    assert_eq!(entry.total_payoff, -4);
    assert_close(entry.average_payoff, -1.0);
    assert_close(
        entry.total_payoff_for_scoreboard(ScoreAggregation::Mean, false),
        -2.0,
    );
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
    for id in ["all_c_a", "all_c_b"] {
        assert_all_c_mean_aggregation(ranked_strategy(&results, id));
    }
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
    assert_all_c_mean_aggregation(entry);
    assert_eq!(entry.draws, 2);
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
        assert_close(runner_row.average_payoff, kernel_row.average_payoff);
        assert_eq!(runner_row.matches, kernel_row.matches);
        assert_eq!(runner_row.wins, kernel_row.wins);
        assert_eq!(runner_row.losses, kernel_row.losses);
        assert_eq!(runner_row.draws, kernel_row.draws);
    }
}

#[test]
fn strategy_result_score_respects_aggregation_and_adjustment() {
    use ScoreAggregation::{Mean, Total};

    let result = StrategyResult {
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

    let score_cases = [
        (Total, false, 12.0),
        (Mean, false, 4.0),
        (Total, true, 9.75),
        (Mean, true, 3.25),
    ];
    for (aggregation, adjusted, expected) in score_cases {
        assert_eq!(result.score(aggregation, adjusted), expected);
    }

    let scoreboard_cases = [
        (Total, false, 12.0),
        (Mean, false, 12.0),
        (Total, true, 9.75),
        (Mean, true, 9.75),
    ];
    for (aggregation, adjusted, expected) in scoreboard_cases {
        assert_eq!(
            result.total_payoff_for_scoreboard(aggregation, adjusted),
            expected
        );
    }

    assert_eq!(result.formatted_score(Mean, false), "4");
    assert_eq!(result.formatted_score(Mean, true), "3.25");
    assert_eq!(result.formatted_total_payoff(Mean, false), "12");
    assert_eq!(result.formatted_total_payoff(Mean, true), "9.75");
}
