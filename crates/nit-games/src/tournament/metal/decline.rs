use super::has_tm_step_cost_conflict;
use super::payload::build_metal_batch_payload;
use crate::config::{NormalizedConfig, StrategySpec};

pub(crate) fn metal_batch_decline_reason(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
    matchup_count: usize,
) -> Option<String> {
    if matchup_count == 0 {
        return Some("no matchups to evaluate".into());
    }
    if !config.engine.fast_eval {
        return Some("`engine.fast_eval = false` disables Metal batch evaluation".into());
    }
    if !config.engine.accelerator.allows_metal() {
        return Some("accelerator mode is set to CPU".into());
    }
    if config.noise != 0.0 {
        return Some("non-zero noise disables Metal batch evaluation".into());
    }
    if strategies.is_empty() {
        return None;
    }
    if has_tm_step_cost_conflict(config, strategies) {
        return Some("TM complexity penalties are not supported on the Metal path".into());
    }
    if build_metal_batch_payload(strategies).is_none() {
        return Some(
            "Metal batch evaluation requires a homogeneous FSM, CA, or TM roster \
             with shared structural parameters."
                .into(),
        );
    }
    None
}
