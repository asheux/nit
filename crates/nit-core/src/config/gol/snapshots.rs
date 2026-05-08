//! Disk snapshot policy for the GoL visualizer.
//!
//! The visualizer can persist interesting attractor states (still lifes,
//! oscillators, spaceships) to disk so they can be re-loaded later. This
//! module owns the budget knobs that keep the snapshot directory bounded.

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GolSnapshotsConfig {
    pub enabled: bool,
    /// Hard ceiling on retained snapshot files. When the cap is hit the prune
    /// policy decides which file gets evicted to make room.
    pub max_files: usize,
    pub prune_policy: SnapshotPrunePolicy,
    /// Smallest oscillator period (in generations) worth recording. Shorter
    /// cycles are often grid noise rather than meaningful attractors.
    pub min_period: u32,
    /// Minimum transient length before an attractor is considered stable
    /// enough to snapshot — filters out fly-by configurations.
    pub min_transient: u32,
    /// Wall-clock floor between successive snapshot writes; prevents bursty
    /// I/O on dense seed runs.
    pub min_interval_ms: u64,
    /// Force a snapshot the first time an attractor is detected, regardless
    /// of `min_interval_ms`. Useful for cataloging novel rule discoveries.
    pub snapshot_on_attractor: bool,
}

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum SnapshotPrunePolicy {
    Oldest,
}

impl Default for GolSnapshotsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_files: 500,
            prune_policy: SnapshotPrunePolicy::Oldest,
            min_period: 6,
            min_transient: 20,
            min_interval_ms: 1000,
            snapshot_on_attractor: true,
        }
    }
}
