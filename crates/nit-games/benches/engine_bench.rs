//! Benchmark suite for the `nit-games` tournament engine.
//!
//! Covers sequential and parallel kernel execution modes, cycle-detection fast-eval,
//! Turing-machine halting filters, event/history logging overhead, and sweep I/O
//! throughput across FSM, cellular automaton, and Turing machine strategy families.
//!
//! ## Benchmark groups
//!
//! | Group | Focus |
//! |-------|-------|
//! | Single match / tournament sizes | Baseline latency scaling from 2 to 128 strategies |
//! | Logging overhead | I/O cost of NDJSON event and history writers |
//! | Parallel execution | Rayon-parallel throughput at 64 and 256 strategies |
//! | Fast-eval cycle detection | Cycle-detecting vs brute-force execution |
//! | FSM fast-eval stress | Pure-FSM cycle detection at scale |
//! | TM micro / tournament / heavy | Raw tape-simulation cost and tournament overhead |
//! | TM halting filter | Pre-tournament halting analysis throughput |
//! | Sweep I/O serialisation | JSON write throughput for tournament artefacts |
//!
//! ## Running
//!
//! ```text
//! cargo bench -p nit-games            # full suite
//! cargo bench -p nit-games -- --test  # compile-check only
//! ```

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

use nit_games::config::{
    AcceleratorMode, EngineConfig, HistoryConfig, NormalizedConfig, StrategySpec, StrategySpecKind,
};
use nit_games::events::EventWriter;
use nit_games::game::{Action, PayoffMatrix};
use nit_games::history_log::HistoryWriter;
use nit_games::output::{RunPaths, RunSummary, RUN_SUMMARY_SCHEMA_VERSION};
use nit_games::tournament::{KernelRunMode, Parallelism, TournamentKernel};
use nit_games::{decode_tm_rule_code_wolfram, InputMode, Strategy, TmMove, TmTransition};

// ── Temporary file paths ────────────────────────────────────

/// Generate a collision-resistant temporary file path for benchmark I/O.
///
/// Combines the process ID with a nanosecond timestamp so that parallel
/// benchmark iterations never race on the same filesystem path.
fn temp_bench_path(label: &str, extension: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let mut target = std::env::temp_dir();
    target.push(format!("nit_bench_{label}_{pid}_{nanos}.{extension}"));
    target
}

// ── Strategy spec constructors ──────────────────────────────

/// The standard cooperate/defect output pair used by most benchmark strategies.
#[inline]
fn cooperate_defect_outputs() -> Vec<Action> {
    vec![Action::Cooperate, Action::Defect]
}

/// Build an FSM strategy spec with a given state count and transition table.
///
/// `start_state` is always 0; `input_mode` defaults to `OpponentLastAction`.
/// The `outputs` vector maps each state to its initial action.
fn fsm_spec(
    label: impl Into<String>,
    num_states: usize,
    outputs: Vec<Action>,
    transitions: Vec<Vec<usize>>,
) -> StrategySpec {
    StrategySpec {
        id: label.into(),
        name: None,
        kind: StrategySpecKind::Fsm {
            num_states,
            start_state: 0,
            outputs,
            input_mode: Some(InputMode::OpponentLastAction),
            transitions,
            index: None,
        },
    }
}

/// Build a Turing machine strategy spec with explicit transitions.
///
/// Uses 1-state, 2-symbol defaults (the most common benchmark configuration)
/// and allows overriding `max_steps_per_round` for heavy-compute scenarios.
fn tm_spec(
    label: impl Into<String>,
    transitions: Vec<TmTransition>,
    max_steps_per_round: u32,
) -> StrategySpec {
    StrategySpec {
        id: label.into(),
        name: None,
        kind: StrategySpecKind::OneSidedTm {
            states: 1,
            symbols: 2,
            start_state: 1,
            blank: 0,
            fallback_symbol: Some(0),
            max_steps_per_round,
            input_mode: InputMode::OpponentLastAction,
            output_map: cooperate_defect_outputs(),
            transitions,
            rule_code: None,
        },
    }
}

