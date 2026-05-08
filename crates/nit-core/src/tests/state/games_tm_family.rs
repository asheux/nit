//! `:games run tm` and `:games run ca` family-build paths — TM filter
//! diagnostics, accelerator selection, and the related CA family wire-up.
//!
//! macOS-only Metal-backend assertions live in `games_metal_diagnostics.rs`.

use super::*;

const TM_BASE_CONFIG: &str = r#"
schema_version = 1
game = "ipd"
rounds = 5
repetitions = 1
noise = 0.0

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 32
rule_code = 0
"#;

const TM_TIMING_CONFIG: &str = r#"
schema_version = 1
game = "ipd"
rounds = 200
repetitions = 1
self_play = true
noise = 0.0

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 32
rule_code = 0
"#;

const TM_AUTO_FAST_EVAL_OFF_CONFIG: &str = r#"
schema_version = 1
game = "ipd"
rounds = 30
repetitions = 1
noise = 0.0

[engine]
accelerator = "auto"
fast_eval = false

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 32
rule_code = 0
"#;

const TM_AUTO_NOISE_CONFIG: &str = r#"
schema_version = 1
game = "ipd"
rounds = 30
repetitions = 1
noise = 0.1

[engine]
accelerator = "auto"
fast_eval = true

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 32
rule_code = 0
"#;

const TM_STRICT_METAL_NOISE_CONFIG: &str = r#"
schema_version = 1
game = "ipd"
rounds = 30
repetitions = 1
noise = 0.1

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
max_steps_per_round = 32
rule_code = 0
"#;

const TM_STRICT_METAL_CONFIG: &str = r#"
schema_version = 1
game = "ipd"
rounds = 5
repetitions = 1
noise = 0.0

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
max_steps_per_round = 32
rule_code = 0
"#;

const TM_BLANK_HINT_CONFIG: &str = r#"
schema_version = 1
game = "ipd"
rounds = 5
repetitions = 1
noise = 0.0

[[strategy]]
id = "tm_rule"
type = "tm"
states = 1
symbols = 2
start_state = 1
blank = 1
max_steps_per_round = 32
rule_code = 0
"#;

const TM_GENERATED_BASE_CONFIG: &str = r#"
schema_version = 1
game = "ipd"
rounds = 5
repetitions = 1
noise = 0.0

[[strategy]]
id = "generated_tm"
type = "generated"
source = "missing-strategies.wl"
"#;

const TM_BAD_MAX_STEPS_CONFIG: &str = r#"
schema_version = 1
game = "ipd"
rounds = 5
repetitions = 1
noise = 0.0
"#;

const CA_BASE_CONFIG: &str = r#"
schema_version = 1
game = "ipd"
rounds = 6
repetitions = 1
noise = 0.0

[[strategy]]
id = "ca_rule"
type = "ca"
n = 30
k = 2
r = 1
t = 2
"#;

fn tm_family_request(input: &str) -> GamesFamilyRunRequest {
    GamesFamilyRunRequest {
        family: "tm".into(),
        input: input.into(),
        force: false,
    }
}

#[test]
fn command_games_run_tm_family_queues_generated_override() {
    let (_root, mut state) = games_state_with_config("cmd-games-run-tm-family", TM_BASE_CONFIG);
    assert!(!handle_command_line(&mut state, ":games run tm {1, 2}"));
    assert!(!state.games.pending_run);
    let request = state
        .games
        .pending_family_run
        .as_ref()
        .expect("expected queued family request");
    let override_run = build_family_run_override_for_request(
        &state.workspace_root,
        &state.editor_buffer().content_as_string(),
        request,
    )
    .expect("expected generated override");
    assert!(!override_run.config.strategies.is_empty());
    assert!(override_run.config.strategies.iter().all(|spec| matches!(
        spec.kind,
        nit_games::config::StrategySpecKind::OneSidedTm { .. }
    )));
    assert!(override_run.config.strategies.iter().all(|spec| matches!(
        spec.kind,
        nit_games::config::StrategySpecKind::OneSidedTm {
            max_steps_per_round: DEFAULT_FAMILY_TM_MAX_STEPS,
            ..
        }
    )));
    assert!(override_run.config.tm_filter_applied);
    assert!(matches!(
        override_run.config.engine.mode,
        nit_games::EngineMode::Batch
    ));
    assert!(override_run.config.engine.fast_eval);
    assert!(!override_run.config.event_log.enabled);
    assert!(!override_run.config.history.enabled);
}

