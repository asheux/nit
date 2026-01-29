use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nit_games::config::{
    EngineConfig, HistoryConfig, NormalizedConfig, StrategySpec, StrategySpecKind,
};
use nit_games::events::EventWriter;
use nit_games::game::PayoffMatrix;
use nit_games::history_log::HistoryWriter;
use nit_games::tournament::{KernelRunMode, Parallelism, TournamentKernel};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn temp_path(prefix: &str, ext: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let mut path = std::env::temp_dir();
    path.push(format!("nit_bench_{prefix}_{pid}_{stamp}.{ext}"));
    path
}

fn build_config(strategies: usize, rounds: u32, repetitions: u32, self_play: bool) -> NormalizedConfig {
    let specs: Vec<StrategySpec> = (0..strategies)
        .map(|idx| StrategySpec {
            id: format!("rand{idx}"),
            name: None,
            kind: StrategySpecKind::Random { p_cooperate: 0.5 },
        })
        .collect();

    NormalizedConfig {
        schema_version: 1,
        game: "ipd".into(),
        rounds,
        repetitions,
        self_play,
        seed: Some(12345),
        noise: 0.0,
        payoff: PayoffMatrix::default_pd(),
        strategies: specs,
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
    }
}

fn build_deterministic_config(rounds: u32) -> NormalizedConfig {
    let specs = vec![
        StrategySpec {
            id: "allc".into(),
            name: None,
            kind: StrategySpecKind::Builtin {
                builtin: nit_games::config::BuiltinKind::AllC,
            },
        },
        StrategySpec {
            id: "tft".into(),
            name: None,
            kind: StrategySpecKind::Builtin {
                builtin: nit_games::config::BuiltinKind::TitForTat,
            },
        },
        StrategySpec {
            id: "grim".into(),
            name: None,
            kind: StrategySpecKind::Builtin {
                builtin: nit_games::config::BuiltinKind::GrimTrigger,
            },
        },
        StrategySpec {
            id: "wsls".into(),
            name: None,
            kind: StrategySpecKind::Builtin {
                builtin: nit_games::config::BuiltinKind::WinStayLoseShift,
            },
        },
        StrategySpec {
            id: "mem1".into(),
            name: None,
            kind: StrategySpecKind::Memory {
                n: 1,
                initial: nit_games::game::Action::Cooperate,
                table: vec![
                    nit_games::game::Action::Cooperate,
                    nit_games::game::Action::Defect,
                    nit_games::game::Action::Defect,
                    nit_games::game::Action::Cooperate,
                ],
            },
        },
        StrategySpec {
            id: "fsm".into(),
            name: None,
            kind: StrategySpecKind::Fsm {
                start_state: 0,
                input_index_base: 0,
                output: vec![
                    nit_games::game::Action::Cooperate,
                    nit_games::game::Action::Defect,
                ],
                transitions: vec![[0, 1, 0, 1], [1, 1, 1, 1]],
            },
        },
    ];

    NormalizedConfig {
        schema_version: 1,
        game: "ipd".into(),
        rounds,
        repetitions: 1,
        self_play: false,
        seed: Some(12345),
        noise: 0.0,
        payoff: PayoffMatrix::default_pd(),
        strategies: specs,
        event_log: nit_games::events::EventLogConfig {
            enabled: false,
            include_rounds: false,
        },
        history: HistoryConfig {
            enabled: false,
            include_cycle_metadata: false,
        },
        engine: EngineConfig::default(),
        max_memory_n: 1,
    }
}

fn bench_single_match(c: &mut Criterion) {
    let config = build_config(2, 200, 1, false);
    let kernel = TournamentKernel::new(config);
    c.bench_function("single_match_200_rounds", |b| {
        b.iter(|| {
            let _ = kernel.run(KernelRunMode::Sequential {
                event_writer: None,
                history_writer: None,
            });
        });
    });
}

fn bench_tournament_small(c: &mut Criterion) {
    let config = build_config(16, 200, 1, false);
    let kernel = TournamentKernel::new(config);
    c.bench_function("tournament_small", |b| {
        b.iter(|| {
            let _ = kernel.run(KernelRunMode::Sequential {
                event_writer: None,
                history_writer: None,
            });
        });
    });
}

fn bench_tournament_medium(c: &mut Criterion) {
    let config = build_config(128, 50, 1, false);
    let kernel = TournamentKernel::new(config);
    c.bench_function("tournament_medium", |b| {
        b.iter(|| {
            let _ = kernel.run(KernelRunMode::Sequential {
                event_writer: None,
                history_writer: None,
            });
        });
    });
}

