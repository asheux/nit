//! Runtime configuration and per-run state: parallelism, seed derivation,
//! Metal GPU batch state, and TM halting filter diagnostics.

use crate::config::{AcceleratorMode, ParallelismConfig, ParallelismMode};
use nit_metal::{BatchExecutionPolicy, BatchPolicySource, PreparedBatch};
use nit_utils::hashing::stable_hash_bytes;
use rayon::ThreadPoolBuilder;
use std::time::Duration;

use super::match_state::MatchRole;

/// Deterministic seed derivation from a tournament-level seed.
///
/// Each match gets unique per-strategy and per-noise seeds, derived
/// from the run seed, strategy id, match id, and repetition index.
#[derive(Clone, Debug)]
pub struct SeedDeriver {
    pub run_seed: u64,
    pub noise_base: u64,
}

impl SeedDeriver {
    pub fn new(run_seed: u64) -> Self {
        let noise_base = stable_hash_bytes(format!("{run_seed}:noise").as_bytes());
        Self {
            run_seed,
            noise_base,
        }
    }

    // Base seed per strategy role; per-match seeds derive from this plus match_id/repetition.
    pub fn base_strategy_seed(&self, role: MatchRole, strategy_id: &str) -> u64 {
        stable_hash_bytes(format!("{}:{}:{}", self.run_seed, role.label(), strategy_id).as_bytes())
    }

    pub fn strategy_seed(
        &self,
        match_id: usize,
        repetition: u32,
        role: MatchRole,
        strategy_id: &str,
    ) -> u64 {
        let base = self.base_strategy_seed(role, strategy_id);
        stable_hash_bytes(format!("{base}:{match_id}:{repetition}").as_bytes())
    }

    pub fn noise_seed(&self, match_id: usize, repetition: u32) -> u64 {
        stable_hash_bytes(format!("{}:{match_id}:{repetition}", self.noise_base).as_bytes())
    }
}

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
/// threads. All other variants run on the global pool (`Auto`) or the calling
/// thread (`Off`).
pub fn run_with_parallelism<T: Send>(parallelism: Parallelism, f: impl FnOnce() -> T + Send) -> T {
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

/// Tracks whether the Metal GPU batch evaluator has been probed and prepared.
pub enum MetalBatchState {
    Uninitialized,
    Prepared(PreparedMetalBatch),
    Unavailable,
}

/// A validated Metal batch ready for dispatch, with execution policy metadata.
pub struct PreparedMetalBatch {
    pub prepared: PreparedBatch,
    pub policy: BatchExecutionPolicy,
    pub policy_source: BatchPolicySource,
    pub policy_cache_key: Option<String>,
    pub policy_cache_path: Option<String>,
}