/// A minimal two-transition TM table: writes the opposite symbol and moves in
/// the given direction. Shared by `build_tm_uniform_tournament`.
fn binary_tm_transitions(movement: HeadMovement) -> Vec<TmTransition> {
    let move_dir = movement.to_tm_move();
    vec![
        TmTransition {
            write: 0,
            move_dir,
            next: 1,
        },
        TmTransition {
            write: 1,
            move_dir,
            next: 1,
        },
    ]
}

/// Build a cellular automaton strategy spec from rule parameters.
fn ca_spec(
    label: impl Into<String>,
    rule_number: u64,
    radius: f32,
    threshold: u32,
) -> StrategySpec {
    StrategySpec {
        id: label.into(),
        name: None,
        kind: StrategySpecKind::Ca {
            n: rule_number,
            k: 2,
            r: radius,
            t: threshold,
        },
    }
}

// ── Benchmark kernel wrapper ────────────────────────────────

/// Thin wrapper around [`TournamentKernel`] that bundles config and kernel
/// together, providing `run_sequential` as a convenience for benchmarks.
struct BenchTournament {
    kernel: TournamentKernel,
}

impl BenchTournament {
    fn from_config(config: NormalizedConfig) -> Self {
        let kernel = TournamentKernel::new(config);
        Self { kernel }
    }

    /// Execute one full tournament pass with no event or history writers.
    fn run_sequential(&self) -> nit_games::output::TournamentResults {
        self.kernel.run(KernelRunMode::Sequential {
            event_writer: None,
            history_writer: None,
        })
    }
}

// ── TM head-movement patterns ───────────────────────────────

/// Named movement patterns for TM benchmark transitions.
///
/// Using an enum instead of raw `TmMove` values improves readability at
/// call sites and lets each benchmark clearly declare its movement intent.
#[derive(Clone, Copy)]
enum HeadMovement {
    /// Head advances right each step — tape is consumed linearly.
    Advancing,
    /// Head stays in place — worst-case per-step cost (same cell rewritten).
    Stationary,
}

impl HeadMovement {
    /// Convert to the underlying `TmMove` for strategy construction.
    const fn to_tm_move(self) -> TmMove {
        match self {
            Self::Advancing => TmMove::Right,
            Self::Stationary => TmMove::Stay,
        }
    }
}

impl std::fmt::Display for HeadMovement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Advancing => f.write_str("right"),
            Self::Stationary => f.write_str("stay"),
        }
    }
}

/// Standard round count used for baseline benchmarks where latency scaling
/// (rather than absolute throughput) is the metric of interest.
const BASELINE_ROUNDS: u32 = 200;

// ── Config scaffold ─────────────────────────────────────────

/// Construct a `NormalizedConfig` with standard IPD benchmark defaults.
///
/// All optional fields (event logging, history, noise) are disabled.
/// The `engine` and `max_memory_n` fields use their defaults; callers
/// override them on the returned struct when needed.
fn config_scaffold(
    strategies: Vec<StrategySpec>,
    rounds: u32,
    repetitions: u32,
    self_play: bool,
) -> NormalizedConfig {
    NormalizedConfig {
        schema_version: 1,
        game: "ipd".into(),
        rounds,
        repetitions,
        self_play,
        save_data: true,
        seed: Some(12345),
        noise: 0.0,
        payoff: PayoffMatrix::default_pd(),
        strategies,
        event_log: nit_games::events::EventLogConfig {
            enabled: false,
            include_rounds: false,
        },
        history: HistoryConfig {
            enabled: false,
            include_cycle_metadata: false,
        },
        engine: EngineConfig::default(),
        max_memory_n: 0,
        tm_filter_applied: false,
    }
}

// ── Parameterised config builders ───────────────────────────

/// Variable-size FSM tournament: each strategy gets a unique 2-state transition
/// table derived from its index, producing varied cooperation/defection patterns.
fn build_generic_fsm_tournament(
    strategy_count: usize,
    rounds: u32,
    repetitions: u32,
    self_play: bool,
) -> NormalizedConfig {
    let specs: Vec<StrategySpec> = (0..strategy_count)
        .map(|idx| {
            let row_a = vec![idx % 2, (idx / 2) % 2];
            let row_b = vec![(idx / 2) % 2, idx % 2];
            fsm_spec(
                format!("fsm{idx}"),
                2,
                cooperate_defect_outputs(),
                vec![row_a, row_b],
            )
        })
        .collect();

    config_scaffold(specs, rounds, repetitions, self_play)
}

