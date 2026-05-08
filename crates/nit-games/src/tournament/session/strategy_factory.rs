use crate::config::{StrategySpec, StrategySpecKind};
use crate::output::StrategyDefinition;
use crate::strategy::{CaStrategy, FsmStrategy, OneSidedTmStrategy, Strategy};

pub(in crate::tournament) fn build_strategy_definitions(
    strategies: &[StrategySpec],
) -> Vec<StrategyDefinition> {
    strategies
        .iter()
        .map(|spec| StrategyDefinition {
            id: spec.id.clone(),
            name: spec.name.clone(),
            kind: spec.kind.clone(),
            rng_seed_a: None,
            rng_seed_b: None,
        })
        .collect()
}

// Compact display id: prefer the numeric FSM index, CA rule number, or TM
// rule code when present; otherwise fall back to the registered string id.
pub(in crate::tournament) fn strategy_log_id(spec: &StrategySpec) -> String {
    match &spec.kind {
        StrategySpecKind::Fsm {
            index: Some(index), ..
        } => index.to_string(),
        StrategySpecKind::Ca { n, .. } => n.to_string(),
        StrategySpecKind::OneSidedTm {
            rule_code: Some(rule_code),
            ..
        } => rule_code.to_string(),
        _ => spec.id.clone(),
    }
}

pub(crate) fn build_strategy(spec: &StrategySpec, _seed: u64) -> Box<dyn Strategy> {
    match &spec.kind {
        StrategySpecKind::Fsm {
            start_state,
            outputs,
            input_mode,
            transitions,
            ..
        } => Box::new(FsmStrategy::new(
            spec.id.clone(),
            *start_state,
            outputs.clone(),
            input_mode.unwrap_or_default(),
            transitions.clone(),
        )),
        StrategySpecKind::Ca { n, k, r, t } => Box::new(CaStrategy::new(
            spec.id.clone(),
            *n,
            *k,
            (*r * 2.0).round() as u32,
            *t,
        )),
        StrategySpecKind::OneSidedTm {
            symbols,
            start_state,
            blank,
            max_steps_per_round,
            transitions,
            ..
        } => Box::new(OneSidedTmStrategy::new(
            spec.id.clone(),
            *symbols,
            *start_state,
            *blank,
            *max_steps_per_round,
            transitions.clone(),
        )),
    }
}
