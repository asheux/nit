//! Metal GPU device introspection.

/// Runtime capabilities of the Metal GPU backend on this machine.
#[derive(Debug, Clone)]
pub struct MetalBackendInfo {
    pub device_name: String,
    pub working_set_bytes: u64,
}

impl MetalBackendInfo {
    pub fn probe() -> Option<Self> {
        let device = metal::Device::system_default()?;
        Some(Self {
            device_name: device.name().to_string(),
            working_set_bytes: device.recommended_max_working_set_size(),
        })
    }

    /// High-core-count Apple Silicon (Pro, Max, Ultra) benefits from deeper
    /// dispatch queues and larger batch sizes.
    pub fn is_high_performance(&self) -> bool {
        const HIGH_PERF_TIERS: &[&str] = &["Pro", "Max", "Ultra"];
        HIGH_PERF_TIERS
            .iter()
            .any(|tier| self.device_name.contains(tier))
    }

    pub fn working_set_mib(&self) -> u64 {
        self.working_set_bytes / (1024 * 1024)
    }

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

pub fn gpu_device_name() -> Option<String> {
    let device = metal::Device::system_default()?;
    Some(device.name().to_string())
}