/// Classic IPD strategy suite for deterministic-outcome benchmarks.
///
/// Includes Always-Cooperate, Always-Defect, Tit-for-Tat, Anti-TFT,
/// a Rule-30 cellular automaton, and a minimal Turing machine.
fn build_deterministic_strategy_suite(rounds: u32) -> NormalizedConfig {
    let allc = fsm_spec("fsm_allc", 1, vec![Action::Cooperate], vec![vec![0, 0]]);
    let alld = fsm_spec("fsm_alld", 1, vec![Action::Defect], vec![vec![0, 0]]);

    let tft = fsm_spec(
        "fsm_tft",
        2,
        cooperate_defect_outputs(),
        vec![vec![0, 1], vec![0, 1]],
    );
    let anti_tft = fsm_spec(
        "fsm_anti_tft",
        2,
        cooperate_defect_outputs(),
        vec![vec![1, 0], vec![1, 0]],
    );

    let ca_rule30 = ca_spec("ca30", 30, 1.0, 2);
    let tm_basic = tm_spec("tm", binary_tm_transitions(HeadMovement::Advancing), 32);

    let mut cfg = config_scaffold(
        vec![allc, alld, tft, anti_tft, ca_rule30, tm_basic],
        rounds,
        1,
        false,
    );
    // Deterministic suite needs memory_n=1 for TFT lookback.
    cfg.max_memory_n = 1;
    cfg
}

/// Pure-FSM tournament with many 2-state strategies for fast-eval stress testing.
fn build_fsm_heavy_tournament(strategy_count: usize, rounds: u32) -> NormalizedConfig {
    let specs: Vec<StrategySpec> = (0..strategy_count)
        .map(|idx| {
            let a_transition = idx % 2;
            let b_transition = (idx / 2) % 2;
            fsm_spec(
                format!("fsm{idx}"),
                2,
                cooperate_defect_outputs(),
                vec![
                    vec![a_transition, b_transition],
                    vec![b_transition, a_transition],
                ],
            )
        })
        .collect();

    let mut cfg = config_scaffold(specs, rounds, 1, false);
    cfg.max_memory_n = 1;
    cfg
}

/// Uniform TM tournament: all strategies share the same transition table,
/// head movement pattern, and step budget.
fn build_tm_uniform_tournament(
    strategy_count: usize,
    rounds: u32,
    movement: HeadMovement,
    step_budget: u32,
    label_prefix: &str,
) -> NormalizedConfig {
    let transitions = binary_tm_transitions(movement);
    let specs: Vec<StrategySpec> = (0..strategy_count)
        .map(|idx| {
            tm_spec(
                format!("{label_prefix}{idx}"),
                transitions.clone(),
                step_budget,
            )
        })
        .collect();
    config_scaffold(specs, rounds, 1, false)
}

/// All 16 Wolfram-encoded 1-state-2-symbol TMs for halting filter benchmarks.
///
/// Forces CPU-only execution (`AcceleratorMode::Cpu`) so the halting filter
/// always takes the software path, giving stable measurements.
fn build_tm_family_reference(rounds: u32, step_budget: u32) -> NormalizedConfig {
    let specs: Vec<StrategySpec> = (0u64..=15)
        .map(|rule_code| {
            let (decoded_transitions, _remainder) = decode_tm_rule_code_wolfram(rule_code, 1, 2);
            StrategySpec {
                id: format!("tm_{rule_code}"),
                name: None,
                kind: StrategySpecKind::OneSidedTm {
                    states: 1,
                    symbols: 2,
                    start_state: 1,
                    blank: 0,
                    fallback_symbol: Some(0),
                    max_steps_per_round: step_budget,
                    input_mode: InputMode::OpponentLastAction,
                    output_map: cooperate_defect_outputs(),
                    transitions: decoded_transitions,
                    rule_code: Some(rule_code),
                },
            }
        })
        .collect();

    let mut cfg = config_scaffold(specs, rounds, 1, true);
    cfg.engine.accelerator = AcceleratorMode::Cpu;
    cfg
}

