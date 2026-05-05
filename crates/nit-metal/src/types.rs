//! Public types shared across the Metal GPU acceleration crate.

#[derive(Clone, Debug)]
pub struct MatchPair {
    pub a_idx: u32,
    pub b_idx: u32,
}

/// Default CA window compiled into the Metal kernel. The macOS backend may
/// compile specialized pipelines with a wider window at runtime.
pub const CA_MAX_WINDOW: u32 = 1024;

/// Default TM scratch width compiled into the Metal kernel. Wider tapes force
/// runtime recompilation of a specialized pipeline.
pub const TM_MAX_WIDTH: u32 = 1024;

/// Default FSM state count compiled into the Metal kernel. Larger state
/// machines trigger runtime recompilation of a specialized pipeline.
pub const FSM_MAX_STATES: u32 = 4;

#[derive(Clone, Debug)]
pub struct ScorePair {
    pub a_total: i64,
    pub b_total: i64,
}

/// Halting flags for both strategies of a TM match. Only meaningful for TM
/// payloads; FSM and CA evaluations do not produce halting information.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TmHaltingPair {
    pub a_all_halted: bool,
    pub b_all_halted: bool,
}

/// Evaluation configuration shared across every pair in a batch.
///
/// Intentionally excludes the match-pair list so a prepared batch can be
/// reused across multiple pair sets without re-uploading payload buffers.
#[derive(Clone, Debug)]
pub struct BatchEvalConfig {
    pub rounds: u32,
    /// `payoff[action_a][action_b][player]`.
    pub payoff: [[[i32; 2]; 2]; 2],
    /// Score assigned to a player that times out (loses the round).
    pub timeout_lose: i32,
    /// Score assigned to the opponent when a player times out.
    pub timeout_win: i32,
}

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

/// 1-D totalistic cellular automaton batch payload.
#[derive(Clone, Debug)]
pub struct CaBatch {
    pub symbols: u32,
    /// Twice the neighborhood radius (diameter minus one).
    pub two_r: u32,
    pub steps: u32,
    pub rule_table_len: u32,
    pub rule_tables: Vec<u32>,
}

/// A single TM transition packed with explicit fields for GPU upload.
#[derive(Clone, Debug)]
pub struct TmTransitionPacked {
    pub write: u32,
    /// 0 = left, 1 = right.
    pub move_dir: u32,
    pub next: u32,
}

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

/// One of the three supported kernel payloads.
#[derive(Clone, Debug)]
pub enum BatchPayload {
    Fsm(FsmBatch),
    Ca(CaBatch),
    Tm(TmBatch),
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

/// A resolved policy with its provenance and on-disk cache coordinates.
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

impl BatchPayload {
    /// Short label used in logging and cache key prefixes.
    pub fn variant_name(&self) -> &'static str {
        match self {
            Self::Fsm(_) => "fsm",
            Self::Ca(_) => "ca",
            Self::Tm(_) => "tm",
        }
    }

    /// Number of strategies encoded in this payload.
    ///
    /// Returns 0 for a CA payload with `rule_table_len == 0`; callers that
    /// divide by this value must guard against the zero case.
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

    /// Internal states (FSM/TM) or cell symbols (CA).
    pub fn state_dimension(&self) -> u32 {
        match self {
            Self::Fsm(fsm) => fsm.states,
            Self::Ca(ca) => ca.symbols,
            Self::Tm(tm) => tm.states,
        }
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
