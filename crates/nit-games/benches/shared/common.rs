//! Shared scaffolding for the `engine_bench` Criterion suite.
//!
//! This module owns every spec / config builder, the `BenchTournament`
//! façade, and the head-movement enum used by the TM benchmarks. The bench
//! file consumes these helpers so each `bench_*` fn can focus on its
//! Criterion plumbing.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use nit_games::config::{
    AcceleratorMode, EngineConfig, HistoryConfig, NormalizedConfig, StrategySpec, StrategySpecKind,
};
use nit_games::game::{Action, PayoffMatrix};
use nit_games::tournament::{KernelRunMode, TournamentKernel};
use nit_games::{decode_tm_rule_code_wolfram, InputMode, TmMove, TmTransition};

pub const BASELINE_ROUNDS: u32 = 200;

/// PID + nanosecond timestamp keeps parallel benchmark iterations from
/// racing on the same temp file.
pub fn temp_bench_path(label: &str, extension: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let mut target = std::env::temp_dir();
    target.push(format!("nit_bench_{label}_{pid}_{nanos}.{extension}"));
    target
}

pub fn cooperate_defect_outputs() -> Vec<Action> {
    vec![Action::Cooperate, Action::Defect]
}

pub fn fsm_spec(
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

pub fn tm_spec(
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

pub fn binary_tm_transitions(movement: HeadMovement) -> Vec<TmTransition> {
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

pub fn ca_spec(
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

#[derive(Clone, Copy)]
pub enum HeadMovement {
    Advancing,
    Stationary,
}

impl HeadMovement {
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

pub struct BenchTournament {
    kernel: TournamentKernel,
}

impl BenchTournament {
    pub fn from_config(config: NormalizedConfig) -> Self {
        Self {
            kernel: TournamentKernel::new(config),
        }
    }

    pub fn run_sequential(&self) -> nit_games::output::TournamentResults {
        self.kernel.run(KernelRunMode::Sequential {
            event_writer: None,
            history_writer: None,
        })
    }
}

pub fn config_scaffold(
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

pub fn build_generic_fsm_tournament(
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

pub fn build_deterministic_strategy_suite(rounds: u32) -> NormalizedConfig {
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
    // memory_n=1 lets TFT see the previous round.
    cfg.max_memory_n = 1;
    cfg
}

pub fn build_fsm_heavy_tournament(strategy_count: usize, rounds: u32) -> NormalizedConfig {
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
/// head movement, and step budget.
pub fn build_tm_uniform_tournament(
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

/// All 16 Wolfram-encoded 1-state-2-symbol TMs for halting-filter benches.
/// Forces CPU-only execution so the filter always takes the software path,
/// which keeps measurements stable.
pub fn build_tm_family_reference(rounds: u32, step_budget: u32) -> NormalizedConfig {
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

/// Minimal FSM baseline for A/B comparison against TM strategies. Even
/// indices always cooperate, odd indices always defect.
pub fn build_alternating_baseline(strategy_count: usize, rounds: u32) -> NormalizedConfig {
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

/// Splice one strategy from each family (FSM AllC, FSM TFT, CA Rule-30, TM)
/// into the first four slots, producing a heterogeneous fast-eval workload.
pub fn apply_mixed_strategy_overrides(cfg: &mut NormalizedConfig) {
    if cfg.strategies.len() < 4 {
        return;
    }

    cfg.strategies[0] = fsm_spec(
        cfg.strategies[0].id.clone(),
        1,
        vec![Action::Cooperate],
        vec![vec![0, 0]],
    );
    cfg.strategies[1] = fsm_spec(
        cfg.strategies[1].id.clone(),
        2,
        cooperate_defect_outputs(),
        vec![vec![0, 1], vec![0, 1]],
    );
    cfg.strategies[2] = ca_spec(cfg.strategies[2].id.clone(), 30, 1.0, 3);
    cfg.strategies[3] = tm_spec(
        cfg.strategies[3].id.clone(),
        binary_tm_transitions(HeadMovement::Advancing),
        32,
    );
}

/// Run a config with `fast_eval` toggled on then off, recording each pass
/// under the supplied label.
pub fn bench_fast_eval_pair(
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
