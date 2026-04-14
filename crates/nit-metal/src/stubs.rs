//! Non-macOS stubs for the Metal GPU backend.
//!
//! Each item mirrors the macOS public API with a no-op body so that
//! dependent crates compile without conditional compilation at call sites.

use crate::{
    BatchEvalConfig, BatchPayload, BatchPolicyCacheSnapshot, BatchRequest, MatchPair,
    RecommendedBatchPolicy, ScorePair, TmHaltingPair,
};

/// Runtime capabilities of the Metal GPU backend (placeholder on non-macOS).
#[derive(Debug, Clone)]
pub struct MetalBackendInfo {
    pub device_name: String,
    pub working_set_bytes: u64,
}

impl MetalBackendInfo {
    pub fn probe() -> Option<Self> {
        None
    }

    pub fn is_high_performance(&self) -> bool {
        false
    }

    pub fn working_set_mib(&self) -> u64 {
        0
    }

    pub fn diagnostic_label(&self) -> String {
        String::from("metal-unavailable")
    }
}

impl std::fmt::Display for MetalBackendInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Metal backend unavailable")
    }
}

pub fn gpu_device_name() -> Option<String> {
    None
}

/// Never constructed on non-macOS; exists only to satisfy the public API.
pub struct PreparedBatch;

/// Never constructed on non-macOS; exists only to satisfy the public API.
pub struct PendingBatch;

macro_rules! batch_api_stubs {
    ($(pub fn $name:ident( $($sig:tt)* ) -> $ret:ty { $body:expr })*) => {
        $(pub fn $name( $($sig)* ) -> $ret { $body })*
    };
}

batch_api_stubs! {
    pub fn try_evaluate_batch(
        _request: &BatchRequest
    ) -> Result<Option<Vec<ScorePair>>, String> { Ok(None) }

    pub fn try_evaluate_prepared_batch(
        _prepared: &PreparedBatch, _pairs: &[MatchPair]
    ) -> Result<Option<Vec<ScorePair>>, String> { Ok(None) }

    pub fn try_evaluate_prepared_tm_halting_batch(
        _prepared: &PreparedBatch, _pairs: &[MatchPair]
    ) -> Result<Option<Vec<TmHaltingPair>>, String> { Ok(None) }

    pub fn try_prepare_batch(
        _config: &BatchEvalConfig, _payload: &BatchPayload
    ) -> Result<Option<PreparedBatch>, String> { Ok(None) }

    pub fn try_begin_prepared_batch(
        _prepared: &PreparedBatch, _pairs: &[MatchPair]
    ) -> Result<Option<PendingBatch>, String> { Ok(None) }

    pub fn try_finish_prepared_batch(
        _pending: PendingBatch
    ) -> Result<Vec<ScorePair>, String> { Ok(Vec::new()) }

    pub fn try_finish_prepared_tm_halting_batch(
        _pending: PendingBatch
    ) -> Result<Vec<TmHaltingPair>, String> { Ok(Vec::new()) }

    pub fn recommended_batch_policy(
        _config: &BatchEvalConfig, _payload: &BatchPayload
    ) -> Result<Option<RecommendedBatchPolicy>, String> { Ok(None) }

    pub fn batch_policy_cache_snapshot(
    ) -> Result<BatchPolicyCacheSnapshot, String> { Ok(BatchPolicyCacheSnapshot::default()) }

    pub fn clear_batch_policy_cache_entry(
        _path: &str
    ) -> Result<bool, String> { Ok(false) }

    pub fn clear_batch_policy_cache(
    ) -> Result<usize, String> { Ok(0) }

    pub fn prewarm_default_batch_shaders(
    ) -> Result<(), String> { Ok(()) }
}
