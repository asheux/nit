//! Metal GPU dispatch and batch execution for game-theory tournaments.

use crate::{
    BatchEvalConfig, BatchPayload, CaBatch, FsmBatch, MatchPair, ScorePair, TmBatch, TmHaltingPair,
    TmTransitionPacked,
};
use metal::{MTLResourceOptions, MTLSize};
use std::ffi::c_void;
use std::mem::{size_of, size_of_val};
use std::slice;

use super::shader::{context_for_key, MetalContext, ShaderKey};

/// GPU-side shared evaluation parameters.
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

/// GPU-side FSM kernel parameters.
#[repr(C)]
#[derive(Copy, Clone)]
struct FsmParams {
    states: u32,
    alphabet: u32,
}

/// GPU-side CA kernel parameters.
#[repr(C)]
#[derive(Copy, Clone)]
struct CaParams {
    symbols: u32,
    two_r: u32,
    steps: u32,
    rule_table_len: u32,
}

/// GPU-side TM kernel parameters.
#[repr(C)]
#[derive(Copy, Clone)]
struct TmParams {
    states: u32,
    symbols: u32,
    blank: u32,
    max_steps: u32,
    transitions_per_strategy: u32,
}

/// GPU-side match pair (mirrors Metal shader layout).
#[repr(C)]
#[derive(Copy, Clone)]
struct MatchPairPod {
    a_idx: u32,
    b_idx: u32,
}

/// GPU-side score accumulator (mirrors Metal shader layout).
#[repr(C)]
#[derive(Copy, Clone)]
struct ScorePairPod {
    a_total: i64,
    b_total: i64,
}

/// GPU-side TM halting flags (mirrors Metal shader layout).
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

impl PreparedPayload {
    fn upload_fsm(device: &metal::Device, fsm: &FsmBatch) -> Self {
        Self::Fsm {
            params: FsmParams {
                states: fsm.states,
                alphabet: fsm.alphabet,
            },
            starts: buffer_from_slice(device, &fsm.starts),
            outputs: buffer_from_slice(device, &fsm.outputs),
            transitions: buffer_from_slice(device, &fsm.transitions),
        }
    }

    fn upload_ca(device: &metal::Device, ca: &CaBatch) -> Self {
        Self::Ca {
            params: CaParams {
                symbols: ca.symbols,
                two_r: ca.two_r,
                steps: ca.steps,
                rule_table_len: ca.rule_table_len,
            },
            rule_tables: buffer_from_slice(device, &ca.rule_tables),
        }
    }

    /// Transitions are converted to padded GPU-side representation for Metal alignment.
    fn upload_tm(device: &metal::Device, tm: &TmBatch) -> Self {
        let gpu_transitions = tm_transition_pods(&tm.transitions);
        Self::Tm {
            params: TmParams {
                states: tm.states,
                symbols: tm.symbols,
                blank: tm.blank,
                max_steps: tm.max_steps,
                transitions_per_strategy: tm.states.saturating_mul(tm.symbols),
            },
            starts: buffer_from_slice(device, &tm.start_states),
            transitions: buffer_from_slice(device, &gpu_transitions),
        }
    }

    fn upload(device: &metal::Device, payload: &BatchPayload) -> Self {
        match payload {
            BatchPayload::Fsm(fsm) => Self::upload_fsm(device, fsm),
            BatchPayload::Ca(ca) => Self::upload_ca(device, ca),
            BatchPayload::Tm(tm) => Self::upload_tm(device, tm),
        }
    }
}

/// Reusable across multiple dispatches with different match pair sets.
pub struct PreparedBatch {
    shader_key: ShaderKey,
    eval_config: BatchEvalConfig,
    payload: PreparedPayload,
}

/// Owns input/output buffers to keep them alive until the GPU finishes.
pub struct PendingBatch {
    _pair_input: metal::Buffer,
    score_output: metal::Buffer,
    tm_halting_output: Option<metal::Buffer>,
    command_buffer: metal::CommandBuffer,
    dispatched_pair_count: usize,
}

pub(super) fn bytes_per_match_pair() -> usize {
    size_of::<MatchPairPod>() + size_of::<ScorePairPod>()
}

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

fn allocate_output_buffer<T>(device: &metal::Device, count: usize) -> metal::Buffer {
    device.new_buffer(
        (count.max(1) * size_of::<T>()) as u64,
        MTLResourceOptions::StorageModeShared,
    )
}

/// # Safety
/// Buffer must contain at least `count` valid `ScorePairPod` values
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

/// # Safety
/// Buffer must contain at least `count` valid `TmHaltingPairPod` values
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

