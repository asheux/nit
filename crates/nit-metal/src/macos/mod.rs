//! macOS Metal GPU backend.
//!
//! Layers: shader compilation → device probing → buffer dispatch → policy
//! tuning → on-disk cache.

mod cache;
mod device;
mod dispatch;
mod policy;
mod shader;

pub use device::{gpu_device_name, MetalBackendInfo};

pub use dispatch::{
    try_begin_prepared_batch, try_evaluate_batch, try_evaluate_prepared_batch,
    try_evaluate_prepared_tm_halting_batch, try_finish_prepared_batch,
    try_finish_prepared_tm_halting_batch, try_prepare_batch, PendingBatch, PreparedBatch,
};

pub use cache::{
    batch_policy_cache_snapshot, clear_batch_policy_cache, clear_batch_policy_cache_entry,
};
pub use policy::recommended_batch_policy;
pub use shader::prewarm_default_batch_shaders;

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
