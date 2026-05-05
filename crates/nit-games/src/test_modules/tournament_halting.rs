//! TM halting filter selection tests, including strict-mode + diagnostics.

use super::shared::{
    halting_tm_tournament_toml, ranked_strategy, run_tournament_from_toml,
    tm_family_1x2_reference_toml,
};
use crate::config::{AcceleratorMode, EngineMode, GamesConfig};
use crate::{
    accelerator_preflight, select_halting_turing_machine_strategies,
    try_select_halting_turing_machine_strategies,
    try_select_halting_turing_machine_strategies_with_diagnostics, TmHaltingFilterBackend,
};

#[test]
fn select_halting_turing_machine_strategies_drops_non_halting_tms() {
    let cfg = GamesConfig::from_toml(&halting_tm_tournament_toml(true)).expect("parse config");
    let filtered = select_halting_turing_machine_strategies(cfg);
    let ids = filtered
        .strategies
        .iter()
        .map(|spec| spec.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["tm_c", "tm_d"]);
}

#[test]
fn tm_tournament_matches_halting_only_roster() {
    let with_bad = run_tournament_from_toml(&halting_tm_tournament_toml(true));
    let expected = run_tournament_from_toml(&halting_tm_tournament_toml(false));

    let actual_ids = with_bad
        .ranking
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    let expected_ids = expected
        .ranking
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(actual_ids, expected_ids);
    assert_eq!(actual_ids, vec!["tm_d", "tm_c"]);

    for expected_entry in &expected.ranking {
        let actual = ranked_strategy(&with_bad, &expected_entry.id);
        assert_eq!(actual.total_payoff, expected_entry.total_payoff);
        assert_eq!(actual.matches, expected_entry.matches);
        assert_eq!(actual.wins, expected_entry.wins);
        assert_eq!(actual.losses, expected_entry.losses);
        assert_eq!(actual.draws, expected_entry.draws);
        assert!((actual.average_payoff - expected_entry.average_payoff).abs() < 1e-9);
    }

    assert_eq!(with_bad.pairwise.len(), expected.pairwise.len());
    let actual_pair = with_bad.pairwise.first().expect("pairwise result");
    let expected_pair = expected.pairwise.first().expect("pairwise result");
    assert_eq!(actual_pair.a, expected_pair.a);
    assert_eq!(actual_pair.b, expected_pair.b);
    assert_eq!(actual_pair.a_total, expected_pair.a_total);
    assert_eq!(actual_pair.b_total, expected_pair.b_total);
    assert_eq!(actual_pair.a_adjusted_total, expected_pair.a_adjusted_total);
    assert_eq!(actual_pair.b_adjusted_total, expected_pair.b_adjusted_total);
    assert_eq!(actual_pair.a_wins, expected_pair.a_wins);
    assert_eq!(actual_pair.b_wins, expected_pair.b_wins);
    assert_eq!(actual_pair.draws, expected_pair.draws);
}

#[test]
fn select_halting_turing_machine_strategies_matches_results_06_good_tms() {
    let cfg = GamesConfig::from_toml(&tm_family_1x2_reference_toml(200)).expect("parse config");
    let filtered = select_halting_turing_machine_strategies(cfg);
    let ids = filtered
        .strategies
        .iter()
        .map(|spec| spec.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["tm_1", "tm_3", "tm_5", "tm_7", "tm_13", "tm_15"]);
}

