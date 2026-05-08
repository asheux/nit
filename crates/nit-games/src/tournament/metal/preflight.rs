use super::has_tm_step_cost_conflict;
use super::payload::{build_metal_batch_payload, metal_batch_eval_config};
use crate::config::NormalizedConfig;
use nit_metal::MatchPair;

pub fn accelerator_preflight(config: &NormalizedConfig) -> Result<(), String> {
    if !config.engine.accelerator.requires_metal() {
        return Ok(());
    }
    if !config.engine.fast_eval {
        return Err("Metal accelerator requires `engine.fast_eval = true`.".into());
    }
    if config.noise != 0.0 {
        return Err("Metal accelerator requires `noise = 0.0`.".into());
    }
    if config.strategies.is_empty() {
        return Ok(());
    }
    if has_tm_step_cost_conflict(config, &config.strategies) {
        return Err(
            "Metal accelerator does not support TM complexity penalties; \
             disable `engine.complexity_cost.tm_step_cost` or use `accelerator = \"auto\"`."
                .into(),
        );
    }

    let payload = build_metal_batch_payload(&config.strategies).ok_or_else(|| {
        "Metal accelerator requires a homogeneous FSM, CA, or TM roster \
         that the Metal batch evaluator can encode."
            .to_string()
    })?;

    let eval = metal_batch_eval_config(config);
    let prepared = nit_metal::try_prepare_batch(&eval, &payload)?.ok_or_else(|| {
        "Metal accelerator was requested, but this run is not supported \
         by the active Metal backend."
            .to_string()
    })?;

    let probe_pair = [MatchPair { a_idx: 0, b_idx: 0 }];
    match nit_metal::try_evaluate_prepared_batch(&prepared, &probe_pair) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(
            "Metal accelerator was requested, but this run is not supported \
             by the active Metal backend."
                .into(),
        ),
        Err(err) => Err(format!("Metal accelerator unavailable: {err}")),
    }
}

pub fn accelerator_run_preflight(
    config: &NormalizedConfig,
    event_logging: bool,
    history_logging: bool,
    match_history_previews: bool,
) -> Result<(), String> {
    if !config.engine.accelerator.requires_metal() {
        return Ok(());
    }

    let blockers = [
        (event_logging, "event logging"),
        (history_logging, "history logging"),
        (match_history_previews, "interactive match history previews"),
    ]
    .into_iter()
    .filter_map(|(active, label)| active.then_some(label))
    .collect::<Vec<_>>();

    if !blockers.is_empty() {
        return Err(format!(
            "Metal accelerator was requested, but {} currently requires the CPU path. \
             Disable those features or use `accelerator = \"auto\"`.",
            join_with_oxford_comma(&blockers)
        ));
    }

    accelerator_preflight(config)
}

fn join_with_oxford_comma(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [single] => (*single).to_string(),
        [left, right] => format!("{left} and {right}"),
        _ => {
            let (init, last) = items.split_at(items.len() - 1);
            format!("{}, and {}", init.join(", "), last[0])
        }
    }
}
