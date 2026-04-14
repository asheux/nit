//! Public types shared across the Metal GPU acceleration crate.

/// A pair of strategy indices to be evaluated head-to-head on the GPU.
#[derive(Clone, Debug)]
pub struct MatchPair {
    pub a_idx: u32,
    pub b_idx: u32,
}

/// Maximum cellular-automaton window size compiled into the default Metal kernel.
pub const CA_MAX_WINDOW: u32 = 1024;

/// Default scratch width for the Metal TM kernel; the macOS backend may compile
/// specialized pipelines for larger widths at runtime.
pub const TM_MAX_WIDTH: u32 = 1024;

/// Default FSM state count for cycle detection in the Metal FSM kernel.
/// The macOS backend compiles specialized pipelines with the exact state count.
pub const FSM_MAX_STATES: u32 = 4;

/// Accumulated scores from a single match-pair evaluation.
#[derive(Clone, Debug)]
pub struct ScorePair {
    pub a_total: i64,
    pub b_total: i64,
}

/// Halting status for both sides of a Turing machine match pair.
///
/// Only meaningful for TM payloads; FSM and CA evaluations do not
/// produce halting information.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TmHaltingPair {
    pub a_all_halted: bool,
    pub b_all_halted: bool,
}

/// Parameters common to every batch evaluation request.
#[derive(Clone, Debug)]
pub struct EvalCommon {
    pub rounds: u32,
    /// `payoff[action_a][action_b][player]`.
    pub payoff: [[[i32; 2]; 2]; 2],
    /// Score assigned to a player that times out (loses the round).
    pub timeout_lose: i32,
    /// Score assigned to the opponent when a player times out.
    pub timeout_win: i32,
    pub pairs: Vec<MatchPair>,
}

/// Evaluation configuration without match pairs.
///
/// Extracted from [`EvalCommon`] so that prepared batches can be
/// configured once and reused across multiple sets of pairs.
#[derive(Clone, Debug)]
pub struct BatchEvalConfig {
    pub rounds: u32,
    /// `payoff[action_a][action_b][player]`.
    pub payoff: [[[i32; 2]; 2]; 2],
    pub timeout_lose: i32,
    pub timeout_win: i32,
}

/// Finite state machine batch payload.
#[derive(Clone, Debug)]
pub struct FsmBatch {
    pub states: u32,
    pub alphabet: u32,
    pub starts: Vec<u32>,
    /// `output[strategy * states + state]`.
    pub outputs: Vec<u32>,
    /// `transition[strategy * states * alphabet + state * alphabet + symbol]`.
    pub transitions: Vec<u32>,
}

/// Cellular automaton batch payload (1-D totalistic).
#[derive(Clone, Debug)]
pub struct CaBatch {
    pub symbols: u32,
    /// Twice the neighborhood radius (diameter minus one).
    pub two_r: u32,
    pub steps: u32,
    pub rule_table_len: u32,
    pub rule_tables: Vec<u32>,
}

/// A single Turing machine transition packed for GPU upload.
#[derive(Clone, Debug)]
pub struct TmTransitionPacked {
    pub write: u32,
    /// 0 = left, 1 = right.
    pub move_dir: u32,
    pub next: u32,
}

/// Turing machine batch payload.
#[derive(Clone, Debug)]
pub struct TmBatch {
    pub states: u32,
    /// Tape alphabet size (including blank).
    pub symbols: u32,
    pub blank: u32,
    pub max_steps: u32,
    pub start_states: Vec<u32>,
    pub transitions: Vec<TmTransitionPacked>,
}

/// A batch evaluation payload — one of the three supported kernel types.
#[derive(Clone, Debug)]
pub enum BatchPayload {
    Fsm(FsmBatch),
    Ca(CaBatch),
    Tm(TmBatch),
}

/// A complete batch evaluation request ready for GPU dispatch.
#[derive(Clone, Debug)]
pub struct BatchRequest {
    pub common: EvalCommon,
    pub payload: BatchPayload,
}

/// How a [`RecommendedBatchPolicy`] was determined.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchPolicySource {
    Heuristic,
    Cached,
    Benchmarked,
}

/// Tuning knobs for GPU batch dispatch throughput.
#[derive(Clone, Copy, Debug)]
pub struct BatchExecutionPolicy {
    pub matches_per_batch: usize,
    pub inflight_batches: usize,
}

/// The result of policy resolution: a concrete policy plus its provenance.
#[derive(Clone, Debug)]
pub struct RecommendedBatchPolicy {
    pub policy: BatchExecutionPolicy,
    pub source: BatchPolicySource,
    pub cache_key: Option<String>,
    pub cache_path: Option<String>,
}

/// Metadata for a single entry in the batch policy cache.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BatchPolicyCacheEntryInfo {
    pub key: String,
    pub path: String,
    pub device_name: String,
    pub payload_signature: String,
    pub matches_per_batch: usize,
    pub inflight_batches: usize,
}

/// A snapshot of all entries currently in the batch policy cache.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BatchPolicyCacheSnapshot {
    pub root: Option<String>,
    pub entries: Vec<BatchPolicyCacheEntryInfo>,
}

impl BatchPayload {
    /// Kernel variant name used in logging and cache key prefixes.
    pub fn variant_name(&self) -> &'static str {
        match self {
            Self::Fsm(_) => "fsm",
            Self::Ca(_) => "ca",
            Self::Tm(_) => "tm",
        }
    }

    /// Number of individual strategies (automata) encoded in this payload.
    pub fn population_count(&self) -> usize {
        match self {
            Self::Fsm(fsm) => fsm.starts.len(),
            Self::Ca(ca) => {
                let stride = ca.rule_table_len as usize;
                if stride == 0 {
                    return 0;
                }
                ca.rule_tables.len() / stride
            }
            Self::Tm(tm) => tm.start_states.len(),
        }
    }

    /// State-space dimension: internal states (FSM/TM) or cell symbols (CA).
    pub fn state_dimension(&self) -> u32 {
        match self {
            Self::Fsm(fsm) => fsm.states,
            Self::Ca(ca) => ca.symbols,
            Self::Tm(tm) => tm.states,
        }
    }
}

impl BatchRequest {
    pub fn pair_count(&self) -> usize {
        self.common.pairs.len()
    }

    pub fn kernel_variant(&self) -> &'static str {
        self.payload.variant_name()
    }
}

impl BatchExecutionPolicy {
    /// Saturates rather than wrapping on overflow.
    pub fn total_inflight_matches(&self) -> usize {
        self.matches_per_batch.saturating_mul(self.inflight_batches)
    }
}

impl BatchPolicyCacheSnapshot {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
