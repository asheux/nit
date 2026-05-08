use super::has_tm_step_cost_conflict;
use super::payload::build_metal_batch_payload;
use crate::config::{NormalizedConfig, StrategySpec};

const HETEROGENEOUS_ROSTER: &str =
    "Metal batch evaluation requires a homogeneous FSM, CA, or TM roster \
                                    with shared structural parameters.";

pub(crate) fn metal_batch_decline_reason(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    matchup_count: usize,
) -> Option<String> {
    let reason: &str = if matchup_count == 0 {
        "no matchups to evaluate"
    } else if !config.engine.fast_eval {
        "`engine.fast_eval = false` disables Metal batch evaluation"
    } else if !config.engine.accelerator.allows_metal() {
        "accelerator mode is set to CPU"
    } else if config.noise != 0.0 {
        "non-zero noise disables Metal batch evaluation"
    } else if strategies.is_empty() {
        return None;
    } else if has_tm_step_cost_conflict(config, strategies) {
        "TM complexity penalties are not supported on the Metal path"
    } else if build_metal_batch_payload(strategies).is_none() {
        HETEROGENEOUS_ROSTER
    } else {
        return None;
    };
    Some(reason.into())
}