/// Minimal FSM baseline for A/B comparison against TM strategies.
///
/// Each strategy is a single-state automaton that either always cooperates
/// (even indices) or always defects (odd indices).
fn build_alternating_baseline(strategy_count: usize, rounds: u32) -> NormalizedConfig {
    let specs: Vec<StrategySpec> = (0..strategy_count)
        .map(|idx| {
            let action = if idx % 2 == 0 {
                Action::Cooperate
            } else {
                Action::Defect
            };
            fsm_spec(format!("base{idx}"), 1, vec![action], vec![vec![0, 0]])
        })
        .collect();

    config_scaffold(specs, rounds, 1, false)
}

// ── Benchmarks: single match and tournament sizes ───────────

/// Baseline latency for a single 200-round match between two FSM strategies.
fn bench_single_match(c: &mut Criterion) {
    let pair_match =
        BenchTournament::from_config(build_generic_fsm_tournament(2, BASELINE_ROUNDS, 1, false));
    c.bench_function("single_match_200_rounds", |b| {
        b.iter(|| pair_match.run_sequential());
    });
}

/// Small tournament: 16 strategies, 200 rounds, sequential execution.
fn bench_tournament_small(c: &mut Criterion) {
    let small_field =
        BenchTournament::from_config(build_generic_fsm_tournament(16, BASELINE_ROUNDS, 1, false));
    c.bench_function("tournament_small", |b| {
        b.iter(|| small_field.run_sequential());
    });
}

/// Medium tournament: 128 strategies, 50 rounds, sequential execution.
/// The larger field produces O(n^2) matchups, stressing the scheduler.
fn bench_tournament_medium(c: &mut Criterion) {
    let medium_field =
        BenchTournament::from_config(build_generic_fsm_tournament(128, 50, 1, false));
    c.bench_function("tournament_medium", |b| {
        b.iter(|| medium_field.run_sequential());
    });
}

// ── Benchmarks: logging overhead ────────────────────────────

/// Measures the I/O cost of NDJSON event and history logging during a tournament.
///
/// Compares a no-writers run against one with both `EventWriter` and
/// `HistoryWriter` active, writing to temporary files that are discarded
/// after each iteration.
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

// ── Benchmarks: parallel execution ──────────────────────────

