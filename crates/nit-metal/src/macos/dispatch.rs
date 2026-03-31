//! Metal GPU dispatch and batch execution for game-theory tournaments.
//!
//! Manages GPU buffer allocation, command encoding, and the lifecycle
//! of prepared and pending batches across FSM, CA, and TM payloads.

use crate::{
    BatchEvalConfig, BatchPayload, MatchPair, ScorePair, TmHaltingPair, TmTransitionPacked,
};
use metal::{MTLResourceOptions, MTLSize};
use std::ffi::c_void;
use std::mem::{size_of, size_of_val};
use std::slice;

use super::shader::{context_for_key, MetalContext, ShaderKey};

// ---------------------------------------------------------------------------
// GPU-side repr(C) structs mirroring the Metal shader parameter layouts
// ---------------------------------------------------------------------------

/// Parameters shared across all kernel dispatches.
#[repr(C)]
#[derive(Copy, Clone)]
struct EvalParams {
    rounds: u32,
    pair_count: u32,
    cc_a: i32,
    cc_b: i32,
    cd_a: i32,
    cd_b: i32,
    dc_a: i32,
    dc_b: i32,
    dd_a: i32,
    dd_b: i32,
    timeout_lose: i32,
    timeout_win: i32,
}

/// FSM-specific kernel parameters.
#[repr(C)]
#[derive(Copy, Clone)]
struct FsmParams {
    states: u32,
    alphabet: u32,
}

/// Cellular automaton kernel parameters.
#[repr(C)]
#[derive(Copy, Clone)]
struct CaParams {
    symbols: u32,
    two_r: u32,
    steps: u32,
    rule_table_len: u32,
}

/// Turing machine kernel parameters.
#[repr(C)]
#[derive(Copy, Clone)]
struct TmParams {
    states: u32,
    symbols: u32,
    blank: u32,
    max_steps: u32,
    transitions_per_strategy: u32,
}

/// GPU-side match pair with aligned layout.
#[repr(C)]
#[derive(Copy, Clone)]
struct MatchPairPod {
    a_idx: u32,
    b_idx: u32,
}

/// GPU-side score accumulator with aligned layout.
#[repr(C)]
#[derive(Copy, Clone)]
struct ScorePairPod {
    a_total: i64,
    b_total: i64,
}

/// GPU-side TM halting flags.
#[repr(C)]
#[derive(Copy, Clone)]
struct TmHaltingPairPod {
    a_all_halted: u32,
    b_all_halted: u32,
}

/// GPU-side TM transition with explicit padding for Metal alignment.
#[repr(C)]
#[derive(Copy, Clone)]
struct TmTransitionPod {
    write: u32,
    move_dir: u32,
    next: u32,
    _pad: u32,
}

// ---------------------------------------------------------------------------
// Prepared and pending batch types
// ---------------------------------------------------------------------------

/// Pre-uploaded payload buffers, ready for repeated dispatch with different pairs.
enum PreparedPayload {
    Fsm {
        params: FsmParams,
        starts: metal::Buffer,
        outputs: metal::Buffer,
        transitions: metal::Buffer,
    },
    Ca {
        params: CaParams,
        rule_tables: metal::Buffer,
    },
    Tm {
        params: TmParams,
        starts: metal::Buffer,
        transitions: metal::Buffer,
    },
}

/// A fully prepared batch: shader selected, payload uploaded to GPU.
///
/// Reusable across multiple dispatches with different match pair sets.
pub struct PreparedBatch {
    shader_key: ShaderKey,
    eval_config: BatchEvalConfig,
    payload: PreparedPayload,
}

/// A submitted GPU command buffer awaiting completion.
///
/// Owns the input/output buffers to keep them alive until the GPU finishes.
pub struct PendingBatch {
    _pair_input: metal::Buffer,
    score_output: metal::Buffer,
    tm_halting_output: Option<metal::Buffer>,
    command_buffer: metal::CommandBuffer,
    dispatched_pair_count: usize,
}

// ---------------------------------------------------------------------------
// Buffer helpers
// ---------------------------------------------------------------------------

/// Returns the GPU memory footprint per match pair (input + output buffers).
pub(super) fn bytes_per_match_pair() -> usize {
    size_of::<MatchPairPod>() + size_of::<ScorePairPod>()
}