#[test]
fn command_games_run_tm_family_reports_build_stage_timings() {
    let root = temp_dir("cmd-games-run-tm-family-timings");
    let request = tm_family_request("{1, 2}");
    let (_override_run, timings) =
        build_family_run_override_for_request_with_timings(&root, TM_TIMING_CONFIG, &request)
            .expect("expected generated override with timings");
    println!(
        "tm_family_build timings: generation={:?} estimate={:?} normalize={:?} total={:?}",
        timings.generation_elapsed,
        timings.estimate_elapsed,
        timings.normalize_elapsed,
        timings.total_elapsed
    );
    if let Some(diagnostics) = timings.tm_filter.as_ref() {
        println!(
            "tm_family_build filter: backend={} decline={:?} error={:?}",
            diagnostics.backend.label(),
            diagnostics.metal_decline_reason,
            diagnostics.metal_error
        );
    }
    assert_eq!(timings.generated_strategies, 16);
    assert!(timings.tm_filter.is_some());
    assert!(timings.total_elapsed >= timings.generation_elapsed);
}

#[test]
fn command_games_run_tm_family_forces_fast_eval_in_auto_mode_before_tm_prep() {
    let root = temp_dir("cmd-games-run-tm-family-fast-eval-auto");
    let request = tm_family_request("{1, 2}");
    let (override_run, timings) = build_family_run_override_for_request_with_timings(
        &root,
        TM_AUTO_FAST_EVAL_OFF_CONFIG,
        &request,
    )
    .expect("family build should succeed");
    assert!(override_run.config.engine.fast_eval);
    let diagnostics = timings.tm_filter.expect("expected TM prep diagnostics");
    assert_eq!(
        diagnostics.requested_accelerator,
        nit_games::AcceleratorMode::Auto
    );
    assert!(diagnostics
        .metal_decline_reason
        .as_deref()
        .map(|reason| !reason.contains("fast_eval = false"))
        .unwrap_or(true));
}

#[test]
fn command_games_run_tm_family_reports_noise_fallback_reason() {
    let root = temp_dir("cmd-games-run-tm-family-noise-reason");
    let request = tm_family_request("{1, 2}");
    let (_override_run, timings) =
        build_family_run_override_for_request_with_timings(&root, TM_AUTO_NOISE_CONFIG, &request)
            .expect("family build should succeed with CPU fallback");
    let diagnostics = timings.tm_filter.expect("expected TM prep diagnostics");
    assert!(matches!(
        diagnostics.backend,
        nit_games::TmHaltingFilterBackend::NotebookCpuFallback
    ));
    let reason = diagnostics.metal_decline_reason.unwrap_or_default();
    assert!(
        reason.contains("non-zero noise"),
        "unexpected fallback reason: {reason}"
    );
}

#[test]
fn command_games_run_tm_family_strict_metal_fails_loudly_on_noise() {
    let root = temp_dir("cmd-games-run-tm-family-strict-metal-noise");
    let request = tm_family_request("{1, 2}");
    let err = build_family_run_override_for_request(&root, TM_STRICT_METAL_NOISE_CONFIG, &request)
        .expect_err("strict metal TM family prep should fail on unsupported noise");
    assert!(err.contains("Metal accelerator"), "unexpected error: {err}");
    assert!(err.contains("noise"), "unexpected error: {err}");
}

#[test]
fn command_games_run_tm_family_accepts_post_tuple_max_steps() {
    let (_root, mut state) =
        games_state_with_config("cmd-games-run-tm-family-max-steps", TM_BASE_CONFIG);
    assert!(!handle_command_line(&mut state, ":games run tm {1, 2} 7"));
    let request = state
        .games
        .pending_family_run
        .as_ref()
        .expect("expected queued family request");
    let override_run = build_family_run_override_for_request(
        &state.workspace_root,
        &state.editor_buffer().content_as_string(),
        request,
    )
    .expect("expected generated override");
    assert!(!override_run.config.strategies.is_empty());
    assert!(override_run.config.strategies.iter().all(|spec| matches!(
        spec.kind,
        nit_games::config::StrategySpecKind::OneSidedTm {
            max_steps_per_round: 7,
            ..
        }
    )));
}

