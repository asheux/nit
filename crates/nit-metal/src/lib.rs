//! Metal GPU acceleration for compute-intensive operations (macOS only).
//!
//! On non-macOS platforms every public function returns a no-op stub so that
//! the rest of the workspace compiles unconditionally.

mod types;
pub use types::*;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{
    batch_policy_cache_snapshot, clear_batch_policy_cache, clear_batch_policy_cache_entry,
    gpu_device_name, prewarm_default_batch_shaders, recommended_batch_policy,
    try_begin_prepared_batch, try_evaluate_batch, try_evaluate_prepared_batch,
    try_evaluate_prepared_tm_halting_batch, try_finish_prepared_batch,
    try_finish_prepared_tm_halting_batch, try_prepare_batch, MetalBackendInfo, PendingBatch,
    PreparedBatch,
};

// ---------------------------------------------------------------------------
// Cross-platform payload introspection
// ---------------------------------------------------------------------------

impl BatchPayload {
    /// Returns the kernel variant name used in logging and cache key prefixes.
    pub fn variant_name(&self) -> &'static str {
        match self {
            BatchPayload::Fsm(_) => "fsm",
            BatchPayload::Ca(_) => "ca",
            BatchPayload::Tm(_) => "tm",
        }
    }

    /// Number of individual agents (automata) encoded in this batch.
    ///
    /// For FSM payloads this equals the number of start states; for CA
    /// payloads it is derived from the rule table length divided by entries
    /// per rule; for TM payloads it equals the number of start states.
    pub fn population_count(&self) -> usize {
        match self {
            BatchPayload::Fsm(fsm) => fsm.starts.len(),
            BatchPayload::Ca(ca) => {
                let entries_per_rule = ca.rule_table_len as usize;
                if entries_per_rule == 0 {
                    0
                } else {
                    ca.rule_tables.len() / entries_per_rule
                }
            }
            BatchPayload::Tm(tm) => tm.start_states.len(),
        }
    }

    /// State-space dimension of the encoded automata.
    ///
    /// Returns the number of states for FSM and TM payloads, or the
    /// number of distinct symbols for CA payloads.
    pub fn state_dimension(&self) -> u32 {
        match self {
            BatchPayload::Fsm(fsm) => fsm.states,
            BatchPayload::Ca(ca) => ca.symbols,
            BatchPayload::Tm(tm) => tm.states,
        }
    }
}

impl BatchRequest {
    /// Number of match pairs that will be evaluated in this request.
    pub fn pair_count(&self) -> usize {
        self.common.pairs.len()
    }

    /// Kernel variant name derived from the inner payload.
    pub fn kernel_variant(&self) -> &'static str {
        self.payload.variant_name()
    }
}

impl BatchExecutionPolicy {
    /// Total matches that may be in-flight across all concurrent batches.
    pub fn total_inflight_matches(&self) -> usize {
        self.matches_per_batch.saturating_mul(self.inflight_batches)
    }
}

impl BatchPolicyCacheSnapshot {
    /// Returns `true` when the cache contains no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of cached policy entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

// ---------------------------------------------------------------------------
// Non-macOS stubs
// ---------------------------------------------------------------------------
//
// Each stub mirrors the macOS public API with a minimal no-op body so that
// dependent crates compile on Linux, Windows, and CI containers without
// conditional compilation at every call-site.

/// Runtime capabilities of the Metal GPU backend.
///
/// On non-macOS this is a placeholder struct; [`MetalBackendInfo::probe`]
/// always returns `None`.
#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone)]
pub struct MetalBackendInfo {
    /// GPU device name (unavailable on non-macOS).
    pub device_name: String,

    /// Recommended working set in bytes (zero on non-macOS).
    pub working_set_bytes: u64,
}

#[cfg(not(target_os = "macos"))]
impl MetalBackendInfo {
    /// Always returns `None` — no Metal device on this platform.
    pub fn probe() -> Option<Self> {
        None
    }

    /// Always `false` — no GPU tier detection without Metal.
    pub fn is_high_performance(&self) -> bool {
        false
    }

    /// Returns `0` — no working set budget on non-macOS.
    pub fn working_set_mib(&self) -> u64 {
        0
    }

