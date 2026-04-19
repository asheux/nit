//! Non-macOS stubs for the Metal GPU backend. Every public item mirrors the
//! macOS surface with a no-op body so callers can `use nit_metal::...`
//! unconditionally.

use crate::{
    BatchEvalConfig, BatchPayload, BatchPolicyCacheSnapshot, MatchPair, RecommendedBatchPolicy,
    ScorePair, TmHaltingPair,
};

pub struct PreparedBatch;
pub struct PendingBatch;

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
        "metal-unavailable".into()
    }
}

impl std::fmt::Display for MetalBackendInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Metal backend unavailable")
    }
}

macro_rules! batch_api_stubs {
    ($(
        fn $name:ident($($arg:ident: $arg_ty:ty),* $(,)?) -> $ret:ty = $body:expr;
    )*) => {
        $(
            pub fn $name($($arg: $arg_ty),*) -> $ret { $body }
        )*
    };
}

batch_api_stubs! {
    fn try_prepare_batch(_cfg: &BatchEvalConfig, _payload: &BatchPayload)
        -> Result<Option<PreparedBatch>, String> = Ok(None);
    fn try_begin_prepared_batch(_prepared: &PreparedBatch, _pairs: &[MatchPair])
        -> Result<Option<PendingBatch>, String> = Ok(None);
    fn try_evaluate_prepared_batch(_prepared: &PreparedBatch, _pairs: &[MatchPair])
        -> Result<Option<Vec<ScorePair>>, String> = Ok(None);
    fn try_finish_prepared_batch(_pending: PendingBatch)
        -> Result<Vec<ScorePair>, String> = Ok(Vec::new());
    fn try_finish_prepared_tm_halting_batch(_pending: PendingBatch)
        -> Result<Vec<TmHaltingPair>, String> = Ok(Vec::new());
    fn recommended_batch_policy(_cfg: &BatchEvalConfig, _payload: &BatchPayload)
        -> Result<Option<RecommendedBatchPolicy>, String> = Ok(None);
    fn batch_policy_cache_snapshot()
        -> Result<BatchPolicyCacheSnapshot, String> = Ok(BatchPolicyCacheSnapshot::default());
    fn clear_batch_policy_cache_entry(_path: &str)
        -> Result<bool, String> = Ok(false);
    fn clear_batch_policy_cache() -> Result<usize, String> = Ok(0);
    fn prewarm_default_batch_shaders() -> Result<(), String> = Ok(());
}