#[test]
fn command_games_run_tm_family_build_keeps_strict_metal_behavior() {
    let (_root, mut state) = games_state_with_config(
        "cmd-games-run-tm-family-metal-build",
        TM_STRICT_METAL_CONFIG,
    );
    assert!(!handle_command_line(&mut state, ":games run tm {1, 2}"));
    let request = state
        .games
        .pending_family_run
        .as_ref()
        .expect("expected queued family request");
    let result = build_family_run_override_for_request_with_timings(
        &state.workspace_root,
        &state.editor_buffer().content_as_string(),
        request,
    );
    match result {
        Ok((_override_run, timings)) => {
            let diagnostics = timings.tm_filter.expect("expected TM prep diagnostics");
            assert!(matches!(
                diagnostics.backend,
                nit_games::TmHaltingFilterBackend::Metal
            ));
        }
        Err(err) => {
            assert!(
                err.contains("Metal accelerator"),
                "unexpected strict metal error: {err}"
            );
        }
    }
}

#[test]
fn command_games_run_tm_family_ignores_existing_generated_strategy_sources() {
    let (_root, mut state) = games_state_with_config(
        "cmd-games-run-tm-family-generated-base",
        TM_GENERATED_BASE_CONFIG,
    );
    assert!(!handle_command_line(&mut state, ":games run tm {1, 2}"));
    let request = state
        .games
        .pending_family_run
        .as_ref()
        .expect("expected queued family request");
    let override_run = build_family_run_override_for_request(
        &state.workspace_root,
        &state.editor_buffer().content_as_string(),
        request,
    )
    .expect("family build should ignore unrelated generated sources");
    assert!(!override_run.config.strategies.is_empty());
    assert!(override_run.config.tm_filter_applied);
}

#[test]
fn command_games_run_tm_family_preserves_inline_blank_hint() {
    let (_root, mut state) =
        games_state_with_config("cmd-games-run-tm-family-blank-hint", TM_BLANK_HINT_CONFIG);
    assert!(!handle_command_line(&mut state, ":games run tm {1, 2}"));
    let request = state
        .games
        .pending_family_run
        .as_ref()
        .expect("expected queued family request");
    let override_run = build_family_run_override_for_request(
        &state.workspace_root,
        &state.editor_buffer().content_as_string(),
        request,
    )
    .expect("family build should preserve explicit TM blank hint");
    assert!(override_run.config.strategies.iter().all(|spec| matches!(
        spec.kind,
        nit_games::config::StrategySpecKind::OneSidedTm { blank: 1, .. }
    )));
}

#[test]
fn command_games_run_tm_family_rejects_invalid_post_tuple_max_steps() {
    let (_root, mut state) = games_state_with_config(
        "cmd-games-run-tm-family-bad-max-steps",
        TM_BAD_MAX_STEPS_CONFIG,
    );
    assert!(!handle_command_line(
        &mut state,
        ":games run tm {1, 2} nope"
    ));
    assert!(state.games.pending_family_run.is_none());
    let status = state.status.clone().unwrap_or_default();
    assert!(status.contains("max_steps"));
}

#[test]
fn command_games_run_ca_family_queues_generated_override() {
    let (_root, mut state) = games_state_with_config("cmd-games-run-ca-family", CA_BASE_CONFIG);
    assert!(!handle_command_line(&mut state, ":games run ca {2, 1}"));
    assert!(!state.games.pending_run);
    let request = state
        .games
        .pending_family_run
        .as_ref()
        .expect("expected queued family request");
    let override_run = build_family_run_override_for_request(
        &state.workspace_root,
        &state.editor_buffer().content_as_string(),
        request,
    )
    .expect("expected generated override");
    assert_eq!(override_run.config.strategies.len(), 256);
    assert!(override_run
        .config
        .strategies
        .iter()
        .all(|spec| matches!(spec.kind, nit_games::config::StrategySpecKind::Ca { .. })));
}
