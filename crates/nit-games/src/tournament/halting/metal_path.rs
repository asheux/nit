use super::super::metal::{metal_batch_decline_reason, try_prepare_metal_batch_for_workload};
use super::super::schedule::SchedulePlan;
use super::super::types::{
    MatchOutcome, Matchup, TmHaltingFilterBackend, TmHaltingFilterDiagnostics,
};
use super::tm_stats_always_halt;
use crate::config::NormalizedConfig;
use nit_metal::MatchPair;
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

const AUTO_TM_METAL_PROBE_TIMEOUT: Duration = Duration::from_millis(300);
static AUTO_TM_METAL_PROBE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Default)]
pub(super) struct MetalStats {
    pub(super) scanned_matchups: usize,
    pub(super) batches_submitted: usize,
    pub(super) prepare_elapsed: Duration,
    pub(super) execution_elapsed: Duration,
    pub(super) policy_source: String,
    pub(super) matches_per_batch: usize,
    pub(super) inflight_batches: usize,
    pub(super) policy_cache_key: Option<String>,
    pub(super) policy_cache_path: Option<String>,
}

type ProbeResult = Result<Option<(Vec<bool>, MetalStats)>, String>;

fn apply_halting_chunk(
    keep: &mut [bool],
    matchups: &[Matchup],
    halting: &[nit_metal::TmHaltingPair],
) -> Result<(), String> {
    if matchups.len() != halting.len() {
        return Err(format!(
            "Metal TM halting batch returned {} results for {} matchups",
            halting.len(),
            matchups.len()
        ));
    }
    for (matchup, outcome) in matchups.iter().zip(halting.iter()) {
        if !outcome.a_all_halted {
            keep[matchup.a_idx] = false;
        }
        if !outcome.b_all_halted {
            keep[matchup.b_idx] = false;
        }
    }
    Ok(())
}

pub(super) fn try_family_halting_mask(config: &NormalizedConfig) -> ProbeResult {
    struct PendingChunk {
        matchups: Vec<Matchup>,
        pending: nit_metal::PendingBatch,
    }

    let strategy_count = config.strategies.len();
    let mut keep = vec![true; strategy_count];
    let schedule = SchedulePlan::new(strategy_count, config.repetitions, config.self_play);
    if schedule.is_empty() {
        return Ok(Some((keep, MetalStats::default())));
    }

    let prepare_started = Instant::now();
    let Some(prepared) =
        try_prepare_metal_batch_for_workload(config, &config.strategies, schedule.len())?
    else {
        return Ok(None);
    };
    let prepare_elapsed = prepare_started.elapsed();

    let policy_source = format!("{:?}", prepared.policy_source);
    let policy_cache_key = prepared.policy_cache_key.clone();
    let policy_cache_path = prepared.policy_cache_path.clone();
    let matches_per_batch = prepared.policy.matches_per_batch.max(1);
    let inflight_batches = prepared.policy.inflight_batches.max(1);
    let mut pending = VecDeque::new();
    let mut batches_submitted = 0usize;
    let execution_started = Instant::now();

    for start in (0..schedule.len()).step_by(matches_per_batch) {
        let matchups = schedule.matchups(start, matches_per_batch);
        if matchups.is_empty() {
            continue;
        }
        let pairs = matchups
            .iter()
            .map(|matchup| MatchPair {
                a_idx: matchup.a_idx as u32,
                b_idx: matchup.b_idx as u32,
            })
            .collect::<Vec<_>>();
        let Some(batch) = nit_metal::try_begin_prepared_batch(&prepared.prepared, &pairs)? else {
            return Ok(None);
        };
        pending.push_back(PendingChunk {
            matchups,
            pending: batch,
        });
        batches_submitted += 1;
        if pending.len() >= inflight_batches {
            let ready = pending.pop_front().expect("pending TM halting chunk");
            let halting = nit_metal::try_finish_prepared_tm_halting_batch(ready.pending)?;
            apply_halting_chunk(&mut keep, &ready.matchups, &halting)?;
        }
    }

    while let Some(ready) = pending.pop_front() {
        let halting = nit_metal::try_finish_prepared_tm_halting_batch(ready.pending)?;
        apply_halting_chunk(&mut keep, &ready.matchups, &halting)?;
    }

    Ok(Some((
        keep,
        MetalStats {
            scanned_matchups: schedule.len(),
            batches_submitted,
            prepare_elapsed,
            execution_elapsed: execution_started.elapsed(),
            policy_source,
            matches_per_batch,
            inflight_batches,
            policy_cache_key,
            policy_cache_path,
        },
    )))
}

