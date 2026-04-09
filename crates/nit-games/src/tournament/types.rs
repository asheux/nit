//! Internal types shared across tournament submodules.
//!
//! Contains match scheduling types, parallelism helpers, progress tracking,
//! and the accumulator that aggregates results across matches.

use crate::config::{AcceleratorMode, ParallelismConfig, ParallelismMode, ScoreAggregation};
use crate::game::{Action, Outcome};
use crate::history::History;
use crate::output::RuntimeAcceleratorStats;
use crate::strategy::{Strategy, TmRunStats};
use nit_metal::{BatchExecutionPolicy, BatchPolicySource, PreparedBatch};
use nit_utils::hashing::{stable_hash_bytes, SplitMix64};
use rayon::ThreadPoolBuilder;
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ── Tournament progress ─────────────────────────────────────

/// Snapshot of tournament execution state, used to drive the TUI progress display.
///
/// Built by [`TournamentRunner`](super::runner::TournamentRunner) after each round
/// or match completion to give the TUI enough context to render the live scoreboard.
#[derive(Clone, Debug)]
pub struct TournamentProgress {
    /// One-based index of the current or just-completed match.
    pub match_index: usize,
    /// Total number of matches in the schedule.
    pub total_matches: usize,
    /// Current round number within the active match.
    pub round: u32,
    /// Total rounds configured for each match.
    pub rounds: u32,
    /// `true` when the match has finished all its rounds.
    pub match_complete: bool,
    /// Display identifier for strategy A.
    pub a: String,
    /// Display identifier for strategy B.
    pub b: String,
    /// Cumulative payoff for strategy A across rounds played so far.
    pub total_payoff_a: i64,
    /// Cumulative payoff for strategy B across rounds played so far.
    pub total_payoff_b: i64,
    /// Last-round action chosen by strategy A, if any round has been played.
    pub last_action_a: Option<Action>,
    /// Last-round action chosen by strategy B, if any round has been played.
    pub last_action_b: Option<Action>,
    /// Last-round payoff awarded to strategy A.
    pub last_payoff_a: Option<i32>,
    /// Last-round payoff awarded to strategy B.
    pub last_payoff_b: Option<i32>,
    /// Whether strategy A halted on the last round (TM strategies only).
    pub last_halted_a: Option<bool>,
    /// Whether strategy B halted on the last round (TM strategies only).
    pub last_halted_b: Option<bool>,
    /// Derived outcome (CC/CD/DC/DD) of the last round.
    pub last_outcome: Option<Outcome>,
    /// Runtime accelerator statistics accumulated so far.
    pub runtime: RuntimeAcceleratorStats,
}

impl TournamentProgress {
    /// Construct a progress snapshot from scheduling context, strategy IDs,
    /// cumulative scores, an optional round snapshot, and runtime stats.
    ///
    /// Centralises the construction pattern that was previously repeated at
    /// every call site in the tournament runner.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn build(
        match_index: usize,
        total_matches: usize,
        current_round: u32,
        total_rounds: u32,
        match_complete: bool,
        strategy_a: String,
        strategy_b: String,
        cumulative_payoff_a: i64,
        cumulative_payoff_b: i64,
        last_round: Option<&RoundSnapshot>,
        runtime: RuntimeAcceleratorStats,
    ) -> Self {
        Self {
            match_index,
            total_matches,
            round: current_round,
            rounds: total_rounds,
            match_complete,
            a: strategy_a,
            b: strategy_b,
            total_payoff_a: cumulative_payoff_a,
            total_payoff_b: cumulative_payoff_b,
            last_action_a: last_round.map(|r| r.a_action),
            last_action_b: last_round.map(|r| r.b_action),
            last_payoff_a: last_round.map(|r| r.a_payoff),
            last_payoff_b: last_round.map(|r| r.b_payoff),
            last_halted_a: last_round.map(|r| r.a_halted),
            last_halted_b: last_round.map(|r| r.b_halted),
            last_outcome: last_round.map(|r| Outcome::from_actions(r.a_action, r.b_action)),
            runtime,
        }
    }
}

// ── Match snapshot ──────────────────────────────────────────

