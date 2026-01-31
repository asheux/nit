use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nit_games::config::{
    EngineConfig, HistoryConfig, NormalizedConfig, StrategySpec, StrategySpecKind,
};
use nit_games::events::EventWriter;
use nit_games::game::PayoffMatrix;
use nit_games::history_log::HistoryWriter;
use nit_games::output::{RunPaths, RunSummary, RUN_SUMMARY_SCHEMA_VERSION};
use nit_games::tournament::{KernelRunMode, Parallelism, TournamentKernel};
use nit_games::{InputMode, Strategy, TmMove, TmTransition};
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
                num_states: 2,
                start_state: 0,
                outputs: vec![
                    nit_games::game::Action::Cooperate,
                    nit_games::game::Action::Defect,
                ],
                input_mode: Some(InputMode::JointLastAction),
                transitions: vec![vec![0, 1, 0, 1], vec![1, 1, 1, 1]],
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

fn build_fsm_heavy_config(strategies: usize, rounds: u32) -> NormalizedConfig {
    let mut specs = Vec::new();
    for idx in 0..strategies {
        let a = (idx % 2) as usize;
        let b = ((idx / 2) % 2) as usize;
        specs.push(StrategySpec {
            id: format!("fsm{idx}"),
            name: None,
            kind: StrategySpecKind::Fsm {
                num_states: 2,
                start_state: 0,
                outputs: vec![
                    nit_games::game::Action::Cooperate,
                    nit_games::game::Action::Defect,
                ],
                input_mode: Some(InputMode::JointLastAction),
                transitions: vec![vec![a, b, a, b], vec![b, a, b, a]],
            },
        });
    }
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

fn build_tm_config(strategies: usize, rounds: u32) -> NormalizedConfig {
    let mut specs = Vec::new();
    let transitions = vec![
        TmTransition {
            write: 0,
            move_dir: TmMove::Right,
            next: 1,
        },
        TmTransition {
            write: 1,
            move_dir: TmMove::Right,
            next: 1,
        },
    ];
    for idx in 0..strategies {
        specs.push(StrategySpec {
            id: format!("tm{idx}"),
            name: None,
            kind: StrategySpecKind::OneSidedTm {
                states: 1,
                symbols: 2,
                start_state: 1,
                blank: 0,
                fallback_symbol: Some(0),
                max_steps_per_round: 32,
                input_mode: InputMode::OpponentLastAction,
                output_map: vec![
                    nit_games::game::Action::Cooperate,
                    nit_games::game::Action::Defect,
                ],
                transitions: transitions.clone(),
                rule_code: None,
            },
        });
    }
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
        max_memory_n: 0,
    }
}

fn build_tm_heavy_config(strategies: usize, rounds: u32, max_steps: u32) -> NormalizedConfig {
    let mut specs = Vec::new();
    let transitions = vec![
        TmTransition {
            write: 0,
            move_dir: TmMove::Stay,
            next: 1,
        },
        TmTransition {
            write: 1,
            move_dir: TmMove::Stay,
            next: 1,
        },
    ];
    for idx in 0..strategies {
        specs.push(StrategySpec {
            id: format!("tm_heavy{idx}"),
            name: None,
            kind: StrategySpecKind::OneSidedTm {
                states: 1,
                symbols: 2,
                start_state: 1,
                blank: 0,
                fallback_symbol: Some(0),
                max_steps_per_round: max_steps,
                input_mode: InputMode::OpponentLastAction,
                output_map: vec![
                    nit_games::game::Action::Cooperate,
                    nit_games::game::Action::Defect,
                ],
                transitions: transitions.clone(),
                rule_code: None,
            },
        });
    }
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
        max_memory_n: 0,
    }
}