pub(super) fn mark_tm_halting_selection(
    keep: &mut [bool],
    tm_mask: &[bool],
    matchup: &Matchup,
    outcome: &MatchOutcome,
) {
    if tm_mask[matchup.a_idx] && !tm_stats_always_halt(outcome.a_tm_stats.as_ref()) {
        keep[matchup.a_idx] = false;
    }
    if tm_mask[matchup.b_idx] && !tm_stats_always_halt(outcome.b_tm_stats.as_ref()) {
        keep[matchup.b_idx] = false;
    }
}

pub(super) fn dispatch_probe(
    config: &NormalizedConfig,
    diagnostics: &mut TmHaltingFilterDiagnostics,
    probe_started: Instant,
) -> Option<ProbeResult> {
    use crate::config::AcceleratorMode;
    if !matches!(config.engine.accelerator, AcceleratorMode::Auto) {
        return Some(try_family_halting_mask(config));
    }

    if AUTO_TM_METAL_PROBE_IN_FLIGHT.swap(true, AtomicOrdering::AcqRel) {
        diagnostics.backend_probe_elapsed = probe_started.elapsed();
        diagnostics.metal_decline_reason =
            Some("Metal probe already in progress; using CPU fallback for this run.".into());
        return None;
    }

    let (sender, receiver) = std::sync::mpsc::channel();
    let probe_cfg = config.clone();
    std::thread::spawn(move || {
        let probe_outcome = catch_unwind(AssertUnwindSafe(|| try_family_halting_mask(&probe_cfg)))
            .unwrap_or_else(|_| Err("Metal probe panicked".into()));
        AUTO_TM_METAL_PROBE_IN_FLIGHT.store(false, AtomicOrdering::Release);
        let _ = sender.send(probe_outcome);
    });

    match receiver.recv_timeout(AUTO_TM_METAL_PROBE_TIMEOUT) {
        Ok(outcome) => Some(outcome),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            diagnostics.backend_probe_elapsed = probe_started.elapsed();
            diagnostics.metal_decline_reason = Some(format!(
                "Metal probe exceeded {}ms in auto mode; using CPU fallback",
                AUTO_TM_METAL_PROBE_TIMEOUT.as_millis()
            ));
            None
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            diagnostics.backend_probe_elapsed = probe_started.elapsed();
            diagnostics.metal_error = Some("Metal probe thread terminated unexpectedly".into());
            None
        }
    }
}

pub(super) fn apply_probe_result(
    probe_result: ProbeResult,
    config: &NormalizedConfig,
    schedule_len: usize,
    strict_metal: bool,
    diagnostics: &mut TmHaltingFilterDiagnostics,
    probe_started: Instant,
) -> Result<Option<Vec<bool>>, String> {
    match probe_result {
        Ok(Some((halting_keep, gpu_stats))) => {
            diagnostics.backend = TmHaltingFilterBackend::Metal;
            diagnostics.scanned_matchups = gpu_stats.scanned_matchups;
            diagnostics.backend_probe_elapsed = gpu_stats.prepare_elapsed;
            diagnostics.halting_filter_elapsed = gpu_stats.execution_elapsed;
            diagnostics.metal_batches_submitted = gpu_stats.batches_submitted;
            diagnostics.metal_policy_source = Some(gpu_stats.policy_source);
            diagnostics.metal_matches_per_batch = Some(gpu_stats.matches_per_batch);
            diagnostics.metal_inflight_batches = Some(gpu_stats.inflight_batches);
            diagnostics.metal_policy_cache_key = gpu_stats.policy_cache_key;
            diagnostics.metal_policy_cache_path = gpu_stats.policy_cache_path;
            Ok(Some(halting_keep))
        }
        Ok(None) => {
            diagnostics.backend_probe_elapsed = probe_started.elapsed();
            diagnostics.metal_decline_reason = metal_batch_decline_reason(
                config,
                &config.strategies,
                schedule_len,
            )
            .or_else(|| {
                Some("Metal batch evaluator declined this TM family preparation workload.".into())
            });
            if strict_metal && config.engine.accelerator.requires_metal() {
                let decline_msg = diagnostics.metal_decline_reason.as_deref().unwrap_or(
                    "TM family preparation is not supported by the active Metal backend",
                );
                return Err(format!(
                    "Metal accelerator was requested, but {decline_msg}."
                ));
            }
            Ok(None)
        }
        Err(gpu_error) => {
            diagnostics.backend_probe_elapsed = probe_started.elapsed();
            diagnostics.metal_error = Some(gpu_error.clone());
            if strict_metal && config.engine.accelerator.requires_metal() {
                return Err(format!(
                    "Metal accelerator unavailable during TM family preparation: {gpu_error}"
                ));
            }
            Ok(None)
        }
    }
}
