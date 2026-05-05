//! Metal GPU tournament tests. Every test here is `#[cfg(target_os = "macos")]`
//! and silently skips when no Metal device is available.

#![cfg(target_os = "macos")]

use super::shared::{metal_totals_or_skip, simple_four_state_fsm_spec, simulate_match_from_specs};
use crate::config::GamesConfig;
use crate::game::Action;
use crate::output::RuntimeAcceleratorBackend;
use crate::tournament::TournamentRunner;

#[test]
fn metal_fsm_batch_matches_cpu_baseline() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 12
self_play = false

[engine]
mode = "batch"
fast_eval = true

[[strategy]]
id = "all_c"
type = "fsm"
index = 0
num_states = 1
k = 2

[[strategy]]
id = "all_d"
type = "fsm"
index = 1
num_states = 1
k = 2
"#;
    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let Some(totals) = metal_totals_or_skip(&cfg, &[(0, 1), (1, 0)]) else {
        return;
    };
    let expected = vec![
        simulate_match_from_specs(
            &cfg.strategies[0],
            &cfg.strategies[1],
            cfg.payoff,
            cfg.rounds,
        ),
        simulate_match_from_specs(
            &cfg.strategies[1],
            &cfg.strategies[0],
            cfg.payoff,
            cfg.rounds,
        ),
    ];
    assert_eq!(totals, expected);
}

#[test]
fn metal_ca_batch_matches_cpu_baseline() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 10
self_play = false

[engine]
mode = "batch"
fast_eval = true

[[strategy]]
id = "ca_30"
type = "ca"
n = 30
k = 2
r = 1.0
t = 4

[[strategy]]
id = "ca_110"
type = "ca"
n = 110
k = 2
r = 1.0
t = 4
"#;
    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let Some(totals) = metal_totals_or_skip(&cfg, &[(0, 1), (1, 0)]) else {
        return;
    };
    let expected = vec![
        simulate_match_from_specs(
            &cfg.strategies[0],
            &cfg.strategies[1],
            cfg.payoff,
            cfg.rounds,
        ),
        simulate_match_from_specs(
            &cfg.strategies[1],
            &cfg.strategies[0],
            cfg.payoff,
            cfg.rounds,
        ),
    ];
    assert_eq!(totals, expected);
}

#[test]
fn metal_tm_batch_matches_cpu_baseline() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 8
self_play = false

[engine]
mode = "batch"
fast_eval = true

[[strategy]]
id = "tm_0"
type = "tm"
states = 2
symbols = 2
blank = 0
max_steps_per_round = 16
rule_code = 0

[[strategy]]
id = "tm_3"
type = "tm"
states = 2
symbols = 2
blank = 0
max_steps_per_round = 16
rule_code = 3
"#;
    let cfg = GamesConfig::from_toml(src).expect("parse config");
    let Some(totals) = metal_totals_or_skip(&cfg, &[(0, 1), (1, 0)]) else {
        return;
    };
    let expected = vec![
        simulate_match_from_specs(
            &cfg.strategies[0],
            &cfg.strategies[1],
            cfg.payoff,
            cfg.rounds,
        ),
        simulate_match_from_specs(
            &cfg.strategies[1],
            &cfg.strategies[0],
            cfg.payoff,
            cfg.rounds,
        ),
    ];
    assert_eq!(totals, expected);
}

#[test]
fn metal_large_homogeneous_four_state_fsm_roster_probe() {
    let mut cfg = GamesConfig::from_toml(
        r#"
schema_version = 1
game = "ipd"
rounds = 10
repetitions = 1
self_play = false
noise = 0.0

[engine]
mode = "batch"
fast_eval = true
accelerator = "metal"

[[strategy]]
id = "placeholder"
type = "fsm"
num_states = 1
outputs = ["C"]
transitions = [[0, 0]]
"#,
    )
    .expect("parse config");

    cfg.strategies = (0..52_000)
        .map(|idx| simple_four_state_fsm_spec(format!("fsm_{idx}")))
        .collect();

    let Some(totals) = metal_totals_or_skip(&cfg, &[(0, 1), (51_998, 51_999)]) else {
        return;
    };
    assert_eq!(totals.len(), 2);
}

#[test]
fn metal_large_homogeneous_four_state_fsm_roster_probe_with_self_play() {
    let mut cfg = GamesConfig::from_toml(
        r#"
schema_version = 1
game = "ipd"
rounds = 10
repetitions = 1
self_play = true
noise = 0.0

[engine]
mode = "batch"
fast_eval = true
accelerator = "metal"

[[strategy]]
id = "placeholder"
type = "fsm"
num_states = 1
outputs = ["C"]
transitions = [[0, 0]]
"#,
    )
    .expect("parse config");

    cfg.strategies = (0..52_000)
        .map(|idx| simple_four_state_fsm_spec(format!("fsm_{idx}")))
        .collect();

    let Some(totals) = metal_totals_or_skip(&cfg, &[(0, 0), (0, 1), (51_999, 51_999)]) else {
        return;
    };
    assert_eq!(totals.len(), 3);
}