fn as_void_ptr<T>(value: &T) -> *const c_void {
    (value as *const T).cast()
}

/// Encodes compute commands and commits the command buffer.
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

fn match_pair_pods(pairs: &[MatchPair]) -> Vec<MatchPairPod> {
    pairs
        .iter()
        .map(|mp| MatchPairPod {
            a_idx: mp.a_idx,
            b_idx: mp.b_idx,
        })
        .collect()
}

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

/// Binds pair input buffer (slot 0) and eval params at `eval_slot`.
fn bind_common_inputs(
    enc: &metal::ComputeCommandEncoderRef,
    pair_buf: &metal::Buffer,
    eval: &EvalParams,
    eval_slot: u64,
) {
    enc.set_buffer(0, Some(pair_buf), 0);
    enc.set_bytes(eval_slot, size_of::<EvalParams>() as u64, as_void_ptr(eval));
}

fn dispatch_prepared(
    ctx: &MetalContext,
    prepared: &PreparedBatch,
    pairs: &[MatchPair],
) -> Result<PendingBatch, String> {
    let pods = match_pair_pods(pairs);
    let pair_buf = buffer_from_slice(&ctx.device, &pods);
    let score_buf = allocate_output_buffer::<ScorePairPod>(&ctx.device, pods.len());
    let eval = build_eval_params(&prepared.eval_config, pods.len());
    let thread_count = pods.len();

    let (cmd, halting_buf) = match &prepared.payload {
        PreparedPayload::Fsm {
            params,
            starts,
            outputs,
            transitions,
        } => {
            let committed = encode_and_commit(
                &ctx.fsm_pipeline,
                &ctx.queue,
                |enc| {
                    bind_common_inputs(enc, &pair_buf, &eval, 5);
                    enc.set_buffer(1, Some(starts), 0);
                    enc.set_buffer(2, Some(outputs), 0);
                    enc.set_buffer(3, Some(transitions), 0);
                    enc.set_buffer(4, Some(&score_buf), 0);
                    enc.set_bytes(6, size_of::<FsmParams>() as u64, as_void_ptr(params));
                },
                thread_count,
            );
            (committed, None)
        }

        PreparedPayload::Ca {
            params,
            rule_tables,
        } => {
            let committed = encode_and_commit(
                &ctx.ca_pipeline,
                &ctx.queue,
                |enc| {
                    bind_common_inputs(enc, &pair_buf, &eval, 3);
                    enc.set_buffer(1, Some(rule_tables), 0);
                    enc.set_buffer(2, Some(&score_buf), 0);
                    enc.set_bytes(4, size_of::<CaParams>() as u64, as_void_ptr(params));
                },
                thread_count,
            );
            (committed, None)
        }

        PreparedPayload::Tm {
            params,
            starts,
            transitions,
        } => {
            let halting = allocate_output_buffer::<TmHaltingPairPod>(&ctx.device, thread_count);
            let committed = encode_and_commit(
                &ctx.tm_pipeline,
                &ctx.queue,
                |enc| {
                    bind_common_inputs(enc, &pair_buf, &eval, 4);
                    enc.set_buffer(1, Some(starts), 0);
                    enc.set_buffer(2, Some(transitions), 0);
                    enc.set_buffer(3, Some(&score_buf), 0);
                    enc.set_bytes(5, size_of::<TmParams>() as u64, as_void_ptr(params));
                    enc.set_buffer(6, Some(&halting), 0);
                },
                thread_count,
            );
            (committed, Some(halting))
        }
    };

    Ok(PendingBatch {
        _pair_input: pair_buf,
        score_output: score_buf,
        tm_halting_output: halting_buf,
        command_buffer: cmd,
        dispatched_pair_count: thread_count,
    })
}

pub fn try_evaluate_batch(request: &crate::BatchRequest) -> Result<Option<Vec<ScorePair>>, String> {
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

pub fn try_prepare_batch(
    config: &BatchEvalConfig,
    payload: &BatchPayload,
) -> Result<Option<PreparedBatch>, String> {
    let shader_key = ShaderKey::for_payload(payload);
    let ctx = context_for_key(shader_key)?;

    Ok(Some(PreparedBatch {
        shader_key,
        eval_config: config.clone(),
        payload: PreparedPayload::upload(&ctx.device, payload),
    }))
}

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

pub fn try_finish_prepared_batch(pending: PendingBatch) -> Result<Vec<ScorePair>, String> {
    pending.command_buffer.wait_until_completed();
    Ok(unsafe { extract_scores(&pending.score_output, pending.dispatched_pair_count) })
}

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
