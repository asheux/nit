//! Game of Life simulation engine.
//!
//! Rule parsing and catalog lookup, grid evolution, attractor detection,
//! RLE snapshot encoding, background snapshot management, and scored
//! rule evaluation.

#![forbid(unsafe_code)]

mod hash;
mod rle;

pub mod analyze;
pub mod attractor;
pub mod catalog;
pub mod grid;
pub mod rule;
pub mod snapshot;
pub mod snapshot_manager;
pub mod step;

#[cfg(test)]
mod tests;

pub use grid::{EdgeMode, Grid};
pub use rule::{Rule, RuleParseError};

pub use attractor::{AttractorEvent, AttractorExtra, AutoStopPolicy};
pub use catalog::{
    RuleCatalog, RuleDefaultParams, RuleEntry, RuleOverlay, RuleSelectError, SelectedRule,
};
