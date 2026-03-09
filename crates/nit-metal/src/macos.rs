use crate::{
    BatchPayload, BatchRequest, CaBatch, EvalCommon, FsmBatch, MatchPair, ScorePair, TmBatch,
    TmTransitionPacked, CA_MAX_WINDOW, TM_MAX_WIDTH,
};
use metal::{CompileOptions, ComputePipelineState, Device, Library, MTLResourceOptions, MTLSize};
use std::ffi::c_void;
use std::mem::size_of;
use std::slice;
use std::sync::OnceLock;

const SHADER_SOURCE: &str = include_str!("batch_eval.metal");

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
struct TmTransitionPod {
    write: u32,
    move_dir: u32,
    next: u32,
    pad: u32,
}

struct MetalContext {
    device: Device,
    queue: metal::CommandQueue,
    _library: Library,
    fsm_pipeline: ComputePipelineState,
    ca_pipeline: ComputePipelineState,
    tm_pipeline: ComputePipelineState,
}

impl MetalContext {
    fn new() -> Result<Self, String> {
        let device =
            Device::system_default().ok_or_else(|| "Metal device unavailable".to_string())?;
        let options = CompileOptions::new();
        let library = device.new_library_with_source(SHADER_SOURCE, &options)?;
        let fsm_fn = library
            .get_function("fsm_batch", None)
            .map_err(|err| err.to_string())?;
        let ca_fn = library
            .get_function("ca_batch", None)
            .map_err(|err| err.to_string())?;
        let tm_fn = library
            .get_function("tm_batch", None)
            .map_err(|err| err.to_string())?;
        let fsm_pipeline = device.new_compute_pipeline_state_with_function(&fsm_fn)?;
        let ca_pipeline = device.new_compute_pipeline_state_with_function(&ca_fn)?;
        let tm_pipeline = device.new_compute_pipeline_state_with_function(&tm_fn)?;
        let queue = device.new_command_queue();
        Ok(Self {
            device,
            queue,
            _library: library,
            fsm_pipeline,
            ca_pipeline,
            tm_pipeline,
        })
    }
}

fn context() -> Result<&'static MetalContext, String> {
    static CONTEXT: OnceLock<Result<MetalContext, String>> = OnceLock::new();
    CONTEXT
        .get_or_init(MetalContext::new)
        .as_ref()
        .map_err(|err| err.clone())
}

fn eval_params(common: &EvalCommon) -> EvalParams {
    EvalParams {
        rounds: common.rounds,
        pair_count: common.pairs.len() as u32,
        cc_a: common.payoff[0][0][0],
        cc_b: common.payoff[0][0][1],
        cd_a: common.payoff[0][1][0],
        cd_b: common.payoff[0][1][1],
        dc_a: common.payoff[1][0][0],
        dc_b: common.payoff[1][0][1],
        dd_a: common.payoff[1][1][0],
        dd_b: common.payoff[1][1][1],
        timeout_lose: common.timeout_lose,
        timeout_win: common.timeout_win,
    }
}

fn buffer_from_slice<T>(device: &Device, slice: &[T]) -> metal::Buffer {
    if slice.is_empty() {
        return device.new_buffer(1, MTLResourceOptions::StorageModeShared);
    }
    let len = (slice.len() * size_of::<T>()) as u64;
    device.new_buffer_with_data(
        slice.as_ptr() as *const c_void,
        len,
        MTLResourceOptions::StorageModeShared,
    )
}

fn empty_output_buffer<T>(device: &Device, len: usize) -> metal::Buffer {
    device.new_buffer(
        (len.max(1) * size_of::<T>()) as u64,
        MTLResourceOptions::StorageModeShared,
    )
}

unsafe fn read_scores(buffer: &metal::BufferRef, len: usize) -> Vec<ScorePair> {
    let ptr = buffer.contents() as *const ScorePairPod;
    let slice = slice::from_raw_parts(ptr, len);
    slice
        .iter()
        .map(|score| ScorePair {
            a_total: score.a_total,
            b_total: score.b_total,
        })
        .collect()
}

fn dispatch(
    pipeline: &ComputePipelineState,
    queue: &metal::CommandQueue,
    encode: impl FnOnce(&metal::ComputeCommandEncoderRef),
    pair_count: usize,
) {
    let command_buffer = queue.new_command_buffer();
    let encoder = command_buffer.new_compute_command_encoder();
    encoder.set_compute_pipeline_state(pipeline);
    encode(&encoder);
    let width = pipeline.thread_execution_width().max(1);
    let threads_per_group = MTLSize {
        width,
        height: 1,
        depth: 1,
    };
    let group_count = MTLSize {
        width: ((pair_count as u64) + width - 1) / width,
        height: 1,
        depth: 1,
    };
    encoder.dispatch_thread_groups(group_count, threads_per_group);
    encoder.end_encoding();
    command_buffer.commit();
    command_buffer.wait_until_completed();
}

fn pair_pods(pairs: &[MatchPair]) -> Vec<MatchPairPod> {
    pairs
        .iter()
        .map(|pair| MatchPairPod {
            a_idx: pair.a_idx,
            b_idx: pair.b_idx,
        })
        .collect()
}

fn tm_pods(transitions: &[TmTransitionPacked]) -> Vec<TmTransitionPod> {
    transitions
        .iter()
        .map(|trans| TmTransitionPod {
            write: trans.write,
            move_dir: trans.move_dir,
            next: trans.next,
            pad: 0,
        })
        .collect()
}

