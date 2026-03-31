//! Public types shared across the Metal GPU acceleration crate.
//!
//! These types form the API boundary between the game-theory tournament
//! engine and the GPU backend. They are platform-agnostic: both the real
//! macOS Metal path and the non-macOS stubs operate on them.

/// A pair of strategy indices to be evaluated head-to-head on the GPU.
///
/// Each index refers to a position in the payload's strategy table
/// (start states for FSM/TM, rule table offset for CA).
#[derive(Clone, Debug)]
pub struct MatchPair {
    /// Index of the first (row) strategy.
    pub a_idx: u32,
    /// Index of the second (column) strategy.
    pub b_idx: u32,
}

/// Maximum cellular-automaton window size compiled into the default Metal kernel.
pub const CA_MAX_WINDOW: u32 = 1024;

// NOTE: This is the default scratch width compiled into the Metal TM batch kernel.
// The macOS Metal backend may compile specialized pipelines for larger TM widths at runtime.
pub const TM_MAX_WIDTH: u32 = 1024;

/// Default FSM state count for the cycle detection lookup table in the Metal FSM kernel.
/// The macOS Metal backend compiles specialized pipelines with the exact state count.
pub const FSM_MAX_STATES: u32 = 4;

/// Accumulated scores from a single match-pair evaluation.
#[derive(Clone, Debug)]
pub struct ScorePair {
    /// Total payoff accumulated by strategy A across all rounds.
    pub a_total: i64,
    /// Total payoff accumulated by strategy B across all rounds.
    pub b_total: i64,
}

/// Halting status for both sides of a Turing machine match pair.
///
/// Only meaningful for TM payloads; FSM and CA evaluations do not
/// produce halting information.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TmHaltingPair {
    /// Whether all TM instances for strategy A reached a halting state.
    pub a_all_halted: bool,
    /// Whether all TM instances for strategy B reached a halting state.
    pub b_all_halted: bool,
}

/// Parameters common to every batch evaluation request.
///
/// Bundles the game rules (rounds, payoff matrix, timeout scoring) with
/// the match pairs to be dispatched.
#[derive(Clone, Debug)]
pub struct EvalCommon {
    /// Number of iterated-game rounds per match.
    pub rounds: u32,
    /// 2x2x2 payoff tensor: `payoff[action_a][action_b][player]`.
    pub payoff: [[[i32; 2]; 2]; 2],
    /// Score assigned to a player that times out (loses the round).
    pub timeout_lose: i32,
    /// Score assigned to the opponent when a player times out (wins the round).
    pub timeout_win: i32,
    /// Strategy index pairs to evaluate.
    pub pairs: Vec<MatchPair>,
}

/// Evaluation configuration without match pairs.
///
/// Extracted from [`EvalCommon`] so that prepared batches can be
/// configured once and reused across multiple sets of pairs.
#[derive(Clone, Debug)]
pub struct BatchEvalConfig {
    /// Number of iterated-game rounds per match.
    pub rounds: u32,
    /// 2x2x2 payoff tensor: `payoff[action_a][action_b][player]`.
    pub payoff: [[[i32; 2]; 2]; 2],
    /// Score assigned to a player that times out.
    pub timeout_lose: i32,
    /// Score assigned to the opponent of a timed-out player.
    pub timeout_win: i32,
}

/// Finite state machine batch payload.
///
/// Encodes a population of deterministic finite automata for GPU evaluation.
#[derive(Clone, Debug)]
pub struct FsmBatch {
    /// Number of states per automaton.
    pub states: u32,
    /// Size of the input alphabet.
    pub alphabet: u32,
    /// Per-strategy initial state indices.
    pub starts: Vec<u32>,
    /// Flat output table: `output[strategy * states + state]`.
    pub outputs: Vec<u32>,
    /// Flat transition table: `transition[strategy * states * alphabet + state * alphabet + symbol]`.
    pub transitions: Vec<u32>,
}