/// Full snapshot of a match in progress, including the outcome history buffer.
///
/// Captures the complete trace of a match for the TUI detail view, including
/// per-round outcome characters, payoff pairs, and halting flags.
#[derive(Clone, Debug)]
pub struct MatchSnapshot {
    /// One-based index of this match within the tournament.
    pub match_index: usize,
    /// Total matches in the tournament schedule.
    pub total_matches: usize,
    /// Current round number (zero-based count of rounds played so far).
    pub round: u32,
    /// Total rounds configured for this match.
    pub rounds: u32,
    /// Display identifier for strategy A.
    pub a: String,
    /// Display identifier for strategy B.
    pub b: String,
    /// Cumulative payoff for strategy A.
    pub a_score: i64,
    /// Cumulative payoff for strategy B.
    pub b_score: i64,
    /// Encoded outcome history: one digit char (`'0'`..`'3'`) per round.
    pub outcomes: String,
    /// Per-round payoff pairs `[a_payoff, b_payoff]`.
    pub payoffs: Vec<[i32; 2]>,
    /// Per-round halting flags for strategy A (`'0'` or `'1'` per round).
    pub a_halted: String,
    /// Per-round halting flags for strategy B (`'0'` or `'1'` per round).
    pub b_halted: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchHistoryPreview {
    pub match_index: usize,
    pub total_matches: usize,
    pub a: String,
    pub b: String,
    pub rounds_total: u32,
    #[serde(alias = "outcomes_prefix")]
    pub outcomes: String,
}

impl MatchHistoryPreview {
    /// Maximum rounds shown in the TUI preview widget.
    pub const DISPLAY_ROUND_CAP: usize = 500;

    pub fn preview_rounds(&self) -> usize {
        self.outcomes.len().min(Self::DISPLAY_ROUND_CAP)
    }

    pub fn preview_outcomes(&self) -> &str {
        let end = self.preview_rounds();
        self.outcomes.get(..end).unwrap_or(self.outcomes.as_str())
    }
}

/// Outcome of a single completed match: strategy indices, raw and adjusted scores.
///
/// The `adjusted_total` fields incorporate complexity-cost penalties when enabled;
/// otherwise they equal the raw totals cast to `f64`.
#[derive(Clone, Debug)]
pub struct MatchResult {
    pub a_idx: usize,
    pub b_idx: usize,
    pub rounds: u32,
    pub a_total: i64,
    pub b_total: i64,
    pub a_adjusted_total: f64,
    pub b_adjusted_total: f64,
    pub repetition: u32,
    pub match_id: usize,
}

// ── Seed derivation ─────────────────────────────────────────

/// Which side of a match a strategy occupies (first-mover vs second-mover).
#[derive(Copy, Clone, Debug)]
pub(super) enum MatchRole {
    A,
    B,
}

impl MatchRole {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
}

/// Deterministic seed derivation from a tournament-level seed.
///
/// Each match gets unique per-strategy and per-noise seeds, derived
/// from the run seed, strategy id, match id, and repetition index.
#[derive(Clone, Debug)]
pub(super) struct SeedDeriver {
    pub(super) run_seed: u64,
    pub(super) noise_base: u64,
}

impl SeedDeriver {
    pub(super) fn new(run_seed: u64) -> Self {
        let noise_base = stable_hash_bytes(format!("{run_seed}:noise").as_bytes());
        Self {
            run_seed,
            noise_base,
        }
    }

    // Base seed per strategy role; per-match seeds derive from this plus match_id/repetition.
    pub(super) fn base_strategy_seed(&self, role: MatchRole, strategy_id: &str) -> u64 {
        stable_hash_bytes(format!("{}:{}:{}", self.run_seed, role.label(), strategy_id).as_bytes())
    }

    pub(super) fn strategy_seed(
        &self,
        match_id: usize,
        repetition: u32,
        role: MatchRole,
        strategy_id: &str,
    ) -> u64 {
        let base = self.base_strategy_seed(role, strategy_id);
        stable_hash_bytes(format!("{base}:{match_id}:{repetition}").as_bytes())
    }

    pub(super) fn noise_seed(&self, match_id: usize, repetition: u32) -> u64 {
        stable_hash_bytes(format!("{}:{match_id}:{repetition}", self.noise_base).as_bytes())
    }
}

// ── Parallelism ─────────────────────────────────────────────

/// Controls how matches are distributed across threads during tournament execution.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Parallelism {
    /// Use the global Rayon thread pool.
    Auto,
    /// Run everything on the calling thread.
    Off,
    /// Spawn a dedicated pool with the given thread count.
    Threads(usize),
}

