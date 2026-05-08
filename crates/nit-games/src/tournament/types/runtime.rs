use crate::config::{AcceleratorMode, ParallelismConfig, ParallelismMode};
use nit_metal::{BatchExecutionPolicy, BatchPolicySource, PreparedBatch};
use nit_utils::hashing::stable_hash_bytes;
use rayon::ThreadPoolBuilder;
use std::time::Duration;

use super::match_state::MatchRole;

// Per-match seeds derive from `(run_seed, role, strategy_id, match_id, repetition)`.
// Splitting on role ensures A and B never share an RNG stream even on
// self-play; splitting on (match_id, repetition) ensures every match is
// independently reproducible from the run seed.
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
                ParallelismMode::Auto => Self::Auto,
                ParallelismMode::Off => Self::Off,
            },
            ParallelismConfig::Threads { threads } => Self::Threads(*threads),
        }
    }
}

// `Threads(n)` builds a dedicated pool because we don't want our work to
// share the global Rayon pool with caller-driven parallelism (e.g.
// per-strategy fast-eval). `Auto` uses the global pool; `Off` runs inline.
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

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum TmHaltingFilterBackend {
    NotApplied,
    #[default]
    NotRequired,
    MixedRosterCpu,
    NotebookCpu,
    NotebookCpuFallback,
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

pub enum MetalBatchState {
    Uninitialized,
    Prepared(PreparedMetalBatch),
    Unavailable,
}

pub struct PreparedMetalBatch {
    pub prepared: PreparedBatch,
    pub policy: BatchExecutionPolicy,
    pub policy_source: BatchPolicySource,
    pub policy_cache_key: Option<String>,
    pub policy_cache_path: Option<String>,
}
