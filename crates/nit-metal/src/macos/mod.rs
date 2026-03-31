//! macOS Metal GPU backend for game-theory tournament batch evaluation.
//!
//! Sub-modules:
//! - [`cache`] — on-disk cache for benchmark results
//! - [`device`] — GPU device introspection and capabilities
//! - [`dispatch`] — GPU buffer allocation and batch lifecycle
//! - [`policy`] — heuristic and benchmark-driven policy selection
//! - [`shader`] — Metal shader compilation and pipeline management

mod cache;
mod device;
mod dispatch;
mod policy;
mod shader;

// ---------------------------------------------------------------------------
// GPU device introspection
// ---------------------------------------------------------------------------

pub use device::{gpu_device_name, MetalBackendInfo};

// ---------------------------------------------------------------------------
// GPU dispatch and lifecycle
// ---------------------------------------------------------------------------

/// Batch dispatch: buffer allocation, kernel submission, and result collection.
pub use dispatch::{
    try_begin_prepared_batch, try_evaluate_batch, try_evaluate_prepared_batch,
    try_evaluate_prepared_tm_halting_batch, try_finish_prepared_batch,
    try_finish_prepared_tm_halting_batch, try_prepare_batch, PendingBatch, PreparedBatch,
};

// ---------------------------------------------------------------------------
// Policy and cache operations
// ---------------------------------------------------------------------------

/// Cache CRUD: snapshot, clear individual entries, or purge the entire cache.
pub use cache::{
    batch_policy_cache_snapshot, clear_batch_policy_cache, clear_batch_policy_cache_entry,
};

/// Benchmark-driven or heuristic policy recommendation for a given payload.
pub use policy::recommended_batch_policy;

/// Pre-compile default shader variants to reduce first-dispatch latency.
pub use shader::prewarm_default_batch_shaders;

// ---------------------------------------------------------------------------
// Test-only re-exports
// ---------------------------------------------------------------------------

#[cfg(test)]
use cache::{
    clear_policy_cache_entry_in_root, clear_policy_cache_in_root, load_cached_policy_from_dir,
    persist_cached_policy_from_dir, snapshot_policy_cache_from_dir, PolicyCacheEntry,
    POLICY_CACHE_SCHEMA_VERSION,
};

#[cfg(test)]
use policy::{payload_signature, preferred_base_limit, preferred_inflight_batches};

#[cfg(test)]
use shader::ShaderKey;

#[cfg(test)]
#[path = "../tests/macos.rs"]
mod tests;
