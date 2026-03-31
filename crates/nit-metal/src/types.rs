//! Public types shared across the Metal GPU acceleration crate.

#[derive(Clone, Debug)]
pub struct MatchPair {
    pub a_idx: u32,
    pub b_idx: u32,
}

pub const CA_MAX_WINDOW: u32 = 1024;
// NOTE: This is the default scratch width compiled into the Metal TM batch kernel.
// The macOS Metal backend may compile specialized pipelines for larger TM widths at runtime.
pub const TM_MAX_WIDTH: u32 = 1024;
// Default FSM state count for the cycle detection lookup table in the Metal FSM kernel.
// The macOS Metal backend compiles specialized pipelines with the exact state count.
pub const FSM_MAX_STATES: u32 = 4;

#[derive(Clone, Debug)]
pub struct ScorePair {
    pub a_total: i64,
    pub b_total: i64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TmHaltingPair {
    pub a_all_halted: bool,
    pub b_all_halted: bool,
}

#[derive(Clone, Debug)]
pub struct EvalCommon {
    pub rounds: u32,
    pub payoff: [[[i32; 2]; 2]; 2],
    pub timeout_lose: i32,
    pub timeout_win: i32,
    pub pairs: Vec<MatchPair>,
}

#[derive(Clone, Debug)]
pub struct BatchEvalConfig {
    pub rounds: u32,
    pub payoff: [[[i32; 2]; 2]; 2],
    pub timeout_lose: i32,
    pub timeout_win: i32,
}

#[derive(Clone, Debug)]
pub struct FsmBatch {
    pub states: u32,
    pub alphabet: u32,
    pub starts: Vec<u32>,
    pub outputs: Vec<u32>,
    pub transitions: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct CaBatch {
    pub symbols: u32,
    pub two_r: u32,
    pub steps: u32,
    pub rule_table_len: u32,
    pub rule_tables: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct TmTransitionPacked {
    pub write: u32,
    pub move_dir: u32,
    pub next: u32,
}

#[derive(Clone, Debug)]
pub struct TmBatch {
    pub states: u32,
    pub symbols: u32,
    pub blank: u32,
    pub max_steps: u32,
    pub start_states: Vec<u32>,
    pub transitions: Vec<TmTransitionPacked>,
}

#[derive(Clone, Debug)]
pub enum BatchPayload {
    Fsm(FsmBatch),
    Ca(CaBatch),
    Tm(TmBatch),
}

#[derive(Clone, Debug)]
pub struct BatchRequest {
    pub common: EvalCommon,
    pub payload: BatchPayload,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchPolicySource {
    Heuristic,
    Cached,
    Benchmarked,
}

#[derive(Clone, Copy, Debug)]
pub struct BatchExecutionPolicy {
    pub matches_per_batch: usize,
    pub inflight_batches: usize,
}

#[derive(Clone, Debug)]
pub struct RecommendedBatchPolicy {
    pub policy: BatchExecutionPolicy,
    pub source: BatchPolicySource,
    pub cache_key: Option<String>,
    pub cache_path: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BatchPolicyCacheEntryInfo {
    pub key: String,
    pub path: String,
    pub device_name: String,
    pub payload_signature: String,
    pub matches_per_batch: usize,
    pub inflight_batches: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BatchPolicyCacheSnapshot {
    pub root: Option<String>,
    pub entries: Vec<BatchPolicyCacheEntryInfo>,
}