/// Converts config + pair count into the GPU-side `EvalParams` struct.
fn build_eval_params(config: &BatchEvalConfig, pair_count: usize) -> EvalParams {
    EvalParams {
        rounds: config.rounds,
        pair_count: pair_count as u32,
        cc_a: config.payoff[0][0][0],
        cc_b: config.payoff[0][0][1],
        cd_a: config.payoff[0][1][0],
        cd_b: config.payoff[0][1][1],
        dc_a: config.payoff[1][0][0],
        dc_b: config.payoff[1][0][1],
        dd_a: config.payoff[1][1][0],
        dd_b: config.payoff[1][1][1],
        timeout_lose: config.timeout_lose,
        timeout_win: config.timeout_win,
    }
}

/// Creates a Metal buffer initialized from a CPU-side slice.
fn buffer_from_slice<T>(device: &metal::Device, data: &[T]) -> metal::Buffer {
    if data.is_empty() {
        return device.new_buffer(1, MTLResourceOptions::StorageModeShared);
    }
    let byte_length = size_of_val(data) as u64;
    device.new_buffer_with_data(
        data.as_ptr() as *const c_void,
        byte_length,
        MTLResourceOptions::StorageModeShared,
    )
}

/// Allocates a zeroed output buffer for `count` elements of type `T`.
fn allocate_output_buffer<T>(device: &metal::Device, count: usize) -> metal::Buffer {
    device.new_buffer(
        (count.max(1) * size_of::<T>()) as u64,
        MTLResourceOptions::StorageModeShared,
    )
}

// ---------------------------------------------------------------------------
// GPU result extraction (unsafe: reads from raw buffer pointers)
// ---------------------------------------------------------------------------

/// Reads score pairs from a completed GPU output buffer.
///
/// # Safety
/// The buffer must contain at least `count` valid `ScorePairPod` values
/// and the command buffer must have completed execution.
unsafe fn extract_scores(buffer: &metal::BufferRef, count: usize) -> Vec<ScorePair> {
    let raw_ptr = buffer.contents() as *const ScorePairPod;
    let gpu_scores = slice::from_raw_parts(raw_ptr, count);
    gpu_scores
        .iter()
        .map(|pod| ScorePair {
            a_total: pod.a_total,
            b_total: pod.b_total,
        })
        .collect()
}

