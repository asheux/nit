//! Non-macOS stubs for the Metal GPU backend.
//!
//! Each item mirrors the macOS public API with a minimal no-op body so that
//! dependent crates compile on Linux, Windows, and CI containers without
//! conditional compilation at every call-site.

use crate::{
    BatchEvalConfig, BatchPayload, BatchPolicyCacheSnapshot, BatchRequest, MatchPair,
    RecommendedBatchPolicy, ScorePair, TmHaltingPair,
};

// ---------------------------------------------------------------------------
// Device info stub
// ---------------------------------------------------------------------------

/// Runtime capabilities of the Metal GPU backend.
///
/// On non-macOS this is a placeholder struct; [`MetalBackendInfo::probe`]
/// always returns `None`, and all methods return sensible defaults.
#[derive(Debug, Clone)]
pub struct MetalBackendInfo {
    /// GPU device name (unavailable on non-macOS).
    pub device_name: String,
    /// Recommended working set in bytes (zero on non-macOS).
    pub working_set_bytes: u64,
}

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

impl std::fmt::Display for MetalBackendInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Metal backend unavailable")
    }
}

/// Device name stub — always `None` on non-macOS.
pub fn gpu_device_name() -> Option<String> {
    None
}

// ---------------------------------------------------------------------------
// Opaque batch handles
// ---------------------------------------------------------------------------

/// Opaque handle for a prepared (but not yet dispatched) GPU batch.
///
/// On non-macOS this type exists only to satisfy the public API; no instance
/// is ever constructed because `try_prepare_batch` always returns `None`.
pub struct PreparedBatch;

/// Opaque handle for an in-flight GPU batch awaiting completion.
///
/// On non-macOS this type exists only to satisfy the public API; no instance
/// is ever constructed because `try_begin_prepared_batch` always returns `None`.
pub struct PendingBatch;

// ---------------------------------------------------------------------------
// Full evaluation stubs
// ---------------------------------------------------------------------------

/// Full evaluate-and-collect: no-op without Metal.
pub fn try_evaluate_batch(_request: &BatchRequest) -> Result<Option<Vec<ScorePair>>, String> {
    Ok(None)
}

/// Evaluate using a pre-prepared batch: no-op without Metal.
pub fn try_evaluate_prepared_batch(
    _prepared: &PreparedBatch,
    _pairs: &[MatchPair],
) -> Result<Option<Vec<ScorePair>>, String> {
    Ok(None)
}

/// Evaluate TM halting via prepared batch: no-op without Metal.
pub fn try_evaluate_prepared_tm_halting_batch(
    _prepared: &PreparedBatch,
    _pairs: &[MatchPair],
) -> Result<Option<Vec<TmHaltingPair>>, String> {
    Ok(None)
}

// ---------------------------------------------------------------------------
// Prepared batch lifecycle stubs
// ---------------------------------------------------------------------------

/// Prepare a batch context: no-op without Metal.
pub fn try_prepare_batch(
    _config: &BatchEvalConfig,
    _payload: &BatchPayload,
) -> Result<Option<PreparedBatch>, String> {
    Ok(None)
}

/// Begin async dispatch: no-op without Metal.
pub fn try_begin_prepared_batch(
    _prepared: &PreparedBatch,
    _pairs: &[MatchPair],
) -> Result<Option<PendingBatch>, String> {
    Ok(None)
}

/// Collect results from a pending batch: no-op without Metal.
pub fn try_finish_prepared_batch(_pending: PendingBatch) -> Result<Vec<ScorePair>, String> {
    Ok(Vec::new())
}

/// Collect TM halting results: no-op without Metal.
pub fn try_finish_prepared_tm_halting_batch(
    _pending: PendingBatch,
) -> Result<Vec<TmHaltingPair>, String> {
    Ok(Vec::new())
}

// ---------------------------------------------------------------------------
// Policy and cache stubs
// ---------------------------------------------------------------------------

/// Policy recommendation: no-op without Metal.
pub fn recommended_batch_policy(
    _config: &BatchEvalConfig,
    _payload: &BatchPayload,
) -> Result<Option<RecommendedBatchPolicy>, String> {
    Ok(None)
}

/// Cache snapshot: returns an empty snapshot without Metal.
pub fn batch_policy_cache_snapshot() -> Result<BatchPolicyCacheSnapshot, String> {
    Ok(BatchPolicyCacheSnapshot::default())
}

/// Clear a single cache entry: no-op without Metal.
pub fn clear_batch_policy_cache_entry(_path: &str) -> Result<bool, String> {
    Ok(false)
}

/// Purge the full cache: no-op without Metal.
pub fn clear_batch_policy_cache() -> Result<usize, String> {
    Ok(0)
}

/// Shader pre-warming: no-op without Metal.
pub fn prewarm_default_batch_shaders() -> Result<(), String> {
    Ok(())
}
