//! Metal GPU acceleration for compute-intensive game-theory tournaments.
//!
//! This crate provides a platform-adaptive backend for evaluating large
//! populations of finite state machines, cellular automata, and Turing
//! machines in batch on Apple Silicon GPUs via Metal.
//!
//! On non-macOS platforms every public function returns a no-op stub so that
//! the rest of the workspace compiles unconditionally without `#[cfg]`
//! guards at every call site.

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

#[cfg(not(target_os = "macos"))]
mod stubs;

#[cfg(not(target_os = "macos"))]
pub use stubs::*;

// ---------------------------------------------------------------------------
// Cross-platform payload introspection
// ---------------------------------------------------------------------------

impl BatchPayload {
    /// Returns the kernel variant name used in logging and cache key prefixes.
    ///
    /// Each variant maps to a distinct Metal compute kernel; the returned
    /// string is stable and safe to embed in filesystem paths.
    pub fn variant_name(&self) -> &'static str {
        match self {
            Self::Fsm(_) => "fsm",
            Self::Ca(_) => "ca",
            Self::Tm(_) => "tm",
        }
    }

    /// Number of individual strategies (automata) encoded in this payload.
    ///
    /// For FSM payloads this equals the number of start states; for CA
    /// payloads it is derived from the rule table length divided by entries
    /// per strategy; for TM payloads it equals the number of start states.
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

    /// State-space dimension of the encoded automata.
    ///
    /// Returns the number of internal states for FSM and TM payloads,
    /// or the number of distinct cell symbols for CA payloads.
    pub fn state_dimension(&self) -> u32 {
        match self {
            Self::Fsm(fsm) => fsm.states,
            Self::Ca(ca) => ca.symbols,
            Self::Tm(tm) => tm.states,
        }
    }
}

// ---------------------------------------------------------------------------
// Request-level convenience accessors
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Policy arithmetic
// ---------------------------------------------------------------------------

impl BatchExecutionPolicy {
    /// Total matches that may be in-flight across all concurrent batches.
    ///
    /// Saturates rather than wrapping on overflow, which is defensive
    /// against absurdly large configurations reaching the UI layer.
    pub fn total_inflight_matches(&self) -> usize {
        self.matches_per_batch.saturating_mul(self.inflight_batches)
    }
}

// ---------------------------------------------------------------------------
// Cache snapshot queries
// ---------------------------------------------------------------------------

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