#[cfg(target_os = "macos")]
#[test]
fn metal_halting_turing_machine_selection_matches_cpu_results_06_good_tms() {
    let cfg_cpu = GamesConfig::from_toml(&tm_family_1x2_reference_toml(200)).expect("parse config");
    let expected = select_halting_turing_machine_strategies(cfg_cpu);
    let expected_ids = expected
        .strategies
        .iter()
        .map(|spec| spec.id.as_str())
        .collect::<Vec<_>>();

    let mut cfg_metal =
        GamesConfig::from_toml(&tm_family_1x2_reference_toml(200)).expect("parse config");
    cfg_metal.engine.mode = EngineMode::Batch;
    cfg_metal.engine.fast_eval = true;
    cfg_metal.engine.accelerator = AcceleratorMode::Metal;
    if accelerator_preflight(&cfg_metal).is_err() {
        return;
    }
    let actual = select_halting_turing_machine_strategies(cfg_metal);
    let actual_ids = actual
        .strategies
        .iter()
        .map(|spec| spec.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(actual_ids, expected_ids);
}

#[test]
fn strict_tm_selection_with_diagnostics_preserves_results_06_survivors() {
    let cfg = GamesConfig::from_toml(&tm_family_1x2_reference_toml(200)).expect("parse config");
    let (filtered, diagnostics) =
        try_select_halting_turing_machine_strategies_with_diagnostics(cfg)
            .expect("strict TM selection should succeed");
    println!(
        "tm_filter strict diagnostics: backend={} kept={}/{} scanned={}/{} probe={:?} filter={:?}",
        diagnostics.backend.label(),
        diagnostics.strategy_count_after,
        diagnostics.strategy_count_before,
        diagnostics.scanned_matchups,
        diagnostics.schedule_matches,
        diagnostics.backend_probe_elapsed,
        diagnostics.halting_filter_elapsed
    );
    let ids = filtered
        .strategies
        .iter()
        .map(|spec| spec.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["tm_1", "tm_3", "tm_5", "tm_7", "tm_13", "tm_15"]);
    assert!(matches!(
        diagnostics.backend,
        TmHaltingFilterBackend::Metal
            | TmHaltingFilterBackend::NotebookCpu
            | TmHaltingFilterBackend::NotebookCpuFallback
    ));
    assert_eq!(diagnostics.strategy_count_before, 16);
    assert_eq!(diagnostics.strategy_count_after, 6);
}

#[test]
fn notebook_tm_filter_reports_cache_activity_for_reference_family() {
    let mut cfg = GamesConfig::from_toml(&tm_family_1x2_reference_toml(200)).expect("parse config");
    cfg.engine.accelerator = AcceleratorMode::Cpu;
    let (_filtered, diagnostics) =
        try_select_halting_turing_machine_strategies_with_diagnostics(cfg)
            .expect("strict TM selection should succeed");
    println!(
        "tm_filter diagnostics: backend={} kept={}/{} scanned={}/{} probe={:?} filter={:?} evals={} hits={} misses={} steps={}",
        diagnostics.backend.label(),
        diagnostics.strategy_count_after,
        diagnostics.strategy_count_before,
        diagnostics.scanned_matchups,
        diagnostics.schedule_matches,
        diagnostics.backend_probe_elapsed,
        diagnostics.halting_filter_elapsed,
        diagnostics.tm_evaluations,
        diagnostics.tm_cache_hits,
        diagnostics.tm_cache_misses,
        diagnostics.tm_steps
    );

    assert!(matches!(
        diagnostics.backend,
        TmHaltingFilterBackend::NotebookCpu
    ));
    assert!(diagnostics.tm_evaluations > 0);
    assert!(diagnostics.tm_cache_hits > 0);
    assert!(diagnostics.tm_steps > 0);
}

#[test]
fn strict_tm_selection_rejects_cpu_fallback_when_metal_is_required() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 3
repetitions = 1
self_play = true

[engine]
accelerator = "metal"
fast_eval = false

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 4
rule_code = 1
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let err = try_select_halting_turing_machine_strategies(cfg)
        .expect_err("strict metal selection should fail before CPU fallback");
    assert!(
        err.contains("fast_eval")
            || err.contains("TM family preparation")
            || err.contains("Metal accelerator"),
        "unexpected strict metal error: {err}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn metal_tm_family_selection_matches_cpu_reference() {
    let mut metal_cfg =
        GamesConfig::from_toml(&tm_family_1x2_reference_toml(200)).expect("parse config");
    metal_cfg.engine.mode = EngineMode::Batch;
    metal_cfg.engine.fast_eval = true;
    metal_cfg.engine.accelerator = AcceleratorMode::Metal;

    let filtered = match try_select_halting_turing_machine_strategies(metal_cfg) {
        Ok(filtered) => filtered,
        Err(err)
            if err.contains("Metal accelerator")
                || err.contains("Metal backend")
                || err.contains("active Metal backend")
                || err.contains("Metal device unavailable") =>
        {
            return;
        }
        Err(err) => panic!("strict metal TM selection failed: {err}"),
    };

    let ids = filtered
        .strategies
        .iter()
        .map(|spec| spec.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["tm_1", "tm_3", "tm_5", "tm_7", "tm_13", "tm_15"]);
}