    /// Returns a static placeholder label.
    pub fn diagnostic_label(&self) -> String {
        String::from("metal-unavailable")
    }
}

#[cfg(not(target_os = "macos"))]
impl std::fmt::Display for MetalBackendInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Metal backend unavailable")
    }
}

/// Device name stub — always `None` on non-macOS.
#[cfg(not(target_os = "macos"))]
pub fn gpu_device_name() -> Option<String> {
    None
}

// --- Batch evaluation stubs ------------------------------------------------

/// Opaque handle for a prepared (but not yet dispatched) GPU batch.
#[cfg(not(target_os = "macos"))]
pub struct PreparedBatch;

/// Opaque handle for an in-flight GPU batch awaiting completion.
#[cfg(not(target_os = "macos"))]
pub struct PendingBatch;

/// Full evaluate-and-collect: no-op without Metal.
#[cfg(not(target_os = "macos"))]
pub fn try_evaluate_batch(_request: &BatchRequest) -> Result<Option<Vec<ScorePair>>, String> {
    Ok(None)
}

/// Policy recommendation: no-op without Metal.
#[cfg(not(target_os = "macos"))]
pub fn recommended_batch_policy(
    _config: &BatchEvalConfig,
    _payload: &BatchPayload,
) -> Result<Option<RecommendedBatchPolicy>, String> {
    Ok(None)
}

/// Cache snapshot: returns an empty snapshot without Metal.
#[cfg(not(target_os = "macos"))]
pub fn batch_policy_cache_snapshot() -> Result<BatchPolicyCacheSnapshot, String> {
    Ok(BatchPolicyCacheSnapshot::default())
}

/// Clear a single cache entry: no-op without Metal.
#[cfg(not(target_os = "macos"))]
pub fn clear_batch_policy_cache_entry(_path: &str) -> Result<bool, String> {
    Ok(false)
}

/// Purge the full cache: no-op without Metal.
#[cfg(not(target_os = "macos"))]
pub fn clear_batch_policy_cache() -> Result<usize, String> {
    Ok(0)
}

/// Shader pre-warming: no-op without Metal.
#[cfg(not(target_os = "macos"))]
pub fn prewarm_default_batch_shaders() -> Result<(), String> {
    Ok(())
}

// --- Prepared batch lifecycle stubs ----------------------------------------

/// Prepare a batch context: no-op without Metal.
#[cfg(not(target_os = "macos"))]
pub fn try_prepare_batch(
    _config: &BatchEvalConfig,
    _payload: &BatchPayload,
) -> Result<Option<PreparedBatch>, String> {
    Ok(None)
}

/// Evaluate using a pre-prepared batch: no-op without Metal.
#[cfg(not(target_os = "macos"))]
pub fn try_evaluate_prepared_batch(
    _prepared: &PreparedBatch,
    _pairs: &[MatchPair],
) -> Result<Option<Vec<ScorePair>>, String> {
    Ok(None)
}

/// Evaluate TM halting via prepared batch: no-op without Metal.
#[cfg(not(target_os = "macos"))]
pub fn try_evaluate_prepared_tm_halting_batch(
    _prepared: &PreparedBatch,
    _pairs: &[MatchPair],
) -> Result<Option<Vec<TmHaltingPair>>, String> {
    Ok(None)
}

/// Begin async dispatch: no-op without Metal.
#[cfg(not(target_os = "macos"))]
pub fn try_begin_prepared_batch(
    _prepared: &PreparedBatch,
    _pairs: &[MatchPair],
) -> Result<Option<PendingBatch>, String> {
    Ok(None)
}

/// Collect results from a pending batch: no-op without Metal.
#[cfg(not(target_os = "macos"))]
pub fn try_finish_prepared_batch(_pending: PendingBatch) -> Result<Vec<ScorePair>, String> {
    Ok(Vec::new())
}

/// Collect TM halting results: no-op without Metal.
#[cfg(not(target_os = "macos"))]
pub fn try_finish_prepared_tm_halting_batch(
    _pending: PendingBatch,
) -> Result<Vec<TmHaltingPair>, String> {
    Ok(Vec::new())
}