/// Reads TM halting flags from a completed GPU output buffer.
///
/// # Safety
/// The buffer must contain at least `count` valid `TmHaltingPairPod` values
/// and the command buffer must have completed execution.
unsafe fn extract_tm_halting(buffer: &metal::BufferRef, count: usize) -> Vec<TmHaltingPair> {
    let raw_ptr = buffer.contents() as *const TmHaltingPairPod;
    let gpu_flags = slice::from_raw_parts(raw_ptr, count);
    gpu_flags
        .iter()
        .map(|pod| TmHaltingPair {
            a_all_halted: pod.a_all_halted != 0,
            b_all_halted: pod.b_all_halted != 0,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// GPU dispatch
// ---------------------------------------------------------------------------

/// Casts a reference to a `*const c_void` for Metal's `set_bytes`.
fn as_void_ptr<T>(value: &T) -> *const c_void {
    (value as *const T).cast()
}

/// Encodes and commits a compute command, returning the owned command buffer.
fn encode_and_commit(
    pipeline: &metal::ComputePipelineState,
    command_queue: &metal::CommandQueue,
    set_buffers: impl FnOnce(&metal::ComputeCommandEncoderRef),
    thread_count: usize,
) -> metal::CommandBuffer {
    let cmd_buf = command_queue.new_command_buffer();
    let encoder = cmd_buf.new_compute_command_encoder();
    encoder.set_compute_pipeline_state(pipeline);
    set_buffers(encoder);

    let thread_width = pipeline.thread_execution_width().max(1);
    let threads_per_group = MTLSize {
        width: thread_width,
        height: 1,
        depth: 1,
    };
    let group_count = MTLSize {
        width: (thread_count as u64).div_ceil(thread_width),
        height: 1,
        depth: 1,
    };

    encoder.dispatch_thread_groups(group_count, threads_per_group);
    encoder.end_encoding();
    cmd_buf.commit();
    cmd_buf.to_owned()
}

/// Converts public `MatchPair` values to their GPU-side pod representation.
fn match_pair_pods(pairs: &[MatchPair]) -> Vec<MatchPairPod> {
    pairs
        .iter()
        .map(|mp| MatchPairPod {
            a_idx: mp.a_idx,
            b_idx: mp.b_idx,
        })
        .collect()
}

/// Converts public `TmTransitionPacked` values to padded GPU-side pods.
fn tm_transition_pods(transitions: &[TmTransitionPacked]) -> Vec<TmTransitionPod> {
    transitions
        .iter()
        .map(|tr| TmTransitionPod {
            write: tr.write,
            move_dir: tr.move_dir,
            next: tr.next,
            _pad: 0,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Core dispatch: submits a prepared batch to the GPU
// ---------------------------------------------------------------------------

/// Encodes and dispatches a prepared batch with the given match pairs.
fn dispatch_prepared(
    ctx: &MetalContext,
    prepared: &PreparedBatch,
    pairs: &[MatchPair],
) -> Result<PendingBatch, String> {
    let pods = match_pair_pods(pairs);
    let pair_buf = buffer_from_slice(&ctx.device, &pods);
    let score_buf = allocate_output_buffer::<ScorePairPod>(&ctx.device, pods.len());
    let eval = build_eval_params(&prepared.eval_config, pods.len());

    let mut halting_buf = None;

    let cmd = match &prepared.payload {
        PreparedPayload::Fsm {
            params,
            starts,
            outputs,
            transitions,
        } => encode_and_commit(
            &ctx.fsm_pipeline,
            &ctx.queue,
            |enc| {
                enc.set_buffer(0, Some(&pair_buf), 0);
                enc.set_buffer(1, Some(starts), 0);
                enc.set_buffer(2, Some(outputs), 0);
                enc.set_buffer(3, Some(transitions), 0);
                enc.set_buffer(4, Some(&score_buf), 0);
                enc.set_bytes(5, size_of::<EvalParams>() as u64, as_void_ptr(&eval));
                enc.set_bytes(6, size_of::<FsmParams>() as u64, as_void_ptr(params));
            },
            pods.len(),
        ),

        PreparedPayload::Ca {
            params,
            rule_tables,
        } => encode_and_commit(
            &ctx.ca_pipeline,
            &ctx.queue,
            |enc| {
                enc.set_buffer(0, Some(&pair_buf), 0);
                enc.set_buffer(1, Some(rule_tables), 0);
                enc.set_buffer(2, Some(&score_buf), 0);
                enc.set_bytes(3, size_of::<EvalParams>() as u64, as_void_ptr(&eval));
                enc.set_bytes(4, size_of::<CaParams>() as u64, as_void_ptr(params));
            },
            pods.len(),
        ),

        PreparedPayload::Tm {
            params,
            starts,
            transitions,
        } => {
            let halting = allocate_output_buffer::<TmHaltingPairPod>(&ctx.device, pods.len());
            let submitted = encode_and_commit(
                &ctx.tm_pipeline,
                &ctx.queue,
                |enc| {
                    enc.set_buffer(0, Some(&pair_buf), 0);
                    enc.set_buffer(1, Some(starts), 0);
                    enc.set_buffer(2, Some(transitions), 0);
                    enc.set_buffer(3, Some(&score_buf), 0);
                    enc.set_bytes(4, size_of::<EvalParams>() as u64, as_void_ptr(&eval));
                    enc.set_bytes(5, size_of::<TmParams>() as u64, as_void_ptr(params));
                    enc.set_buffer(6, Some(&halting), 0);
                },
                pods.len(),
            );
            halting_buf = Some(halting);
            submitted
        }
    };

    Ok(PendingBatch {
        _pair_input: pair_buf,
        score_output: score_buf,
        tm_halting_output: halting_buf,
        command_buffer: cmd,
        dispatched_pair_count: pods.len(),
    })
}

// ---------------------------------------------------------------------------
// Public batch API
// ---------------------------------------------------------------------------

/// Evaluates a full batch request synchronously, returning scores or `None`
/// if Metal is unavailable.
pub fn try_evaluate_batch(
    request: &crate::BatchRequest,
) -> Result<Option<Vec<ScorePair>>, String> {
    let eval_config = BatchEvalConfig {
        rounds: request.common.rounds,
        payoff: request.common.payoff,
        timeout_lose: request.common.timeout_lose,
        timeout_win: request.common.timeout_win,
    };
    let Some(prepared) = try_prepare_batch(&eval_config, &request.payload)? else {
        return Ok(None);
    };
    try_evaluate_prepared_batch(&prepared, &request.common.pairs)
}

/// Uploads payload data to the GPU, returning a reusable `PreparedBatch`.
pub fn try_prepare_batch(
    config: &BatchEvalConfig,
    payload: &BatchPayload,
) -> Result<Option<PreparedBatch>, String> {
    let shader_key = ShaderKey::for_payload(payload);
    let ctx = context_for_key(shader_key)?;

    let prepared_payload = match payload {
        BatchPayload::Fsm(fsm) => PreparedPayload::Fsm {
            params: FsmParams {
                states: fsm.states,
                alphabet: fsm.alphabet,
            },
            starts: buffer_from_slice(&ctx.device, &fsm.starts),
            outputs: buffer_from_slice(&ctx.device, &fsm.outputs),
            transitions: buffer_from_slice(&ctx.device, &fsm.transitions),
        },

        BatchPayload::Ca(ca) => PreparedPayload::Ca {
            params: CaParams {
                symbols: ca.symbols,
                two_r: ca.two_r,
                steps: ca.steps,
                rule_table_len: ca.rule_table_len,
            },
            rule_tables: buffer_from_slice(&ctx.device, &ca.rule_tables),
        },

        BatchPayload::Tm(tm) => {
            let gpu_transitions = tm_transition_pods(&tm.transitions);
            PreparedPayload::Tm {
                params: TmParams {
                    states: tm.states,
                    symbols: tm.symbols,
                    blank: tm.blank,
                    max_steps: tm.max_steps,
                    transitions_per_strategy: tm.states.saturating_mul(tm.symbols),
                },
                starts: buffer_from_slice(&ctx.device, &tm.start_states),
                transitions: buffer_from_slice(&ctx.device, &gpu_transitions),
            }
        }
    };

    Ok(Some(PreparedBatch {
        shader_key,
        eval_config: config.clone(),
        payload: prepared_payload,
    }))
}

/// Dispatches and synchronously waits for a prepared batch to complete.
pub fn try_evaluate_prepared_batch(
    prepared: &PreparedBatch,
    pairs: &[MatchPair],
) -> Result<Option<Vec<ScorePair>>, String> {
    if pairs.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let Some(pending) = try_begin_prepared_batch(prepared, pairs)? else {
        return Ok(None);
    };
    try_finish_prepared_batch(pending).map(Some)
}

/// Dispatches a TM batch and synchronously waits for halting results.
pub fn try_evaluate_prepared_tm_halting_batch(
    prepared: &PreparedBatch,
    pairs: &[MatchPair],
) -> Result<Option<Vec<TmHaltingPair>>, String> {
    if pairs.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let Some(pending) = try_begin_prepared_batch(prepared, pairs)? else {
        return Ok(None);
    };
    try_finish_prepared_tm_halting_batch(pending).map(Some)
}

/// Submits a prepared batch to the GPU without waiting for completion.
pub fn try_begin_prepared_batch(
    prepared: &PreparedBatch,
    pairs: &[MatchPair],
) -> Result<Option<PendingBatch>, String> {
    if pairs.is_empty() {
        return Ok(None);
    }
    let ctx = context_for_key(prepared.shader_key)?;
    dispatch_prepared(ctx, prepared, pairs).map(Some)
}

/// Waits for a pending batch to complete and reads back score results.
pub fn try_finish_prepared_batch(pending: PendingBatch) -> Result<Vec<ScorePair>, String> {
    pending.command_buffer.wait_until_completed();
    Ok(unsafe { extract_scores(&pending.score_output, pending.dispatched_pair_count) })
}

/// Waits for a pending TM batch and reads back halting flags.
pub fn try_finish_prepared_tm_halting_batch(
    pending: PendingBatch,
) -> Result<Vec<TmHaltingPair>, String> {
    pending.command_buffer.wait_until_completed();
    let halting_buf = pending
        .tm_halting_output
        .as_ref()
        .ok_or("TM halting results are only available for TM prepared batches")?;
    Ok(unsafe { extract_tm_halting(halting_buf, pending.dispatched_pair_count) })
}
