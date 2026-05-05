//! Metal batch dispatch: public API, POD mirrors for the GPU-side structs,
//! and the per-kernel binding match.
//!
//! The numbered slot constants below MUST mirror the `[[buffer(N)]]`
//! attributes in `src/batch_eval.metal`; the Metal compiler cannot
//! cross-validate these bindings. POD field order, stride, and trailing
//! padding likewise stay in lock-step with the Metal-side layouts.

use super::shader::{context_for_key, MetalContext, ShaderKey};
use super::MetalResult;
use crate::{
    BatchEvalConfig, BatchPayload, CaBatch, FsmBatch, MatchPair, ScorePair, TmBatch, TmHaltingPair,
    TmTransitionPacked,
};
use metal::{MTLCommandBufferStatus, MTLResourceOptions, MTLSize};
use std::ffi::c_void;
use std::mem::{size_of, size_of_val};
use std::slice;

const PAIRS_SLOT: u64 = 0;

mod fsm_slot {
    pub const STARTS: u64 = 1;
    pub const OUTPUTS: u64 = 2;
    pub const TRANSITIONS: u64 = 3;
    pub const SCORES: u64 = 4;
    pub const EVAL_PARAMS: u64 = 5;
    pub const FSM_PARAMS: u64 = 6;
}

mod ca_slot {
    pub const RULE_TABLES: u64 = 1;
    pub const SCORES: u64 = 2;
    pub const EVAL_PARAMS: u64 = 3;
    pub const CA_PARAMS: u64 = 4;
}

mod tm_slot {
    pub const START_STATES: u64 = 1;
    pub const TRANSITIONS: u64 = 2;
    pub const SCORES: u64 = 3;
    pub const EVAL_PARAMS: u64 = 4;
    pub const TM_PARAMS: u64 = 5;
    pub const HALTING: u64 = 6;
}

/// Shared per-dispatch parameters (mirrored as `EvalParams` in batch_eval.metal).
/// The payoff matrix is flattened into 8 i32s so the Metal side can read it
/// with a single aligned load.
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