/// Cellular automaton batch payload.
///
/// Encodes a population of 1-D totalistic cellular automata.
#[derive(Clone, Debug)]
pub struct CaBatch {
    /// Number of distinct cell symbols.
    pub symbols: u32,
    /// Twice the neighborhood radius (diameter minus one).
    pub two_r: u32,
    /// Simulation steps per evaluation.
    pub steps: u32,
    /// Length of a single strategy's rule table.
    pub rule_table_len: u32,
    /// Concatenated rule tables for all strategies.
    pub rule_tables: Vec<u32>,
}

/// A single Turing machine transition packed for GPU upload.
#[derive(Clone, Debug)]
pub struct TmTransitionPacked {
    /// Symbol to write on the tape.
    pub write: u32,
    /// Head movement direction (0 = left, 1 = right).
    pub move_dir: u32,
    /// Next state index.
    pub next: u32,
}

/// Turing machine batch payload.
///
/// Encodes a population of deterministic single-tape Turing machines.
#[derive(Clone, Debug)]
pub struct TmBatch {
    /// Number of internal states per machine.
    pub states: u32,
    /// Tape alphabet size (including blank).
    pub symbols: u32,
    /// Index of the blank symbol.
    pub blank: u32,
    /// Maximum simulation steps before forced halt.
    pub max_steps: u32,
    /// Per-strategy initial state indices.
    pub start_states: Vec<u32>,
    /// Flat transition table for all strategies.
    pub transitions: Vec<TmTransitionPacked>,
}

/// A batch evaluation payload — one of the three supported kernel types.
#[derive(Clone, Debug)]
pub enum BatchPayload {
    /// Finite state machine evaluation kernel.
    Fsm(FsmBatch),
    /// Cellular automaton evaluation kernel.
    Ca(CaBatch),
    /// Turing machine evaluation kernel.
    Tm(TmBatch),
}

/// A complete batch evaluation request ready for GPU dispatch.
#[derive(Clone, Debug)]
pub struct BatchRequest {
    /// Game configuration and match pairs.
    pub common: EvalCommon,
    /// Payload-specific strategy data.
    pub payload: BatchPayload,
}

/// How a [`RecommendedBatchPolicy`] was determined.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchPolicySource {
    /// Derived from device-tier heuristics without benchmarking.
    Heuristic,
    /// Loaded from the on-disk policy cache.
    Cached,
    /// Determined by a live GPU benchmark sweep.
    Benchmarked,
}

/// Tuning knobs for GPU batch dispatch throughput.
#[derive(Clone, Copy, Debug)]
pub struct BatchExecutionPolicy {
    /// Maximum match pairs per GPU command buffer.
    pub matches_per_batch: usize,
    /// Number of command buffers kept in flight concurrently.
    pub inflight_batches: usize,
}

/// The result of policy resolution: a concrete policy plus its provenance.
#[derive(Clone, Debug)]
pub struct RecommendedBatchPolicy {
    /// The resolved execution policy.
    pub policy: BatchExecutionPolicy,
    /// How this policy was determined (heuristic, cached, or benchmarked).
    pub source: BatchPolicySource,
    /// Cache key if the policy is cache-backed, `None` for heuristic-only.
    pub cache_key: Option<String>,
    /// Filesystem path to the cache entry, if available.
    pub cache_path: Option<String>,
}

/// Metadata for a single entry in the batch policy cache.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BatchPolicyCacheEntryInfo {
    /// Deterministic cache key derived from device name and payload signature.
    pub key: String,
    /// Filesystem path to the JSON cache file.
    pub path: String,
    /// GPU device name this entry was benchmarked on.
    pub device_name: String,
    /// Payload signature describing the workload shape.
    pub payload_signature: String,
    /// Benchmarked optimal matches per batch.
    pub matches_per_batch: usize,
    /// Benchmarked optimal inflight batch count.
    pub inflight_batches: usize,
}

/// A snapshot of all entries currently in the batch policy cache.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BatchPolicyCacheSnapshot {
    /// Root directory of the cache, if resolved.
    pub root: Option<String>,
    /// All valid cache entries found under the root.
    pub entries: Vec<BatchPolicyCacheEntryInfo>,
}
