use super::{
    adaptive_chunk_steps, dephase_steps_per_tick, finalize_run, next_adaptive_chunk_steps,
    normalize_chunk_steps, requested_steps_per_tick, start_run, steps_for_tick,
    uses_match_step_units, RunRequest, StepUnit,
};
use nit_games::{EngineMode, GamesConfig, NormalizedConfig};
use std::sync::mpsc;
use std::time::Duration;

fn parse_config(toml: &str) -> NormalizedConfig {
    GamesConfig::from_toml(toml).expect("parse config")
}

#[test]
fn dephase_steps_breaks_round_alignment() {
    assert_eq!(dephase_steps_per_tick(50_000, 20), 50_001);
    assert_eq!(dephase_steps_per_tick(20, 20), 21);
}

#[test]
fn dephase_steps_keeps_non_aligned_values() {
    assert_eq!(dephase_steps_per_tick(50_001, 20), 50_001);
    assert_eq!(dephase_steps_per_tick(1, 20), 1);
    assert_eq!(dephase_steps_per_tick(20, 1), 20);
}

#[test]
fn batch_steps_skip_dephase() {
    assert_eq!(steps_for_tick(50_000, 20, EngineMode::Batch, false), 50_000);
    assert_eq!(steps_for_tick(20, 20, EngineMode::Interactive, false), 21);
}

#[test]
fn match_step_units_convert_to_whole_matches() {
    assert_eq!(requested_steps_per_tick(3, 200, StepUnit::Matches), 600);
    assert_eq!(requested_steps_per_tick(3, 200, StepUnit::Rounds), 3);
}

#[test]
fn interactive_match_steps_skip_dephase() {
    assert_eq!(
        steps_for_tick(50_000, 20, EngineMode::Interactive, true),
        50_000
    );
}

#[test]
fn interactive_match_step_units_require_full_match_fast_forward() {
    let config = parse_config(
        r#"
schema_version = 1
game = "ipd"
rounds = 500000
repetitions = 1
self_play = false

[engine]
mode = "interactive"
fast_eval = true
accelerator = "auto"

[event_log]
enabled = false
include_rounds = false

[history]
enabled = false

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
"#,
    );

    assert!(uses_match_step_units(&config, false, false, false));
    assert!(!uses_match_step_units(&config, false, false, true));
}

#[test]
fn adaptive_chunk_steps_batch_starts_at_256_matches() {
    // Batch mode floor: rounds_per_match * 256 = 51200.
    let chunk = adaptive_chunk_steps(250_000, None, 200, EngineMode::Batch, false);
    assert_eq!(chunk, 51_200);
}

#[test]
fn adaptive_chunk_steps_preserves_batch_match_floor() {
    let chunk = adaptive_chunk_steps(400, Some(25), 200, EngineMode::Batch, false);
    assert_eq!(chunk, 200);
}

#[test]
fn next_adaptive_chunk_steps_grows_after_fast_chunk() {
    let next = next_adaptive_chunk_steps(
        4_096,
        Duration::from_millis(30),
        200,
        EngineMode::Batch,
        false,
    );
    assert_eq!(next, 16_384);
}

#[test]
fn next_adaptive_chunk_steps_shrinks_after_slow_chunk() {
    let next = next_adaptive_chunk_steps(
        4_096,
        Duration::from_millis(480),
        200,
        EngineMode::Batch,
        false,
    );
    assert_eq!(next, 2_048);
}

#[test]
fn next_adaptive_chunk_steps_can_grow_from_batch_match_floor() {
    let next = next_adaptive_chunk_steps(
        5_000,
        Duration::from_millis(30),
        5_000,
        EngineMode::Batch,
        false,
    );
    assert_eq!(next, 20_000);
}

// When chunk_steps is not a multiple of rounds_per_match (e.g. u32::MAX from
// saturating arithmetic), the remainder would trigger slow round-by-round CPU
// processing — normalize_chunk_steps must round down.
#[test]
fn whole_match_chunks_align_to_rounds_per_match() {
    let rounds_per_match = 500_000u32;
    let chunk = normalize_chunk_steps(
        u32::MAX,
        u32::MAX,
        rounds_per_match,
        EngineMode::Interactive,
        true,
    );
    assert_eq!(chunk % rounds_per_match, 0, "aligned to rounds_per_match");
    assert_eq!(chunk, (u32::MAX / rounds_per_match) * rounds_per_match);
}

#[test]
fn whole_match_chunks_preserve_already_aligned_values() {
    let chunk = normalize_chunk_steps(
        51_200, // 256 * 200, already aligned
        u32::MAX,
        200,
        EngineMode::Interactive,
        true,
    );
    assert_eq!(chunk, 51_200);
}

// Even with a sub-match candidate, result floors to one whole match.
#[test]
fn whole_match_chunks_floor_at_one_match() {
    let chunk = normalize_chunk_steps(100, u32::MAX, 500_000, EngineMode::Interactive, true);
    assert_eq!(chunk, 500_000);
}

#[test]
fn finalize_run_keeps_paths_empty_for_ephemeral_runs() {
    let config = parse_config(
        r#"
schema_version = 1
game = "ipd"
rounds = 4
repetitions = 1
self_play = false
save_data = false

[engine]
mode = "batch"
fast_eval = true
accelerator = "cpu"

[event_log]
enabled = true
include_rounds = false

[history]
enabled = true

[[strategy]]
id = "fsm_0"
type = "fsm"
index = 0
num_states = 1
k = 2
"#,
    );
    let (event_tx, _event_rx) = mpsc::channel();
    let request = RunRequest {
        config,
        config_text: String::new(),
        timestamp: "2026-03-12T00:00:00Z".into(),
        run_id: "run".into(),
        run_dir: None,
        summary_path: None,
        definitions_path: None,
        results_path: None,
        config_path: None,
        analysis_dir: None,
        event_path: None,
        history_path: None,
        progress_interval: Duration::from_millis(10),
        steps_per_tick: 128,
    };

    let state = start_run(request, &event_tx).expect("ephemeral run should start");
    let summary = finalize_run(state).expect("ephemeral run should finalize");
    assert!(summary.paths.summary.is_none());
    assert!(summary.paths.definitions.is_none());
    assert!(summary.paths.results.is_none());
    assert!(summary.paths.config.is_none());
    assert!(summary.paths.analysis_dir.is_none());
    assert!(summary.run_dir.is_none());
    assert!(summary.event_log.is_none());
    assert!(summary.history_log.is_none());
}