impl Parallelism {
    /// Derive parallelism settings from the user-facing config enum.
    pub fn from_config(config: &ParallelismConfig) -> Self {
        match config {
            ParallelismConfig::Mode(mode) => match mode {
                ParallelismMode::Auto => Self::Auto,
                ParallelismMode::Off => Self::Off,
            },
            ParallelismConfig::Threads { threads } => Self::Threads(*threads),
        }
    }
}

/// Execute a closure on a Rayon pool governed by the [`Parallelism`] setting.
///
/// When `Parallelism::Threads(n)` is set, a dedicated pool is built with `n`
/// threads. All other variants (`Auto`, `Off`) run the closure directly — `Auto`
/// relies on the global Rayon pool, while `Off` executes sequentially.
pub(super) fn run_with_parallelism<T: Send>(
    parallelism: Parallelism,
    f: impl FnOnce() -> T + Send,
) -> T {
    match parallelism {
        Parallelism::Threads(threads) if threads > 0 => {
            let pool = ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap_or_else(|_| ThreadPoolBuilder::new().build().expect("thread pool"));
            pool.install(f)
        }
        _ => f(),
    }
}

// ── TM halting filter ───────────────────────────────────────

/// Identifies which backend was used for TM halting analysis.
///
/// Reported in [`TmHaltingFilterDiagnostics`] so the caller can see which code
/// path actually ran (Metal GPU, notebook CPU, mixed-roster CPU, or skipped).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum TmHaltingFilterBackend {
    /// The filter was skipped because it had already been applied.
    NotApplied,
    /// No TM strategies were present, so no filtering was needed.
    #[default]
    NotRequired,
    /// Mixed roster (TMs + non-TMs): full match simulation on the CPU.
    MixedRosterCpu,
    /// All-TM roster evaluated pairwise on the CPU.
    NotebookCpu,
    /// All-TM roster: Metal probe failed or timed out, fell back to CPU.
    NotebookCpuFallback,
    /// All-TM roster evaluated on the Metal GPU.
    Metal,
}

impl TmHaltingFilterBackend {
    /// Returns a short dash-separated label suitable for logging and telemetry.
    pub fn label(self) -> &'static str {
        match self {
            Self::NotApplied => "not-applied",
            Self::NotRequired => "not-required",
            Self::MixedRosterCpu => "mixed-cpu",
            Self::NotebookCpu => "tm-cpu",
            Self::NotebookCpuFallback => "tm-cpu-fallback",
            Self::Metal => "metal",
        }
    }
}

/// Diagnostic telemetry from the TM halting filter pass.
#[derive(Clone, Debug, Default)]
pub struct TmHaltingFilterDiagnostics {
    /// Which backend actually executed the halting analysis.
    pub backend: TmHaltingFilterBackend,
    pub requested_accelerator: AcceleratorMode,
    pub strategy_count_before: usize,
    pub strategy_count_after: usize,
    pub schedule_matches: usize,
    pub scanned_matchups: usize,
    pub backend_probe_elapsed: Duration,
    pub halting_filter_elapsed: Duration,
    pub total_elapsed: Duration,
    pub tm_cache_hits: u64,
    pub tm_cache_misses: u64,
    pub tm_evaluations: u64,
    pub tm_steps: u64,
    pub metal_batches_submitted: usize,
    pub metal_decline_reason: Option<String>,
    pub metal_error: Option<String>,
    pub metal_policy_source: Option<String>,
    pub metal_matches_per_batch: Option<usize>,
    pub metal_inflight_batches: Option<usize>,
    pub metal_policy_cache_key: Option<String>,
    pub metal_policy_cache_path: Option<String>,
}

// ── Metal GPU state ─────────────────────────────────────────

/// Tracks whether the Metal GPU batch evaluator has been probed and prepared.
pub(super) enum MetalBatchState {
    Uninitialized,
    Prepared(PreparedMetalBatch),
    Unavailable,
}