fn build_baseline_deterministic(strategies: usize, rounds: u32) -> NormalizedConfig {
    let builtins = [
        nit_games::config::BuiltinKind::AllC,
        nit_games::config::BuiltinKind::AllD,
        nit_games::config::BuiltinKind::TitForTat,
        nit_games::config::BuiltinKind::WinStayLoseShift,
    ];
    let specs = (0..strategies)
        .map(|idx| StrategySpec {
            id: format!("base{idx}"),
            name: None,
            kind: StrategySpecKind::Builtin {
                builtin: builtins[idx % builtins.len()],
            },
        })
        .collect();
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
        max_memory_n: 0,
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

fn bench_fsm_fast_eval_heavy(c: &mut Criterion) {
    let mut group = c.benchmark_group("fsm_fast_eval");
    let mut config_fast = build_fsm_heavy_config(32, 3000);
    config_fast.engine.fast_eval = true;
    let kernel_fast = TournamentKernel::new(config_fast.clone());
    group.bench_function("fsm_fast", |b| {
        b.iter(|| {
            let _ = kernel_fast.run(KernelRunMode::Sequential {
                event_writer: None,
                history_writer: None,
            });
        });
    });

    config_fast.engine.fast_eval = false;
    let kernel_slow = TournamentKernel::new(config_fast);
    group.bench_function("fsm_slow", |b| {
        b.iter(|| {
            let _ = kernel_slow.run(KernelRunMode::Sequential {
                event_writer: None,
                history_writer: None,
            });
        });
    });
    group.finish();
}

fn bench_tm_micro(c: &mut Criterion) {
    let transitions = vec![
        TmTransition {
            write: 0,
            move_dir: TmMove::Stay,
            next: 1,
        },
        TmTransition {
            write: 1,
            move_dir: TmMove::Stay,
            next: 1,
        },
    ];
    let mut tm = nit_games::OneSidedTmStrategy::new(
        "tm",
        2,
        1,
        0,
        0,
        256,
        InputMode::OpponentLastAction,
        vec![
            nit_games::game::Action::Cooperate,
            nit_games::game::Action::Defect,
        ],
        transitions,
    );
    let history = nit_games::History::new(1);
    c.bench_function("tm_micro_steps", |b| {
        b.iter(|| {
            for _ in 0..256 {
                let _ = tm.next_action(&history, true);
            }
        });
    });
}

fn bench_tm_tournament(c: &mut Criterion) {
    let mut group = c.benchmark_group("tm_tournament");
    let tm_config = build_tm_config(12, 200);
    let baseline = build_baseline_deterministic(12, 200);
    let kernel_tm = TournamentKernel::new(tm_config);
    let kernel_base = TournamentKernel::new(baseline);
    group.bench_function("tm", |b| {
        b.iter(|| {
            let _ = kernel_tm.run(KernelRunMode::Sequential {
                event_writer: None,
                history_writer: None,
            });
        });
    });
    group.bench_function("baseline", |b| {
        b.iter(|| {
            let _ = kernel_base.run(KernelRunMode::Sequential {
                event_writer: None,
                history_writer: None,
            });
        });
    });
    group.finish();
}

fn bench_tm_heavy(c: &mut Criterion) {
    let mut group = c.benchmark_group("tm_heavy");
    let tm_config = build_tm_heavy_config(8, 150, 512);
    let kernel_tm = TournamentKernel::new(tm_config);
    group.bench_function("tm_steps_heavy", |b| {
        b.iter(|| {
            let _ = kernel_tm.run(KernelRunMode::Sequential {
                event_writer: None,
                history_writer: None,
            });
        });
    });
    group.finish();
}

fn bench_sweep_io(c: &mut Criterion) {
    let config = build_tm_config(6, 60);
    let kernel = TournamentKernel::new(config.clone());
    let results = kernel.run(KernelRunMode::Sequential {
        event_writer: None,
        history_writer: None,
    });
    let summary_path = temp_path("sweep_summary", "json");
    let results_path = temp_path("sweep_results", "json");
    let definitions_path = temp_path("sweep_definitions", "json");

    let summary = RunSummary {
        schema_version: RUN_SUMMARY_SCHEMA_VERSION,
        timestamp: EventWriter::timestamp(),
        run_id: "bench".into(),
        seed: 42,
        config_text: toml::to_string(&config).unwrap_or_default(),
        config: config.clone(),
        paths: RunPaths {
            summary: Some(summary_path.display().to_string()),
            events: None,
            history: None,
            definitions: Some(definitions_path.display().to_string()),
            results: Some(results_path.display().to_string()),
            config: None,
            analysis_dir: None,
        },
        strategies: kernel.definitions().to_vec(),
        results: results.clone(),
        event_log: None,
        history_log: None,
        run_dir: None,
    };

    c.bench_function("sweep_cell_io", |b| {
        b.iter(|| {
            let _ = nit_utils::fs::write_atomic(&definitions_path, |writer| {
                serde_json::to_writer_pretty(writer, kernel.definitions())
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
            });
            let _ = nit_utils::fs::write_atomic(&results_path, |writer| {
                serde_json::to_writer_pretty(writer, &results)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
            });
            let _ = nit_utils::fs::write_atomic(&summary_path, |writer| {
                serde_json::to_writer_pretty(writer, &summary)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
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
    bench_sweep_io
);
criterion_main!(benches);
