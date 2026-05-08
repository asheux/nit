//! TOML fixtures and roster builders for the `tournament_metal` test module.
//!
//! macOS-only because every consumer is `#[cfg(target_os = "macos")]`. The
//! large 52 000-strategy rosters intentionally exceed Metal's small-workload
//! threshold so the dispatch path exercises the full policy lookup.

#![cfg(target_os = "macos")]

use super::super::shared::simple_four_state_fsm_spec;
use crate::config::{GamesConfig, NormalizedConfig};

const PLACEHOLDER_TOML_HEAD: &str = r#"
schema_version = 1
game = "ipd"
rounds = 10
repetitions = 1
"#;

const PLACEHOLDER_TOML_TAIL: &str = r#"noise = 0.0

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
"#;

fn placeholder_metal_toml(self_play: bool) -> String {
    format!("{PLACEHOLDER_TOML_HEAD}self_play = {self_play}\n{PLACEHOLDER_TOML_TAIL}")
}

pub(super) fn large_four_state_fsm_config(self_play: bool, count: usize) -> NormalizedConfig {
    let mut cfg = GamesConfig::from_toml(&placeholder_metal_toml(self_play)).expect("parse config");
    cfg.strategies = (0..count)
        .map(|idx| simple_four_state_fsm_spec(format!("fsm_{idx}")))
        .collect();
    cfg
}