fn bench_logging(c: &mut Criterion) {
    let config = build_config(8, 100, 1, false);
    let kernel = TournamentKernel::new(config);
    let mut group = c.benchmark_group("logging");
    group.bench_function(BenchmarkId::new("logging_off", "events"), |b| {
        b.iter(|| {
            let _ = kernel.run(KernelRunMode::Sequential {
                event_writer: None,
                history_writer: None,
            });
        });
    });
    group.bench_function(BenchmarkId::new("logging_on", "events_history"), |b| {
        b.iter(|| {
            let event_path = temp_path("events", "ndjson");
            let history_path = temp_path("history", "ndjson");
            let mut event_writer = EventWriter::new(event_path, true).expect("event writer");
            let mut history_writer = HistoryWriter::new(history_path).expect("history writer");
            let _ = kernel.run(KernelRunMode::Sequential {
                event_writer: Some(&mut event_writer),
                history_writer: Some(&mut history_writer),
            });
            let _ = event_writer.finish();
            let _ = history_writer.finish();
        });
    });
    group.finish();
}

fn bench_parallel(c: &mut Criterion) {
    let mut group = c.benchmark_group("parallel");
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    let config_small = build_config(64, 50, 1, false);
    let kernel_small = TournamentKernel::new(config_small);
    group.bench_function("tournament_parallel_auto", |b| {
        b.iter(|| {
            let _ = kernel_small.run(KernelRunMode::Parallel {
                parallelism: Parallelism::Auto,
                event_sender: None,
                include_rounds: false,
                history_sender: None,
            });
        });
    });

    let config_large = build_config(256, 50, 1, false);
    let kernel_large = TournamentKernel::new(config_large);
    group.bench_function("tournament_parallel_large", |b| {
        b.iter(|| {
            let _ = kernel_large.run(KernelRunMode::Parallel {
                parallelism: Parallelism::Auto,
                event_sender: None,
                include_rounds: false,
                history_sender: None,
            });
        });
    });
    group.finish();
}

fn bench_fast_eval(c: &mut Criterion) {
    let mut group = c.benchmark_group("fast_eval");
    let mut config_fast = build_deterministic_config(5000);
    config_fast.engine.fast_eval = true;
    let kernel_fast = TournamentKernel::new(config_fast.clone());
    group.bench_function("deterministic_fast", |b| {
        b.iter(|| {
            let _ = kernel_fast.run(KernelRunMode::Sequential {
                event_writer: None,
                history_writer: None,
            });
        });
    });

    config_fast.engine.fast_eval = false;
    let kernel_slow = TournamentKernel::new(config_fast);
    group.bench_function("deterministic_slow", |b| {
        b.iter(|| {
            let _ = kernel_slow.run(KernelRunMode::Sequential {
                event_writer: None,
                history_writer: None,
            });
        });
    });

    let mut mixed = build_config(12, 500, 1, false);
    mixed.engine.fast_eval = true;
    // Replace a few random strategies with deterministic ones.
    if mixed.strategies.len() >= 4 {
        mixed.strategies[0].kind = StrategySpecKind::Builtin {
            builtin: nit_games::config::BuiltinKind::AllC,
        };
        mixed.strategies[1].kind = StrategySpecKind::Builtin {
            builtin: nit_games::config::BuiltinKind::TitForTat,
        };
        mixed.strategies[2].kind = StrategySpecKind::Builtin {
            builtin: nit_games::config::BuiltinKind::GrimTrigger,
        };
        mixed.strategies[3].kind = StrategySpecKind::Memory {
            n: 1,
            initial: nit_games::game::Action::Cooperate,
            table: vec![
                nit_games::game::Action::Cooperate,
                nit_games::game::Action::Defect,
                nit_games::game::Action::Defect,
                nit_games::game::Action::Cooperate,
            ],
        };
    }
    let kernel_mixed = TournamentKernel::new(mixed);
    group.bench_function("mixed_random", |b| {
        b.iter(|| {
            let _ = kernel_mixed.run(KernelRunMode::Sequential {
                event_writer: None,
                history_writer: None,
            });
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_single_match,
    bench_tournament_small,
    bench_tournament_medium,
    bench_logging,
    bench_parallel,
    bench_fast_eval
);
criterion_main!(benches);