/// A validated Metal batch ready for dispatch, with execution policy metadata.
pub(super) struct PreparedMetalBatch {
    pub(super) prepared: PreparedBatch,
    pub(super) policy: BatchExecutionPolicy,
    pub(super) policy_source: BatchPolicySource,
    pub(super) policy_cache_key: Option<String>,
    pub(super) policy_cache_path: Option<String>,
}

// ── Match types ─────────────────────────────────────────────

/// A scheduled matchup: two strategy indices, a repetition, and a global match id.
#[derive(Clone, Debug)]
pub(super) struct Matchup {
    pub(super) match_id: usize,
    pub(super) a_idx: usize,
    pub(super) b_idx: usize,
    pub(super) repetition: u32,
}

/// Mutable state for a single match in progress.
///
/// Holds the two strategy instances, shared history buffer, noise RNG,
/// per-round trace buffers, and cumulative scores. Created at the start
/// of each match and consumed when the match finishes.
pub(super) struct MatchSession {
    pub(super) matchup: Matchup,
    pub(super) history: History,
    pub(super) a_strategy: Box<dyn Strategy>,
    pub(super) b_strategy: Box<dyn Strategy>,
    pub(super) noise_rng: SplitMix64,
    pub(super) history_actions_a: String,
    pub(super) history_actions_b: String,
    pub(super) history_halted_a: String,
    pub(super) history_halted_b: String,
    pub(super) history_scores: String,
    pub(super) history_payoffs: Vec<[i32; 2]>,
    pub(super) round: u32,
    pub(super) rounds_total: u32,
    pub(super) a_total: i64,
    pub(super) b_total: i64,
    pub(super) a_crashed: bool,
    pub(super) b_crashed: bool,
    pub(super) record_history: bool,
    pub(super) record_trace: bool,
}

// ── Round and outcome types ─────────────────────────────────

/// Actions and payoffs from a single round of play.
#[derive(Clone, Debug)]
pub(super) struct RoundSnapshot {
    pub(super) a_action: Action,
    pub(super) b_action: Action,
    pub(super) a_halted: bool,
    pub(super) b_halted: bool,
    pub(super) a_payoff: i32,
    pub(super) b_payoff: i32,
}

/// Round snapshot plus crash flags, returned from `play_round_core`.
pub(super) struct RoundOutcome {
    pub(super) snapshot: RoundSnapshot,
    pub(super) a_crash_now: bool,
    pub(super) b_crash_now: bool,
}

// ── Accumulator types ───────────────────────────────────────

/// Per-strategy running totals used by [`TournamentAccumulator`].
#[derive(Clone, Debug)]
pub(super) struct StrategyStats {
    pub(super) total: i64,
    pub(super) adjusted_total: f64,
    pub(super) score_samples: u64,
    pub(super) matches: u32,
    pub(super) wins: u32,
    pub(super) losses: u32,
    pub(super) draws: u32,
    pub(super) crash_count: u32,
    pub(super) crashed: bool,
    pub(super) tm_stats: Option<TmRunStats>,
}

/// Pairwise head-to-head statistics between two strategies.
#[derive(Clone, Debug, Default)]
pub(super) struct PairStats {
    pub(super) a_total: i64,
    pub(super) b_total: i64,
    pub(super) a_adjusted_total: f64,
    pub(super) b_adjusted_total: f64,
    pub(super) a_wins: u32,
    pub(super) b_wins: u32,
    pub(super) draws: u32,
}

impl PairStats {
    pub(super) fn is_empty(&self) -> bool {
        self.a_total == 0
            && self.b_total == 0
            && self.a_wins == 0
            && self.b_wins == 0
            && self.draws == 0
    }
}

/// Aggregates match results into per-strategy and pairwise statistics.
pub(super) struct TournamentAccumulator {
    pub(super) strategies: Vec<StrategyStats>,
    pub(super) pairwise: Option<Vec<Vec<PairStats>>>,
    pub(super) use_adjusted: bool,
    pub(super) score_aggregation: ScoreAggregation,
}

/// Complete outcome of a match: result, crash flags, TM stats, and last round.
pub(super) struct MatchOutcome {
    pub(super) result: MatchResult,
    pub(super) a_crashed: bool,
    pub(super) b_crashed: bool,
    pub(super) a_tm_stats: Option<TmRunStats>,
    pub(super) b_tm_stats: Option<TmRunStats>,
    pub(super) last_round: Option<RoundSnapshot>,
}
