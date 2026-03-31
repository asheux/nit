//! macOS Metal GPU backend for game-theory tournament batch evaluation.
//!
//! Sub-modules:
//! - [`cache`] — on-disk cache for benchmark results
//! - [`dispatch`] — GPU buffer allocation and batch lifecycle
//! - [`policy`] — heuristic and benchmark-driven policy selection
//! - [`shader`] — Metal shader compilation and pipeline management

mod cache;
mod dispatch;
mod policy;
mod shader;

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
// Backend introspection
// ---------------------------------------------------------------------------

/// Runtime capabilities of the Metal GPU backend on this machine.
///
/// Wraps device probing into a single snapshot so callers can inspect
/// GPU name, memory budget, and performance tier without repeated FFI calls.
#[derive(Debug, Clone)]
pub struct MetalBackendInfo {
    /// GPU device name reported by Metal, e.g. "Apple M4 Max".
    pub device_name: String,

    /// Recommended maximum working set in bytes for this device.
    pub working_set_bytes: u64,
}

impl MetalBackendInfo {
    /// Probes the system default Metal device and captures its capabilities.
    ///
    /// Returns `None` when no Metal-capable GPU is available (e.g. CI runners
    /// without discrete graphics or VMs without GPU passthrough).
    pub fn probe() -> Option<Self> {
        let device = metal::Device::system_default()?;
        Some(Self {
            device_name: device.name().to_string(),
            working_set_bytes: device.recommended_max_working_set_size(),
        })
    }

    /// Returns `true` when the device belongs to a high-core-count Apple
    /// Silicon tier (Pro, Max, or Ultra) that benefits from deeper dispatch
    /// queues and larger batch sizes.
    pub fn is_high_performance(&self) -> bool {
        self.device_name.contains("Pro")
            || self.device_name.contains("Max")
            || self.device_name.contains("Ultra")
    }

    /// Working set converted to mebibytes, rounded down.
    pub fn working_set_mib(&self) -> u64 {
        self.working_set_bytes / (1024 * 1024)
    }

    /// Short diagnostic label suitable for log lines and cache key prefixes.
    pub fn diagnostic_label(&self) -> String {
        format!(
            "metal-macos/{}/{}MiB",
            self.device_name,
            self.working_set_mib()
        )
    }
}

impl std::fmt::Display for MetalBackendInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({} MiB working set)",
            self.device_name,
            self.working_set_mib()
        )
    }
}

/// Convenience wrapper: returns the Metal GPU device name if available.
///
/// Equivalent to `MetalBackendInfo::probe().map(|info| info.device_name)`
/// but avoids constructing the full info struct when only the name is needed.
pub fn gpu_device_name() -> Option<String> {
    let device = metal::Device::system_default()?;
    Some(device.name().to_string())
}

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
