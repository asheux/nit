//! Reference scoreboards: TM family and 2x2 / 3x2 FSM tournaments.
//!
//! The expected values in `tournament_reference_data` are externally-validated
//! regression baselines — they must not drift without independent re-derivation.

#[path = "tournament_reference_data.rs"]
mod data;

use super::shared::{ranked_strategy, run_tournament_from_toml, tm_family_1x2_reference_toml};
use crate::config::ScoreAggregation;
use crate::output::StrategyResult;
use crate::unique_fsm_behavior_representatives;
use data::{RankRow, FSM_3X2_TOP_15_IDS, FSM_3X2_TOP_20, TM_FAMILY_PD_TOP6};

#[cfg(target_os = "macos")]
use super::shared::{metal_totals_or_skip, simulate_match_from_specs};

const SCORE_TOL: f64 = 1e-9;

fn assert_rank_row(entry: &StrategyResult, expected: &RankRow) {
    let (id, matches, wins, losses, draws, average_payoff, scoreboard_total) = *expected;
    assert_eq!(entry.id, id);
    assert_eq!(entry.matches, matches);
    assert_eq!(entry.wins, wins);
    assert_eq!(entry.losses, losses);
    assert_eq!(entry.draws, draws);
    assert!((entry.average_payoff - average_payoff).abs() < SCORE_TOL);
    assert!(
        (entry.total_payoff_for_scoreboard(ScoreAggregation::Mean, false) - scoreboard_total).abs()
            < SCORE_TOL
    );
}

fn build_self_play_fsm_toml(rounds: u32, num_states: usize, k: usize) -> String {
    let reps =
        unique_fsm_behavior_representatives(num_states, k).expect("fsm behavior representatives");
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
    for idx in reps {
        src.push_str(&format!(
            r#"[[strategy]]
id = "fsm_{idx}"
type = "fsm"
index = {idx}
num_states = {num_states}
k = {k}

"#
        ));
    }
    src
}

#[test]
fn tm_tournament_matches_results_06_pd_reference_scoreboard() {
    let results = run_tournament_from_toml(&tm_family_1x2_reference_toml(200));

    let actual_ids = results
        .ranking
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    let expected_ids = TM_FAMILY_PD_TOP6
        .iter()
        .map(|row| row.0)
        .collect::<Vec<_>>();
    assert_eq!(actual_ids, expected_ids);

    for expected in TM_FAMILY_PD_TOP6 {
        let entry = ranked_strategy(&results, expected.0);
        assert_rank_row(entry, expected);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn metal_tm_tournament_matches_cpu_results_06_baseline() {
    use crate::config::{AcceleratorMode, EngineMode, GamesConfig};
    use crate::select_halting_turing_machine_strategies;

    let mut metal_cfg =
        GamesConfig::from_toml(&tm_family_1x2_reference_toml(200)).expect("parse config");
    metal_cfg.engine.mode = EngineMode::Batch;
    metal_cfg.engine.fast_eval = true;
    metal_cfg.engine.accelerator = AcceleratorMode::Metal;
    metal_cfg = select_halting_turing_machine_strategies(metal_cfg);
    let pairs = (0..metal_cfg.strategies.len())
        .flat_map(|a| (0..metal_cfg.strategies.len()).map(move |b| (a, b)))
        .collect::<Vec<_>>();

    let Some(totals) = metal_totals_or_skip(&metal_cfg, &pairs) else {
        return;
    };

    let expected = pairs
        .iter()
        .map(|(a, b)| {
            simulate_match_from_specs(
                &metal_cfg.strategies[*a],
                &metal_cfg.strategies[*b],
                metal_cfg.payoff,
                metal_cfg.rounds,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(totals, expected);
}

#[test]
fn tournament_matches_code_02_fsm_2x2_reference_scoreboard() {
    let results = run_tournament_from_toml(&build_self_play_fsm_toml(2000, 2, 2));
    assert_eq!(results.ranking.len(), 22);
    assert_eq!(results.ranking[0].id, "fsm_30");
    assert_eq!(results.ranking[1].id, "fsm_0");
    assert_eq!(results.ranking[2].id, "fsm_19");

    let cases: &[RankRow] = &[
        ("fsm_30", 44, 26, 4, 14, -0.8644772727272727, -38.037),
        ("fsm_0", 44, 36, 0, 8, -1.0, -44.0),
        ("fsm_23", 44, 24, 10, 10, -1.2046363636363637, -53.004),
        ("fsm_47", 44, 4, 26, 14, -2.090068181818182, -91.963),
    ];
    for expected in cases {
        let entry = ranked_strategy(&results, expected.0);
        assert_rank_row(entry, expected);
    }
}

#[test]
fn tournament_matches_saved_code_02_fsm_3x2_reference_scoreboard() {
    let results = run_tournament_from_toml(&build_self_play_fsm_toml(10, 3, 2));
    assert_eq!(results.ranking.len(), 956);

    let actual_top = results
        .ranking
        .iter()
        .take(FSM_3X2_TOP_15_IDS.len())
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(actual_top, FSM_3X2_TOP_15_IDS);

    for (entry, expected) in results
        .ranking
        .iter()
        .take(FSM_3X2_TOP_20.len())
        .zip(FSM_3X2_TOP_20)
    {
        assert_rank_row(entry, expected);
    }

    for id in ["fsm_799", "fsm_823", "fsm_0"] {
        let entry = ranked_strategy(&results, id);
        let expected = FSM_3X2_TOP_20
            .iter()
            .find(|row| row.0 == id)
            .expect("present in top-20");
        assert_rank_row(entry, expected);
    }
}
