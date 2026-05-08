//! Benchmark suite for the `nit-games` tournament engine.
//!
//! Config builders + `BenchTournament` façade live under `shared/common.rs`;
//! this file holds the Criterion bench fns and the `criterion_main!` harness.
//! Helpers live in a subdirectory so Cargo's bench auto-discovery does not
//! pick them up as a standalone bench target.

#[path = "shared/common.rs"]
mod common;

use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

use nit_games::events::EventWriter;
use nit_games::history_log::HistoryWriter;
use nit_games::output::{RunPaths, RunSummary, RUN_SUMMARY_SCHEMA_VERSION};
use nit_games::tournament::{KernelRunMode, Parallelism, TournamentKernel};
use nit_games::Strategy;

use common::{
    apply_mixed_strategy_overrides, bench_fast_eval_pair, binary_tm_transitions,
    build_alternating_baseline, build_deterministic_strategy_suite, build_fsm_heavy_tournament,
    build_generic_fsm_tournament, build_tm_family_reference, build_tm_uniform_tournament,
    temp_bench_path, BenchTournament, HeadMovement, BASELINE_ROUNDS,
};

// ── tournament sizes ──

fn bench_fsm_field(c: &mut Criterion, label: &'static str, strategies: usize, rounds: u32) {
    let bench =
        BenchTournament::from_config(build_generic_fsm_tournament(strategies, rounds, 1, false));
    c.bench_function(label, |b| {
        b.iter(|| bench.run_sequential());
    });
}

/// Baseline: a single 200-round match between two FSM strategies.
fn bench_single_match(c: &mut Criterion) {
    bench_fsm_field(c, "single_match_200_rounds", 2, BASELINE_ROUNDS);
}

/// 16 strategies, 200 rounds, sequential.
fn bench_tournament_small(c: &mut Criterion) {
    bench_fsm_field(c, "tournament_small", 16, BASELINE_ROUNDS);
}

/// 128 strategies × 50 rounds: O(n²) matchups stress the scheduler.
fn bench_tournament_medium(c: &mut Criterion) {
    bench_fsm_field(c, "tournament_medium", 128, 50);
}

// ── logging overhead ──

/// I/O cost of NDJSON event/history logging vs. a no-writer baseline.
fn bench_logging(c: &mut Criterion) {
    let tournament_config = build_generic_fsm_tournament(8, 100, 1, false);
    let kernel = TournamentKernel::new(tournament_config);

    let mut group = c.benchmark_group("logging");

    group.bench_function(BenchmarkId::new("logging_off", "events"), |b| {
        b.iter(|| {
            kernel.run(KernelRunMode::Sequential {
                event_writer: None,
                history_writer: None,
            })
        });
    });

    group.bench_function(BenchmarkId::new("logging_on", "events_history"), |b| {
        b.iter(|| {
            let event_path = temp_bench_path("events", "ndjson");
            let history_path = temp_bench_path("history", "ndjson");
            let mut event_sink = EventWriter::new(event_path, true).expect("event writer");
            let mut history_sink = HistoryWriter::new(history_path).expect("history writer");

            let _ = kernel.run(KernelRunMode::Sequential {
                event_writer: Some(&mut event_sink),
                history_writer: Some(&mut history_sink),
            });

            let _ = event_sink.finish();
            let _ = history_sink.finish();
        });
    });

    group.finish();
}

// ── parallel execution ──

/// Rayon-parallel tournaments at 64- and 256-strategy field sizes.
/// Sample budgets are trimmed so the 256-strategy variant stays bounded.
fn bench_parallel(c: &mut Criterion) {
    let mut group = c.benchmark_group("parallel");
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    let small_field = build_generic_fsm_tournament(64, 50, 1, false);
    let kernel_small = TournamentKernel::new(small_field);
    group.bench_function("tournament_parallel_auto", |b| {
        b.iter(|| {
            kernel_small.run(KernelRunMode::Parallel {
                parallelism: Parallelism::Auto,
                event_sender: None,
                include_rounds: false,
                history_sender: None,
            })
        });
    });

    let large_field = build_generic_fsm_tournament(256, 50, 1, false);
    let kernel_large = TournamentKernel::new(large_field);
    group.bench_function("tournament_parallel_large", |b| {
        b.iter(|| {
            kernel_large.run(KernelRunMode::Parallel {
                parallelism: Parallelism::Auto,
                event_sender: None,
                include_rounds: false,
                history_sender: None,
            })
        });
    });

    group.finish();
}

// ── fast-eval ──

/// Fast-eval (cycle-detecting) vs. brute-force execution. The
/// `deterministic` subgroup uses pure FSMs that always cycle; `mixed`
/// blends FSM/CA/TM to exercise the heterogeneous fast-eval path.
fn bench_fast_eval(c: &mut Criterion) {
    let mut group = c.benchmark_group("fast_eval");

    let deterministic_config = build_deterministic_strategy_suite(5000);
    bench_fast_eval_pair(
        &mut group,
        deterministic_config,
        "deterministic_fast",
        "deterministic_slow",
    );

    let mut mixed_config = build_generic_fsm_tournament(12, 500, 1, false);
    mixed_config.engine.fast_eval = true;
    apply_mixed_strategy_overrides(&mut mixed_config);

    let bench_mixed = BenchTournament::from_config(mixed_config);

    group.bench_function("mixed_fsm_ca_tm", |b| {
        b.iter(|| bench_mixed.run_sequential());
    });

    group.finish();
}

/// Stress-test fast-eval with 32 FSMs over 3000 rounds. The gap between
/// the fast and slow rows quantifies the cycle-detection benefit.
fn bench_fsm_fast_eval_heavy(c: &mut Criterion) {
    let mut group = c.benchmark_group("fsm_fast_eval");
    let fsm_config = build_fsm_heavy_tournament(32, 3000);
    bench_fast_eval_pair(&mut group, fsm_config, "fsm_fast", "fsm_slow");
    group.finish();
}