/// Rayon-parallel tournament at two field sizes (64 and 256 strategies).
///
/// Uses `Parallelism::Auto` which defers to the global Rayon pool.
/// The warm-up and measurement times are shortened to keep wall-clock
/// time reasonable for the 256-strategy variant.
fn bench_parallel(c: &mut Criterion) {
    let mut group = c.benchmark_group("parallel");
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    // 64-strategy field: moderate contention on the Rayon pool.
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

    // 256-strategy field: heavy contention, O(256^2 / 2) matchups.
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

// ── Benchmarks: fast-eval cycle detection ───────────────────

/// Compare fast-eval (cycle-detecting) vs brute-force execution.
///
/// Fast-eval detects deterministic cycles early and short-circuits the
/// remaining rounds. The "deterministic" subgroup uses pure FSM strategies
/// that always cycle; the "mixed" subgroup blends FSM, CA, and TM families
/// to exercise the heterogeneous fast-eval path.
fn bench_fast_eval(c: &mut Criterion) {
    let mut group = c.benchmark_group("fast_eval");

    let deterministic_config = build_deterministic_strategy_suite(5000);
    bench_fast_eval_pair(
        &mut group,
        deterministic_config,
        "deterministic_fast",
        "deterministic_slow",
    );

    // Mixed family benchmark: splice FSM/CA/TM into slots 0-3.
    let mut mixed_config = build_generic_fsm_tournament(12, 500, 1, false);
    mixed_config.engine.fast_eval = true;
    apply_mixed_strategy_overrides(&mut mixed_config);

    let bench_mixed = BenchTournament::from_config(mixed_config);

    group.bench_function("mixed_fsm_ca_tm", |b| {
        b.iter(|| bench_mixed.run_sequential());
    });

    group.finish();
}

/// Overwrite the first four strategy slots with one of each family type,
/// creating a heterogeneous tournament for mixed fast-eval testing.
fn apply_mixed_strategy_overrides(cfg: &mut NormalizedConfig) {
    if cfg.strategies.len() < 4 {
        return;
    }

    // Slot 0: single-state Always-Cooperate FSM.
    cfg.strategies[0] = fsm_spec(
        cfg.strategies[0].id.clone(),
        1,
        vec![Action::Cooperate],
        vec![vec![0, 0]],
    );

    // Slot 1: two-state Tit-for-Tat FSM.
    cfg.strategies[1] = fsm_spec(
        cfg.strategies[1].id.clone(),
        2,
        cooperate_defect_outputs(),
        vec![vec![0, 1], vec![0, 1]],
    );

    // Slot 2: Rule-30 cellular automaton (radius 1, threshold 3).
    cfg.strategies[2] = ca_spec(cfg.strategies[2].id.clone(), 30, 1.0, 3);

    // Slot 3: minimal right-moving Turing machine.
    cfg.strategies[3] = tm_spec(
        cfg.strategies[3].id.clone(),
        binary_tm_transitions(HeadMovement::Advancing),
        32,
    );
}

/// Benchmark a config with fast_eval on vs off, using the given label pair.
fn bench_fast_eval_pair(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    mut config: NormalizedConfig,
    fast_label: &str,
    slow_label: &str,
) {
    config.engine.fast_eval = true;
    let bench_fast = BenchTournament::from_config(config.clone());
    group.bench_function(fast_label, |b| {
        b.iter(|| bench_fast.run_sequential());
    });

    config.engine.fast_eval = false;
    let bench_slow = BenchTournament::from_config(config);
    group.bench_function(slow_label, |b| {
        b.iter(|| bench_slow.run_sequential());
    });
}

// ── Benchmarks: FSM fast-eval stress ────────────────────────

/// Stress-test fast-eval with 32 FSM strategies over 3000 rounds.
///
/// Compares fast-eval (which should detect short cycles in all-FSM fields)
/// against the brute-force baseline. The gap between these two measurements
/// quantifies the cycle-detection benefit for pure-FSM tournaments.
fn bench_fsm_fast_eval_heavy(c: &mut Criterion) {
    let mut group = c.benchmark_group("fsm_fast_eval");
    let fsm_config = build_fsm_heavy_tournament(32, 3000);
    bench_fast_eval_pair(&mut group, fsm_config, "fsm_fast", "fsm_slow");
    group.finish();
}

// ── Benchmarks: Turing machine micro and tournament ─────────

/// Micro-benchmark for raw TM evaluation throughput.
///
/// Calls `next_action` 256 times on a single Turing machine strategy
/// without any tournament scaffolding, isolating the tape-simulation cost.
fn bench_tm_micro(c: &mut Criterion) {
    let stay_transitions = binary_tm_transitions(HeadMovement::Stationary);

    let mut tm_strategy = nit_games::OneSidedTmStrategy::new(
        "tm",
        2,   // symbols
        1,   // start_state
        0,   // blank symbol
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

/// TM tournament with FSM baseline comparison.
///
/// Runs the same 12-strategy, 200-round schedule with TM strategies and
/// with equivalent FSM baseline strategies, quantifying the overhead of
/// tape simulation relative to finite-state lookup.
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

/// Heavy TM computation: 8 strategies with 512-step budgets and stationary heads.
///
/// This represents the worst-case per-round cost for TM evaluation — the head
/// never moves, so every step touches the same tape cell and the full budget
/// is always consumed.
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

// ── Benchmarks: TM halting filter ───────────────────────────

/// Pre-tournament TM halting filter over the full 1-state-2-symbol Wolfram family.
///
/// The halting filter identifies non-halting TM strategies before the tournament
/// begins. This benchmark measures the filter's throughput on 16 TM strategies
/// (all 4-bit Wolfram codes) with a 1000-step budget per round.
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

// ── Benchmarks: sweep I/O serialisation ─────────────────────

/// Measures JSON serialisation throughput for tournament artefacts.
///
/// Writes strategy definitions, tournament results, and a full run summary
/// to temporary files via `nit_utils::fs::write_atomic`, simulating the
/// I/O path taken during a sweep run's cell-completion step.
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
            // Definitions, results, and summary are written atomically to
            // avoid partial reads from concurrent processes.
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

// ── Criterion harness ───────────────────────────────────────

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