#[test]
fn metal_large_homogeneous_four_state_fsm_full_chunk_probe() {
    let mut cfg = GamesConfig::from_toml(
        r#"
schema_version = 1
game = "ipd"
rounds = 10
repetitions = 1
self_play = false
noise = 0.0

[engine]
mode = "batch"
fast_eval = true
accelerator = "metal"

[[strategy]]
id = "placeholder"
type = "fsm"
num_states = 1
outputs = ["C"]
transitions = [[0, 0]]
"#,
    )
    .expect("parse config");

    cfg.strategies = (0..52_000)
        .map(|idx| simple_four_state_fsm_spec(format!("fsm_{idx}")))
        .collect();

    let pairs = (0..16_384usize)
        .map(|idx| (idx, 51_999usize.saturating_sub(idx)))
        .collect::<Vec<_>>();
    let Some(totals) = metal_totals_or_skip(&cfg, &pairs) else {
        return;
    };
    assert_eq!(totals.len(), pairs.len());
}

#[test]
fn metal_fast_forward_keeps_last_round_snapshot_for_single_match_chunk() {
    let src = r#"
schema_version = 1
game = "ipd"
rounds = 5000
repetitions = 1
self_play = false
noise = 0.0

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
    let Some(_) = metal_totals_or_skip(&cfg, &[(0, 1)]) else {
        return;
    };

    let mut runner = TournamentRunner::new(cfg).with_match_history_previews(false);
    runner.step_rounds(5_000);

    let progress = runner.progress().expect("progress should exist");
    assert!(progress.match_complete);
    assert_eq!(progress.match_index, 1);
    // FSM index 0 = AlwaysDefect, FSM index 1 = AlwaysCooperate.
    assert_eq!(progress.last_action_a, Some(Action::Defect));
    assert_eq!(progress.last_action_b, Some(Action::Cooperate));
    assert_eq!(progress.last_payoff_a, Some(0));
    assert_eq!(progress.last_payoff_b, Some(-3));
    // FSMs always report halted=true (default Strategy trait behavior).
    assert_eq!(progress.last_halted_a, Some(true));
    assert_eq!(progress.last_halted_b, Some(true));
    assert_eq!(progress.runtime.backend, RuntimeAcceleratorBackend::Metal);
    assert_eq!(progress.runtime.metal_matches, 1);
    assert_eq!(progress.runtime.cpu_matches, 0);
}

#[test]
#[ignore = "local Metal throughput profiling"]
fn metal_policy_profiles_four_state_fsm_on_local_device() {
    let mut cfg = GamesConfig::from_toml(
        r#"
schema_version = 1
game = "ipd"
rounds = 10
repetitions = 1
self_play = false
noise = 0.0

[engine]
mode = "batch"
fast_eval = true
accelerator = "metal"

[[strategy]]
id = "placeholder"
type = "fsm"
num_states = 1
outputs = ["C"]
transitions = [[0, 0]]
"#,
    )
    .expect("parse config");

    cfg.strategies = (0..52_000)
        .map(|idx| simple_four_state_fsm_spec(format!("fsm_{idx}")))
        .collect();

    let pairs = (0..524_288usize)
        .map(|idx| (idx % 52_000, 51_999usize.saturating_sub(idx % 52_000)))
        .collect::<Vec<_>>();

    let candidates = [
        (65_536usize, 3usize),
        (65_536usize, 4usize),
        (98_304usize, 4usize),
        (131_072usize, 3usize),
        (131_072usize, 4usize),
        (131_072usize, 5usize),
        (196_608usize, 4usize),
        (262_144usize, 3usize),
        (262_144usize, 4usize),
    ];

    const SAMPLES_PER_CANDIDATE: usize = 3;

    let mut baseline = None;
    let mut fastest = None;
    for (matches_per_batch, inflight_batches) in candidates {
        let mut total_elapsed = 0.0f64;
        let mut best_elapsed = f64::INFINITY;
        let mut checksum = (0i64, 0i64);
        for _ in 0..SAMPLES_PER_CANDIDATE {
            let Some((totals, elapsed)) = super::metal::metal_policy_probe_for_test(
                &cfg,
                &pairs,
                matches_per_batch,
                inflight_batches,
            )
            .expect("policy probe") else {
                return;
            };
            let sample_checksum = totals.iter().fold((0i64, 0i64), |acc, value| {
                (acc.0 + value.0, acc.1 + value.1)
            });
            if let Some(reference) = baseline.as_ref() {
                assert_eq!(&totals, reference, "policy changed Metal results");
            } else {
                baseline = Some(totals);
            }
            checksum = sample_checksum;
            let elapsed_secs = elapsed.as_secs_f64();
            total_elapsed += elapsed_secs;
            best_elapsed = best_elapsed.min(elapsed_secs);
        }
        let average_elapsed = total_elapsed / SAMPLES_PER_CANDIDATE as f64;
        let average_matches_per_second = pairs.len() as f64 / average_elapsed;
        let best_matches_per_second = pairs.len() as f64 / best_elapsed;
        println!(
            "metal_policy batch={} inflight={} avg={:.3}s avg_rate={:.0} best={:.3}s best_rate={:.0} checksum=({}, {})",
            matches_per_batch,
            inflight_batches,
            average_elapsed,
            average_matches_per_second,
            best_elapsed,
            best_matches_per_second,
            checksum.0,
            checksum.1
        );
        if fastest
            .as_ref()
            .map(|(_, _, best_average): &(usize, usize, f64)| average_elapsed < *best_average)
            .unwrap_or(true)
        {
            fastest = Some((matches_per_batch, inflight_batches, average_elapsed));
        }
    }
    if let Some((matches_per_batch, inflight_batches, average_elapsed)) = fastest {
        println!("metal_policy best batch={matches_per_batch} inflight={inflight_batches} avg={average_elapsed:.3}s");
    }
}
