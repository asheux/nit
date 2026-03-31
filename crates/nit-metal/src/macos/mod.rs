//! macOS Metal GPU backend for game-theory tournament batch evaluation.

mod dispatch;
mod policy;
mod shader;

pub use dispatch::{
    try_begin_prepared_batch, try_evaluate_batch, try_evaluate_prepared_batch,
    try_evaluate_prepared_tm_halting_batch, try_finish_prepared_batch,
    try_finish_prepared_tm_halting_batch, try_prepare_batch, PendingBatch, PreparedBatch,
};
pub use policy::{
    batch_policy_cache_snapshot, clear_batch_policy_cache, clear_batch_policy_cache_entry,
    recommended_batch_policy,
};
pub use shader::prewarm_default_batch_shaders;

// Test-only re-exports: bring internal items into this namespace so
// `super::` works from the test module (which is a child of `macos`).
#[cfg(test)]
use policy::{
    clear_policy_cache_entry_in_root, clear_policy_cache_in_root, load_cached_policy_from_dir,
    payload_signature, persist_cached_policy_from_dir, preferred_base_limit,
    preferred_inflight_batches, snapshot_policy_cache_from_dir, PolicyCacheEntry,
    POLICY_CACHE_SCHEMA_VERSION,
};
#[cfg(test)]
use shader::ShaderKey;

#[cfg(test)]
#[path = "../tests/macos.rs"]
mod tests;