impl EvalParams {
    fn from_config(config: &BatchEvalConfig, pair_count: usize) -> Self {
        let [[[cc_a, cc_b], [cd_a, cd_b]], [[dc_a, dc_b], [dd_a, dd_b]]] = config.payoff;
        Self {
            rounds: config.rounds,
            pair_count: pair_count as u32,
            cc_a,
            cc_b,
            cd_a,
            cd_b,
            dc_a,
            dc_b,
            dd_a,
            dd_b,
            timeout_lose: config.timeout_lose,
            timeout_win: config.timeout_win,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
struct FsmParams {
    states: u32,
    alphabet: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct CaParams {
    symbols: u32,
    two_r: u32,
    steps: u32,
    rule_table_len: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct TmParams {
    states: u32,
    symbols: u32,
    blank: u32,
    max_steps: u32,
    transitions_per_strategy: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct MatchPairPod {
    a_idx: u32,
    b_idx: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct ScorePairPod {
    a_total: i64,
    b_total: i64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct TmHaltingPairPod {
    a_all_halted: u32,
    b_all_halted: u32,
}

/// Explicit `_pad` keeps the layout aligned with the Metal-side `TmTransition`
/// struct; the Metal compiler inserts the same 4-byte tail automatically.
#[repr(C)]
#[derive(Copy, Clone)]
struct TmTransitionPod {
    write: u32,
    move_dir: u32,
    next: u32,
    _pad: u32,
}

impl From<&TmTransitionPacked> for TmTransitionPod {
    fn from(tr: &TmTransitionPacked) -> Self {
        Self {
            write: tr.write,
            move_dir: tr.move_dir,
            next: tr.next,
            _pad: 0,
        }
    }
}

/// Used by the policy layer to turn working-set size into a pair count budget.
pub(super) fn bytes_per_match_pair() -> usize {
    size_of::<MatchPairPod>() + size_of::<ScorePairPod>()
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

fn tm_transition_pods(source: &[TmTransitionPacked]) -> Vec<TmTransitionPod> {
    source.iter().map(TmTransitionPod::from).collect()
}

/// # Safety
/// Buffer must contain at least `count` valid `ScorePairPod` values, and the
/// command buffer that wrote them must have completed execution.
/// StorageModeShared means the slice aliases GPU memory directly, so the
/// buffer must outlive the returned [`Vec`]'s construction.
unsafe fn extract_scores(buffer: &metal::BufferRef, count: usize) -> Vec<ScorePair> {
    if count == 0 {
        return Vec::new();
    }
    let raw = buffer.contents() as *const ScorePairPod;
    slice::from_raw_parts(raw, count)
        .iter()
        .map(|pod| ScorePair {
            a_total: pod.a_total,
            b_total: pod.b_total,
        })
        .collect()
}

/// # Safety
/// Buffer must contain at least `count` valid `TmHaltingPairPod` values, and
/// the command buffer that wrote them must have completed execution.
unsafe fn extract_tm_halting(buffer: &metal::BufferRef, count: usize) -> Vec<TmHaltingPair> {
    if count == 0 {
        return Vec::new();
    }
    let raw = buffer.contents() as *const TmHaltingPairPod;
    slice::from_raw_parts(raw, count)
        .iter()
        .map(|pod| TmHaltingPair {
            a_all_halted: pod.a_all_halted != 0,
            b_all_halted: pod.b_all_halted != 0,
        })
        .collect()
}

fn buffer_from_slice<T>(device: &metal::Device, data: &[T]) -> metal::Buffer {
    if data.is_empty() {
        return device.new_buffer(1, MTLResourceOptions::StorageModeShared);
    }
    device.new_buffer_with_data(
        data.as_ptr() as *const c_void,
        size_of_val(data) as u64,
        MTLResourceOptions::StorageModeShared,
    )
}

fn allocate_output_buffer<T>(device: &metal::Device, count: usize) -> metal::Buffer {
    device.new_buffer(
        (count.max(1) * size_of::<T>()) as u64,
        MTLResourceOptions::StorageModeShared,
    )
}

fn encode_and_commit(
    pipeline: &metal::ComputePipelineState,
    queue: &metal::CommandQueue,
    bind: impl FnOnce(&metal::ComputeCommandEncoderRef),
    thread_count: usize,
) -> metal::CommandBuffer {
    let cmd = queue.new_command_buffer();
    let enc = cmd.new_compute_command_encoder();
    enc.set_compute_pipeline_state(pipeline);
    bind(enc);

    let width = pipeline.thread_execution_width().max(1);
    let group = MTLSize {
        width,
        height: 1,
        depth: 1,
    };
    let grid = MTLSize {
        width: (thread_count as u64).div_ceil(width),
        height: 1,
        depth: 1,
    };
    enc.dispatch_thread_groups(grid, group);
    enc.end_encoding();
    cmd.commit();
    cmd.to_owned()
}

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
    fn upload(device: &metal::Device, payload: &BatchPayload) -> Self {
        match payload {
            BatchPayload::Fsm(fsm) => Self::upload_fsm(device, fsm),
            BatchPayload::Ca(ca) => Self::upload_ca(device, ca),
            BatchPayload::Tm(tm) => Self::upload_tm(device, tm),
        }
    }

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
}

/// Reusable across multiple dispatches with different match pair sets.
pub struct PreparedBatch {
    shader_key: ShaderKey,
    eval_config: BatchEvalConfig,
    payload: PreparedPayload,
}

/// Owns input/output buffers so they outlive the in-flight command buffer.
pub struct PendingBatch {
    _pair_input: metal::Buffer,
    score_output: metal::Buffer,
    tm_halting_output: Option<metal::Buffer>,
    command_buffer: metal::CommandBuffer,
    dispatched_pair_count: usize,
}

fn as_ptr<T>(v: &T) -> *const c_void {
    (v as *const T).cast()
}

fn dispatch_prepared(
    ctx: &MetalContext,
    prepared: &PreparedBatch,
    pairs: &[MatchPair],
) -> MetalResult<PendingBatch> {
    let pods = match_pair_pods(pairs);
    let thread_count = pods.len();
    let pair_buf = buffer_from_slice(&ctx.device, &pods);
    let score_buf = allocate_output_buffer::<ScorePairPod>(&ctx.device, thread_count);
    let eval = EvalParams::from_config(&prepared.eval_config, thread_count);

    let (cmd, halting_buf) = match &prepared.payload {
        PreparedPayload::Fsm {
            params,
            starts,
            outputs,
            transitions,
        } => {
            let cmd = encode_and_commit(
                &ctx.fsm_pipeline,
                &ctx.queue,
                |enc| {
                    enc.set_buffer(PAIRS_SLOT, Some(&pair_buf), 0);
                    enc.set_buffer(fsm_slot::STARTS, Some(starts), 0);
                    enc.set_buffer(fsm_slot::OUTPUTS, Some(outputs), 0);
                    enc.set_buffer(fsm_slot::TRANSITIONS, Some(transitions), 0);
                    enc.set_buffer(fsm_slot::SCORES, Some(&score_buf), 0);
                    enc.set_bytes(
                        fsm_slot::EVAL_PARAMS,
                        size_of::<EvalParams>() as u64,
                        as_ptr(&eval),
                    );
                    enc.set_bytes(
                        fsm_slot::FSM_PARAMS,
                        size_of::<FsmParams>() as u64,
                        as_ptr(params),
                    );
                },
                thread_count,
            );
            (cmd, None)
        }
        PreparedPayload::Ca {
            params,
            rule_tables,
        } => {
            let cmd = encode_and_commit(
                &ctx.ca_pipeline,
                &ctx.queue,
                |enc| {
                    enc.set_buffer(PAIRS_SLOT, Some(&pair_buf), 0);
                    enc.set_buffer(ca_slot::RULE_TABLES, Some(rule_tables), 0);
                    enc.set_buffer(ca_slot::SCORES, Some(&score_buf), 0);
                    enc.set_bytes(
                        ca_slot::EVAL_PARAMS,
                        size_of::<EvalParams>() as u64,
                        as_ptr(&eval),
                    );
                    enc.set_bytes(
                        ca_slot::CA_PARAMS,
                        size_of::<CaParams>() as u64,
                        as_ptr(params),
                    );
                },
                thread_count,
            );
            (cmd, None)
        }
        PreparedPayload::Tm {
            params,
            starts,
            transitions,
        } => {
            let halting = allocate_output_buffer::<TmHaltingPairPod>(&ctx.device, thread_count);
            let cmd = encode_and_commit(
                &ctx.tm_pipeline,
                &ctx.queue,
                |enc| {
                    enc.set_buffer(PAIRS_SLOT, Some(&pair_buf), 0);
                    enc.set_buffer(tm_slot::START_STATES, Some(starts), 0);
                    enc.set_buffer(tm_slot::TRANSITIONS, Some(transitions), 0);
                    enc.set_buffer(tm_slot::SCORES, Some(&score_buf), 0);
                    enc.set_buffer(tm_slot::HALTING, Some(&halting), 0);
                    enc.set_bytes(
                        tm_slot::EVAL_PARAMS,
                        size_of::<EvalParams>() as u64,
                        as_ptr(&eval),
                    );
                    enc.set_bytes(
                        tm_slot::TM_PARAMS,
                        size_of::<TmParams>() as u64,
                        as_ptr(params),
                    );
                },
                thread_count,
            );
            (cmd, Some(halting))
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

pub fn try_prepare_batch(
    config: &BatchEvalConfig,
    payload: &BatchPayload,
) -> MetalResult<Option<PreparedBatch>> {
    let shader_key = ShaderKey::for_payload(payload);
    let ctx = context_for_key(shader_key)?;
    Ok(Some(PreparedBatch {
        shader_key,
        eval_config: config.clone(),
        payload: PreparedPayload::upload(&ctx.device, payload),
    }))
}

pub fn try_begin_prepared_batch(
    prepared: &PreparedBatch,
    pairs: &[MatchPair],
) -> MetalResult<Option<PendingBatch>> {
    if pairs.is_empty() {
        return Ok(None);
    }
    let ctx = context_for_key(prepared.shader_key)?;
    dispatch_prepared(ctx, prepared, pairs).map(Some)
}

pub fn try_evaluate_prepared_batch(
    prepared: &PreparedBatch,
    pairs: &[MatchPair],
) -> MetalResult<Option<Vec<ScorePair>>> {
    if pairs.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let Some(pending) = try_begin_prepared_batch(prepared, pairs)? else {
        return Ok(None);
    };
    try_finish_prepared_batch(pending).map(Some)
}

pub fn try_finish_prepared_batch(pending: PendingBatch) -> MetalResult<Vec<ScorePair>> {
    pending.command_buffer.wait_until_completed();
    if pending.command_buffer.status() == MTLCommandBufferStatus::Error {
        return Err("Metal batch failed: command buffer reported error status".to_string());
    }
    Ok(unsafe { extract_scores(&pending.score_output, pending.dispatched_pair_count) })
}

pub fn try_finish_prepared_tm_halting_batch(
    pending: PendingBatch,
) -> MetalResult<Vec<TmHaltingPair>> {
    pending.command_buffer.wait_until_completed();
    if pending.command_buffer.status() == MTLCommandBufferStatus::Error {
        return Err("Metal batch failed: command buffer reported error status".to_string());
    }
    let halting = pending
        .tm_halting_output
        .as_ref()
        .ok_or("TM halting results are only available for TM prepared batches")?;
    Ok(unsafe { extract_tm_halting(halting, pending.dispatched_pair_count) })
}
