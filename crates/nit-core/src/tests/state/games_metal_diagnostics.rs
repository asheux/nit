//! macOS-only Metal-backend diagnostics for `:games run tm` family builds.
//! Skips with an early return when the host has no usable Metal device.

#![cfg(target_os = "macos")]

use super::*;

const TM_METAL_DIAGNOSTICS_CONFIG: &str = r#"
schema_version = 1
game = "ipd"
rounds = 30
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

#[test]
fn command_games_run_tm_family_reports_metal_backend_when_available() {
    let root = temp_dir("cmd-games-run-tm-family-metal-diagnostics");
    let request = GamesFamilyRunRequest {
        family: "tm".into(),
        input: "{1, 2}".into(),
        force: false,
    };
    let (_override_run, timings) = match build_family_run_override_for_request_with_timings(
        &root,
        TM_METAL_DIAGNOSTICS_CONFIG,
        &request,
    ) {
        Ok(result) => result,
        Err(err)
            if err.contains("Metal accelerator unavailable")
                || err.contains("active Metal backend")
                || err.contains("Metal device unavailable") =>
        {
            return;
        }
        Err(err) => panic!("unexpected strict metal error: {err}"),
    };
    let diagnostics = timings.tm_filter.expect("expected TM prep diagnostics");
    assert!(matches!(
        diagnostics.backend,
        nit_games::TmHaltingFilterBackend::Metal
    ));
}