// ── Turing machine ──

/// Calls `next_action` 256× on a single TM strategy without tournament
/// scaffolding, isolating tape-simulation cost.
fn bench_tm_micro(c: &mut Criterion) {
    let stay_transitions = binary_tm_transitions(HeadMovement::Stationary);

    let mut tm_strategy = nit_games::OneSidedTmStrategy::new(
        "tm",
        2,   // symbols
        1,   // start_state
        0,   // blank
        256, // max steps per round
        stay_transitions,
    );

    let empty_history = nit_games::History::new(1);
    let invocations = 256u32;

    c.bench_function("tm_micro_steps", |b| {
        b.iter(|| {
            (0..invocations).for_each(|_| {
                black_box(tm_strategy.next_action(&empty_history, true));
            });
        });
    });
}

/// Same 12-strategy / 200-round schedule with TM strategies and FSM
/// baselines, quantifying tape-simulation overhead.
fn bench_tm_tournament(c: &mut Criterion) {
    let mut group = c.benchmark_group("tm_tournament");

    let bench_turing = BenchTournament::from_config(build_tm_uniform_tournament(
        12,
        200,
        HeadMovement::Advancing,
        32,
        "tm",
    ));
    let bench_baseline = BenchTournament::from_config(build_alternating_baseline(12, 200));

    group.bench_function("tm", |b| {
        b.iter(|| bench_turing.run_sequential());
    });

    group.bench_function("baseline", |b| {
        b.iter(|| bench_baseline.run_sequential());
    });

    group.finish();
}

/// Worst-case TM cost: 8 strategies × 512-step budgets × stationary heads.
/// Every step touches the same tape cell, so the full budget is consumed
/// each round.
fn bench_tm_heavy(c: &mut Criterion) {
    let mut group = c.benchmark_group("tm_heavy");

    let bench_heavy = BenchTournament::from_config(build_tm_uniform_tournament(
        8,
        150,
        HeadMovement::Stationary,
        512,
        "tm_heavy",
    ));

    group.bench_function("tm_steps_heavy", |b| {
        b.iter(|| bench_heavy.run_sequential());
    });

    group.finish();
}

// ── halting filter ──

/// Pre-tournament TM halting filter over the full 1-state-2-symbol
/// Wolfram family (16 TMs) with a 1000-step budget per round.
fn bench_tm_family_halting_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("tm_family_halting");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(6));

    let reference_config = build_tm_family_reference(200, 1000);

    group.bench_function("tm_1x2_rounds200_steps1000", |b| {
        b.iter(|| {
            let (_filtered_config, filter_diagnostics) =
                nit_games::try_select_halting_turing_machine_strategies_with_diagnostics(
                    reference_config.clone(),
                )
                .expect("TM family halting selection");
            black_box(filter_diagnostics);
        });
    });

    group.finish();
}

// ── sweep I/O ──

/// JSON serialisation throughput for tournament artefacts. Mirrors the
/// I/O path taken at sweep cell completion (`nit_utils::fs::write_atomic`).
fn bench_sweep_io(c: &mut Criterion) {
    let sweep_config = build_tm_uniform_tournament(6, 60, HeadMovement::Advancing, 32, "tm");
    let kernel = TournamentKernel::new(sweep_config.clone());
    let tournament_results = kernel.run(KernelRunMode::Sequential {
        event_writer: None,
        history_writer: None,
    });

    let summary_dest = temp_bench_path("sweep_summary", "json");
    let results_dest = temp_bench_path("sweep_results", "json");
    let definitions_dest = temp_bench_path("sweep_definitions", "json");

    let run_summary = RunSummary {
        schema_version: RUN_SUMMARY_SCHEMA_VERSION,
        timestamp: EventWriter::timestamp(),
        run_id: "bench".into(),
        seed: 42,
        config_text: toml::to_string(&sweep_config).unwrap_or_default(),
        config: sweep_config.clone(),
        paths: RunPaths {
            summary: Some(summary_dest.display().to_string()),
            events: None,
            history: None,
            definitions: Some(definitions_dest.display().to_string()),
            results: Some(results_dest.display().to_string()),
            config: None,
            analysis_dir: None,
        },
        strategies: kernel.definitions().to_vec(),
        results: tournament_results.clone(),
        event_log: None,
        history_log: None,
        runtime: nit_games::RuntimeAcceleratorStats::new(sweep_config.engine.accelerator),
        run_dir: None,
    };

    c.bench_function("sweep_cell_io", |b| {
        b.iter(|| {
            let _ = nit_utils::fs::write_atomic(&definitions_dest, |w| {
                serde_json::to_writer_pretty(w, kernel.definitions()).map_err(std::io::Error::other)
            });
            let _ = nit_utils::fs::write_atomic(&results_dest, |w| {
                serde_json::to_writer_pretty(w, &tournament_results).map_err(std::io::Error::other)
            });
            let _ = nit_utils::fs::write_atomic(&summary_dest, |w| {
                serde_json::to_writer_pretty(w, &run_summary).map_err(std::io::Error::other)
            });
        });
    });
}

criterion_group!(
    benches,
    bench_single_match,
    bench_tournament_small,
    bench_tournament_medium,
    bench_logging,
    bench_parallel,
    bench_fast_eval,
    bench_fsm_fast_eval_heavy,
    bench_tm_micro,
    bench_tm_tournament,
    bench_tm_heavy,
    bench_tm_family_halting_filter,
    bench_sweep_io
);

criterion_main!(benches);
