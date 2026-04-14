//! Metal GPU acceleration for game-theory tournament batch evaluation.
//!
//! On non-macOS platforms every public function is a no-op stub so the
//! workspace compiles unconditionally.

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
