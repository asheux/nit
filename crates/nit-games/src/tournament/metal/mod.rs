mod decline;
mod dispatch;
mod payload;
mod preflight;
mod prep;

use crate::config::{ComplexityCostConfig, NormalizedConfig, StrategySpec, StrategySpecKind};
use crate::strategy::TmRunStats;

pub(crate) use decline::metal_batch_decline_reason;
pub(crate) use dispatch::try_metal_batch_outcomes_chunked_prepared;
pub use preflight::{accelerator_preflight, accelerator_run_preflight};
pub(crate) use prep::try_prepare_metal_batch_for_workload;

#[cfg(test)]
pub(crate) use dispatch::{encode_matchup_pairs, match_outcomes_from_scores};
#[cfg(test)]
pub(crate) use prep::try_prepare_metal_batch;

pub(super) fn adjusted_total_for_match(
    raw_total: i64,
    strategy: &StrategySpec,
    round_count: u32,
    tm_run_stats: Option<&TmRunStats>,
    cost: &ComplexityCostConfig,
) -> f64 {
    if !cost.enabled {
        return raw_total as f64;
    }
    raw_total as f64 - complexity_penalty(strategy, round_count, tm_run_stats, cost)
}

fn complexity_penalty(
    strategy: &StrategySpec,
    round_count: u32,
    tm_run_stats: Option<&TmRunStats>,
    cost: &ComplexityCostConfig,
) -> f64 {
    match &strategy.kind {
        StrategySpecKind::OneSidedTm { .. } if cost.tm_step_cost != 0.0 => {
            let total_steps = tm_run_stats.map_or(0.0, |stats| stats.steps as f64);
            cost.tm_step_cost * total_steps
        }
        StrategySpecKind::Fsm {
            num_states,
            outputs,
            ..
        } if cost.fsm_state_cost != 0.0 => {
            let state_count = (*num_states).max(outputs.len()) as f64;
            cost.fsm_state_cost * state_count * round_count as f64
        }
        _ => 0.0,
    }
}

pub(super) fn has_tm_step_cost_conflict(
    config: &NormalizedConfig,
    strategies: &[StrategySpec],
) -> bool {
    config.engine.complexity_cost.enabled
        && config.engine.complexity_cost.tm_step_cost != 0.0
        && strategies
            .iter()
            .all(|spec| matches!(spec.kind, StrategySpecKind::OneSidedTm { .. }))
}