fn evaluate_fsm(
    ctx: &MetalContext,
    common: &EvalCommon,
    payload: &FsmBatch,
) -> Result<Vec<ScorePair>, String> {
    let pair_pods = pair_pods(&common.pairs);
    let pair_buffer = buffer_from_slice(&ctx.device, &pair_pods);
    let starts = buffer_from_slice(&ctx.device, &payload.starts);
    let outputs = buffer_from_slice(&ctx.device, &payload.outputs);
    let transitions = buffer_from_slice(&ctx.device, &payload.transitions);
    let scores = empty_output_buffer::<ScorePairPod>(&ctx.device, pair_pods.len());
    let eval = eval_params(common);
    let fsm = FsmParams {
        states: payload.states,
        alphabet: payload.alphabet,
    };
    dispatch(
        &ctx.fsm_pipeline,
        &ctx.queue,
        |encoder| {
            encoder.set_buffer(0, Some(&pair_buffer), 0);
            encoder.set_buffer(1, Some(&starts), 0);
            encoder.set_buffer(2, Some(&outputs), 0);
            encoder.set_buffer(3, Some(&transitions), 0);
            encoder.set_buffer(4, Some(&scores), 0);
            encoder.set_bytes(
                5,
                size_of::<EvalParams>() as u64,
                (&eval as *const EvalParams).cast(),
            );
            encoder.set_bytes(
                6,
                size_of::<FsmParams>() as u64,
                (&fsm as *const FsmParams).cast(),
            );
        },
        pair_pods.len(),
    );
    let out = unsafe { read_scores(&scores, pair_pods.len()) };
    Ok(out)
}

fn evaluate_ca(
    ctx: &MetalContext,
    common: &EvalCommon,
    payload: &CaBatch,
) -> Result<Option<Vec<ScorePair>>, String> {
    if payload
        .two_r
        .saturating_mul(payload.steps)
        .saturating_add(1)
        > CA_MAX_WINDOW
    {
        return Ok(None);
    }
    let pair_pods = pair_pods(&common.pairs);
    let pair_buffer = buffer_from_slice(&ctx.device, &pair_pods);
    let rule_tables = buffer_from_slice(&ctx.device, &payload.rule_tables);
    let scores = empty_output_buffer::<ScorePairPod>(&ctx.device, pair_pods.len());
    let eval = eval_params(common);
    let ca = CaParams {
        symbols: payload.symbols,
        two_r: payload.two_r,
        steps: payload.steps,
        rule_table_len: payload.rule_table_len,
    };
    dispatch(
        &ctx.ca_pipeline,
        &ctx.queue,
        |encoder| {
            encoder.set_buffer(0, Some(&pair_buffer), 0);
            encoder.set_buffer(1, Some(&rule_tables), 0);
            encoder.set_buffer(2, Some(&scores), 0);
            encoder.set_bytes(
                3,
                size_of::<EvalParams>() as u64,
                (&eval as *const EvalParams).cast(),
            );
            encoder.set_bytes(
                4,
                size_of::<CaParams>() as u64,
                (&ca as *const CaParams).cast(),
            );
        },
        pair_pods.len(),
    );
    let out = unsafe { read_scores(&scores, pair_pods.len()) };
    Ok(Some(out))
}

fn evaluate_tm(
    ctx: &MetalContext,
    common: &EvalCommon,
    payload: &TmBatch,
) -> Result<Option<Vec<ScorePair>>, String> {
    if payload.max_steps.saturating_add(1) > TM_MAX_WIDTH {
        return Ok(None);
    }
    let pair_pods = pair_pods(&common.pairs);
    let pair_buffer = buffer_from_slice(&ctx.device, &pair_pods);
    let starts = buffer_from_slice(&ctx.device, &payload.start_states);
    let transitions = tm_pods(&payload.transitions);
    let transitions_buffer = buffer_from_slice(&ctx.device, &transitions);
    let scores = empty_output_buffer::<ScorePairPod>(&ctx.device, pair_pods.len());
    let eval = eval_params(common);
    let tm = TmParams {
        states: payload.states,
        symbols: payload.symbols,
        blank: payload.blank,
        max_steps: payload.max_steps,
        transitions_per_strategy: payload.states.saturating_mul(payload.symbols),
    };
    dispatch(
        &ctx.tm_pipeline,
        &ctx.queue,
        |encoder| {
            encoder.set_buffer(0, Some(&pair_buffer), 0);
            encoder.set_buffer(1, Some(&starts), 0);
            encoder.set_buffer(2, Some(&transitions_buffer), 0);
            encoder.set_buffer(3, Some(&scores), 0);
            encoder.set_bytes(
                4,
                size_of::<EvalParams>() as u64,
                (&eval as *const EvalParams).cast(),
            );
            encoder.set_bytes(
                5,
                size_of::<TmParams>() as u64,
                (&tm as *const TmParams).cast(),
            );
        },
        pair_pods.len(),
    );
    let out = unsafe { read_scores(&scores, pair_pods.len()) };
    Ok(Some(out))
}

pub fn try_evaluate_batch(request: &BatchRequest) -> Result<Option<Vec<ScorePair>>, String> {
    if request.common.pairs.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let ctx = context()?;
    match &request.payload {
        BatchPayload::Fsm(payload) => evaluate_fsm(ctx, &request.common, payload).map(Some),
        BatchPayload::Ca(payload) => evaluate_ca(ctx, &request.common, payload),
        BatchPayload::Tm(payload) => evaluate_tm(ctx, &request.common, payload),
    }
}
