//! Game of Life simulation engine.
//!
//! Provides rule parsing, grid evolution, attractor detection,
//! RLE snapshot encoding, and a scored rule evaluation framework.

#![forbid(unsafe_code)]

pub mod analyze;
pub mod attractor;
pub mod catalog;
pub mod grid;
mod hash;
mod rle;
pub mod rule;
pub mod snapshot;
pub mod snapshot_manager;
pub mod step;

#[cfg(test)]
mod tests;

pub use analyze::{RuleEvaluation, RuleScore};
pub use attractor::{
    AttractorConfig, AttractorDetector, AttractorEvent, AttractorExtra, AutoStopPolicy,
};
pub use catalog::{
    RuleCatalog, RuleDefaultParams, RuleEntry, RuleOverlay, RuleSelectError, SelectedRule,
};
pub use grid::{EdgeMode, Grid};
pub use rule::{Rule, RuleParseError};
pub use snapshot::{SnapshotMetadata, SnapshotPaths};
pub use snapshot_manager::{
    grid_fingerprint, pack_grid_bits, snapshot_queue_capacity, RuleLogEntry, SnapshotEventKind,
    SnapshotManager, SnapshotManagerConfig, SnapshotRequest, SnapshotStats,
};
