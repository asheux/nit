#![forbid(unsafe_code)]

pub mod analyze;
pub mod grid;
pub mod rule;
pub mod snapshot;
pub mod step;

#[cfg(test)]
mod tests;

pub use analyze::{RuleEvaluation, RuleScore};
pub use grid::{EdgeMode, Grid};
pub use rule::{Rule, RuleParseError};
pub use snapshot::{SnapshotMetadata, SnapshotPaths};
