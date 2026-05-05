//! Reference scoreboards: TM family and 2x2 / 3x2 FSM tournaments. The
//! arrays here are regression baselines — every numeric literal is checked
//! against an external reference and must not drift.

use super::shared::{ranked_strategy, run_tournament_from_toml, tm_family_1x2_reference_toml};
use crate::config::ScoreAggregation;
use crate::unique_fsm_behavior_representatives;

#[cfg(target_os = "macos")]
use super::shared::{metal_totals_or_skip, simulate_match_from_specs};

#[test]
fn tm_tournament_matches_results_06_pd_reference_scoreboard() {
    let results = run_tournament_from_toml(&tm_family_1x2_reference_toml(200));
    let expected = vec![
        ("tm_3", 12, 8, 0, 4, -0.9175, -11.01),
        ("tm_15", 12, 8, 0, 4, -0.9175, -11.01),
        ("tm_7", 12, 6, 4, 2, -1.37125, -16.455),
        ("tm_13", 12, 0, 6, 6, -1.70625, -20.475),
        ("tm_1", 12, 0, 6, 6, -1.9125, -22.95),
        ("tm_5", 12, 0, 6, 6, -1.9125, -22.95),
    ];

    let actual_ids = results
        .ranking
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        actual_ids,
        expected.iter().map(|(id, ..)| *id).collect::<Vec<_>>()
    );

    for (id, matches, wins, losses, draws, average_payoff, scoreboard_total) in expected {
        let entry = ranked_strategy(&results, id);
        assert_eq!(entry.matches, matches);
        assert_eq!(entry.wins, wins);
        assert_eq!(entry.losses, losses);
        assert_eq!(entry.draws, draws);
        assert!((entry.average_payoff - average_payoff).abs() < 1e-9);
        assert!(
            (entry.total_payoff_for_scoreboard(ScoreAggregation::Mean, false) - scoreboard_total)
                .abs()
                < 1e-9
        );
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
