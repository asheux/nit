//! Metal GPU device introspection and Apple Silicon tier detection.

/// Apple Silicon performance tier inferred from the device name.
///
/// Shared by policy tuning and [`MetalBackendInfo`] so a single source of
/// truth drives batch-size / queue-depth heuristics.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum AppleTier {
    Base,
    Pro,
    Max,
    Ultra,
}

/// Ordered by specificity: every Ultra also contains "Max" historically, so
/// Ultra must be matched first.
pub(crate) fn apple_tier(device_name: &str) -> AppleTier {
    if device_name.contains("Ultra") {
        return AppleTier::Ultra;
    }
    if device_name.contains("Max") {
        return AppleTier::Max;
    }
    if device_name.contains("Pro") {
        return AppleTier::Pro;
    }
    AppleTier::Base
}

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

    /// Any non-base Apple Silicon tier (Pro/Max/Ultra) benefits from deeper
    /// dispatch queues and larger batch sizes.
    pub fn is_high_performance(&self) -> bool {
        !matches!(apple_tier(&self.device_name), AppleTier::Base)
    }

    pub fn working_set_mib(&self) -> u64 {
        self.working_set_bytes / (1024 * 1024)
    }

    /// Format: `metal-macos/<device_name>/<working_set>MiB`.
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
