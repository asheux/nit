use crate::config::{AcceleratorMode, ParallelismConfig, ParallelismMode, ScoreAggregation};
use crate::game::{Action, Outcome};
use crate::history::History;
use crate::output::RuntimeAcceleratorStats;
use crate::strategy::{Strategy, TmRunStats};
use nit_metal::{BatchExecutionPolicy, BatchPolicySource, PreparedBatch};
use nit_utils::hashing::{stable_hash_bytes, SplitMix64};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct TournamentProgress {
    pub match_index: usize,
    pub total_matches: usize,
    pub round: u32,
    pub rounds: u32,
    pub match_complete: bool,
    pub a: String,
    pub b: String,
    pub total_payoff_a: i64,
    pub total_payoff_b: i64,
    pub last_action_a: Option<Action>,
    pub last_action_b: Option<Action>,
    pub last_payoff_a: Option<i32>,
    pub last_payoff_b: Option<i32>,
    pub last_halted_a: Option<bool>,
    pub last_halted_b: Option<bool>,
    pub last_outcome: Option<Outcome>,
    pub runtime: RuntimeAcceleratorStats,
}

#[derive(Clone, Debug)]
pub struct MatchSnapshot {
    pub match_index: usize,
    pub total_matches: usize,
    pub round: u32,
    pub rounds: u32,
    pub a: String,
    pub b: String,
    pub a_score: i64,
    pub b_score: i64,
    pub outcomes: String,
    pub payoffs: Vec<[i32; 2]>,
    pub a_halted: String,
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
    pub const DISPLAY_ROUND_CAP: usize = 500;

    pub fn preview_rounds(&self) -> usize {
        self.outcomes.len().min(Self::DISPLAY_ROUND_CAP)
    }

    pub fn preview_outcomes(&self) -> &str {
        let end = self.preview_rounds();
        self.outcomes.get(..end).unwrap_or(self.outcomes.as_str())
    }
}

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

#[derive(Copy, Clone, Debug)]
pub(super) enum MatchRole {
    A,
    B,
}

impl MatchRole {
    pub(super) fn label(self) -> &'static str {
        match self {
            MatchRole::A => "A",
            MatchRole::B => "B",
        }
    }
}

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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Parallelism {
    Auto,
    Off,
    Threads(usize),
}

impl Parallelism {
    pub fn from_config(config: &ParallelismConfig) -> Self {
        match config {
            ParallelismConfig::Mode(mode) => match mode {
                ParallelismMode::Auto => Parallelism::Auto,
                ParallelismMode::Off => Parallelism::Off,
            },
            ParallelismConfig::Threads { threads } => Parallelism::Threads(*threads),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TmHaltingFilterBackend {
    NotApplied,
    NotRequired,
    MixedRosterCpu,
    NotebookCpu,
    NotebookCpuFallback,
    Metal,
}

impl TmHaltingFilterBackend {
    pub fn label(self) -> &'static str {
        match self {
            TmHaltingFilterBackend::NotApplied => "not-applied",
            TmHaltingFilterBackend::NotRequired => "not-required",
            TmHaltingFilterBackend::MixedRosterCpu => "mixed-cpu",
            TmHaltingFilterBackend::NotebookCpu => "tm-cpu",
            TmHaltingFilterBackend::NotebookCpuFallback => "tm-cpu-fallback",
            TmHaltingFilterBackend::Metal => "metal",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TmHaltingFilterDiagnostics {
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

impl Default for TmHaltingFilterDiagnostics {
    fn default() -> Self {
        Self {
            backend: TmHaltingFilterBackend::NotRequired,
            requested_accelerator: AcceleratorMode::default(),
            strategy_count_before: 0,
            strategy_count_after: 0,
            schedule_matches: 0,
            scanned_matchups: 0,
            backend_probe_elapsed: Duration::ZERO,
            halting_filter_elapsed: Duration::ZERO,
            total_elapsed: Duration::ZERO,
            tm_cache_hits: 0,
            tm_cache_misses: 0,
            tm_evaluations: 0,
            tm_steps: 0,
            metal_batches_submitted: 0,
            metal_decline_reason: None,
            metal_error: None,
            metal_policy_source: None,
            metal_matches_per_batch: None,
            metal_inflight_batches: None,
            metal_policy_cache_key: None,
            metal_policy_cache_path: None,
        }
    }
}

pub(super) enum MetalBatchState {
    Uninitialized,
    Prepared(PreparedMetalBatch),
    Unavailable,
}

pub(super) struct PreparedMetalBatch {
    pub(super) prepared: PreparedBatch,
    pub(super) policy: BatchExecutionPolicy,
    pub(super) policy_source: BatchPolicySource,
    pub(super) policy_cache_key: Option<String>,
    pub(super) policy_cache_path: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct Matchup {
    pub(super) match_id: usize,
    pub(super) a_idx: usize,
    pub(super) b_idx: usize,
    pub(super) repetition: u32,
}

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

#[derive(Clone, Debug)]
pub(super) struct RoundSnapshot {
    pub(super) a_action: Action,
    pub(super) b_action: Action,
    pub(super) a_halted: bool,
    pub(super) b_halted: bool,
    pub(super) a_payoff: i32,
    pub(super) b_payoff: i32,
}

pub(super) struct RoundOutcome {
    pub(super) snapshot: RoundSnapshot,
    pub(super) a_crash_now: bool,
    pub(super) b_crash_now: bool,
}

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

pub(super) struct TournamentAccumulator {
    pub(super) strategies: Vec<StrategyStats>,
    pub(super) pairwise: Option<Vec<Vec<PairStats>>>,
    pub(super) use_adjusted: bool,
    pub(super) score_aggregation: ScoreAggregation,
}

pub(super) struct MatchOutcome {
    pub(super) result: MatchResult,
    pub(super) a_crashed: bool,
    pub(super) b_crashed: bool,
    pub(super) a_tm_stats: Option<TmRunStats>,
    pub(super) b_tm_stats: Option<TmRunStats>,
    pub(super) last_round: Option<RoundSnapshot>,
}
