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
