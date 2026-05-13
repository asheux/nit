//! Config parsing + accelerator preflight tests.

use crate::config::{
    AcceleratorMode, FsmGroupingMode, GamesConfig, ScoreAggregation, StrategySpecKind,
};
use crate::output::RuntimeAcceleratorBackend;
use crate::{accelerator_preflight, accelerator_run_preflight, KernelRunMode, TournamentKernel};

#[cfg(target_os = "macos")]
use super::shared::metal_totals_or_skip;

#[test]
fn config_infers_fsm_from_fields_and_states_alias() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse fsm auto strategy");
    assert_eq!(cfg.strategies.len(), 1);
    match &cfg.strategies[0].kind {
        StrategySpecKind::Fsm {
            num_states,
            start_state,
            ..
        } => {
            assert_eq!(*num_states, 2);
            assert_eq!(*start_state, 0);
        }
        other => panic!("expected fsm, got {other:?}"),
    }
}

#[test]
fn config_infers_tm_from_auto_type() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[[strategy]]
id = "tm_auto"
type = "auto"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 8
rule_code = 1
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse tm auto strategy");
    assert_eq!(cfg.strategies.len(), 1);
    match &cfg.strategies[0].kind {
        StrategySpecKind::OneSidedTm {
            states,
            symbols,
            max_steps_per_round,
            ..
        } => {
            assert_eq!(*states, 1);
            assert_eq!(*symbols, 2);
            assert_eq!(*max_steps_per_round, 8);
        }
        other => panic!("expected one_sided_tm, got {other:?}"),
    }
}

#[test]
fn config_defaults_fsm_grouping_to_wnbm() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.fsm_grouping, FsmGroupingMode::Wnbm);
}

#[test]
fn config_parses_moorem_fsm_grouping_mode() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
fsm_grouping = "moorem"

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.fsm_grouping, FsmGroupingMode::Moorem);
}

#[test]
fn config_defaults_score_aggregation_to_mean() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.score_aggregation, ScoreAggregation::Mean);
}

#[test]
fn config_defaults_accelerator_to_auto() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.accelerator, AcceleratorMode::Auto);
}

#[test]
fn config_parses_cpu_accelerator() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
accelerator = "cpu"

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.accelerator, AcceleratorMode::Cpu);
}

#[test]
fn config_parses_metal_accelerator() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
accelerator = "metal"

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.accelerator, AcceleratorMode::Metal);
}

#[test]
fn metal_accelerator_preflight_rejects_fast_eval_false() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
accelerator = "metal"
fast_eval = false

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let err = accelerator_preflight(&cfg).expect_err("metal should require fast_eval");
    assert!(err.contains("fast_eval"));
}

#[test]
fn metal_accelerator_preflight_accepts_large_tm_step_count() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
accelerator = "metal"
fast_eval = true

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 2048
rule_code = 0
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    // Metal accepts any TM max_steps — the shader compiles with dynamic width.
    let _ = accelerator_preflight(&cfg);
}

#[test]
fn metal_run_preflight_rejects_logging_paths_before_backend_probe() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
accelerator = "metal"
fast_eval = true

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let err = accelerator_run_preflight(&cfg, true, true, false)
        .expect_err("metal should reject CPU-only logging features");
    assert!(err.contains("event logging"));
    assert!(err.contains("history logging"));
}

#[test]
fn metal_run_preflight_rejects_match_history_previews() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
accelerator = "metal"
fast_eval = true

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let err = accelerator_run_preflight(&cfg, false, false, true)
        .expect_err("metal should reject interactive previews");
    assert!(err.contains("interactive match history previews"));
}

#[test]
fn config_parses_total_score_aggregation() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
score_aggregation = "total"

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.score_aggregation, ScoreAggregation::Total);
}

#[test]
fn config_parses_save_data_false() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1
self_play = false
save_data = false

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert!(!cfg.save_data);
}

#[test]
fn metal_batch_path_can_be_disabled_in_config() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 4
repetitions = 1
self_play = false

[engine]
mode = "batch"
fast_eval = true
accelerator = "cpu"

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
    let totals = super::metal::metal_batch_totals_for_test(&cfg, &[(0, 1)])
        .expect("metal helper should not error");
    assert!(totals.is_none());
}

#[test]
fn kernel_runtime_stats_report_cpu_backend() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 4
repetitions = 1
self_play = false

[engine]
mode = "batch"
fast_eval = true
accelerator = "cpu"

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
    let kernel = TournamentKernel::new(cfg);
    let (_results, runtime) = kernel.run_with_runtime(KernelRunMode::Sequential {
        event_writer: None,
        history_writer: None,
    });

    assert_eq!(runtime.requested, AcceleratorMode::Cpu);
    assert_eq!(runtime.backend, RuntimeAcceleratorBackend::Cpu);
    assert_eq!(runtime.metal_matches, 0);
    assert_eq!(runtime.cpu_matches, 2);
}

#[cfg(target_os = "macos")]
#[test]
fn kernel_runtime_stats_report_metal_usage_when_available() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 4
repetitions = 1
self_play = false

[engine]
mode = "batch"
fast_eval = true
accelerator = "metal"

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
    let Some(_) = metal_totals_or_skip(&cfg, &[(0, 1), (1, 0)]) else {
        return;
    };
    let kernel = TournamentKernel::new(cfg);
    let (_results, runtime) = kernel.run_with_runtime(KernelRunMode::Sequential {
        event_writer: None,
        history_writer: None,
    });

    assert_eq!(runtime.requested, AcceleratorMode::Metal);
    assert_eq!(runtime.backend, RuntimeAcceleratorBackend::Metal);
    assert_eq!(runtime.metal_matches, 2);
    assert!(runtime.cpu_matches == 0);
    assert!(runtime.metal_policy_source.is_some());
    // Small workloads use the heuristic path which does not populate the policy cache.
}

#[test]
fn config_defaults_self_play_to_true() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert!(cfg.self_play);
}

#[test]
fn config_legacy_exact_alias_still_parses() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 2
repetitions = 1

[engine]
fsm_grouping = "exact"

[[strategy]]
id = "fsm_auto"
states = 2
start_state = 0
outputs = ["C", "D"]
transitions = [
  [0, 1],
  [0, 1],
]
"#;

    let cfg = GamesConfig::from_toml(src).expect("parse config");
    assert_eq!(cfg.engine.fsm_grouping, FsmGroupingMode::Moorem);
}
