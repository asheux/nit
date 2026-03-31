//! Metal GPU device introspection.
//!
//! Wraps device probing into a single snapshot so callers can inspect
//! GPU name, memory budget, and performance tier without repeated FFI calls.

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
        const HIGH_PERF_TIERS: &[&str] = &["Pro", "Max", "Ultra"];
        HIGH_PERF_TIERS
            .iter()
            .any(|tier| self.device_name.contains(tier))
    }

    /// Working set converted to mebibytes, rounded down.
    pub fn working_set_mib(&self) -> u64 {
        self.working_set_bytes / (1024 * 1024)
    }

    /// Short diagnostic label suitable for log lines and cache key prefixes.
    ///
    /// Format: `metal-macos/<device_name>/<working_set>MiB`
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
