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

#[cfg(not(target_os = "macos"))]
mod stubs;

#[cfg(not(target_os = "macos"))]
pub use stubs::*;

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
