//! macOS Metal GPU backend for game-theory tournament batch evaluation.
//!
//! Architecture layers (bottom-up):
//!
//! 1. **[`shader`]** — compiles Metal shader source into per-variant pipeline states,
//!    cached behind a `OnceLock` singleton keyed by [`shader::ShaderKey`].
//! 2. **[`device`]** — probes the system default Metal device and captures its
//!    name, memory budget, and performance tier into [`MetalBackendInfo`].
//! 3. **[`dispatch`]** — allocates GPU buffers, encodes compute commands, and
//!    manages the prepared → pending → completed batch lifecycle.
//! 4. **[`policy`]** — selects optimal batch sizes via device-tier heuristics
//!    or live GPU benchmarking, persisting winners through the cache layer.
//! 5. **[`cache`]** — JSON-based on-disk storage keyed by device name and
//!    payload signature, with schema-versioned validation.

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
// Batch dispatch and lifecycle
// ---------------------------------------------------------------------------

pub use dispatch::{
    try_begin_prepared_batch, try_evaluate_batch, try_evaluate_prepared_batch,
    try_evaluate_prepared_tm_halting_batch, try_finish_prepared_batch,
    try_finish_prepared_tm_halting_batch, try_prepare_batch, PendingBatch, PreparedBatch,
};

// ---------------------------------------------------------------------------
// Policy selection and cache management
// ---------------------------------------------------------------------------

pub use cache::{
    batch_policy_cache_snapshot, clear_batch_policy_cache, clear_batch_policy_cache_entry,
};
pub use policy::recommended_batch_policy;
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
