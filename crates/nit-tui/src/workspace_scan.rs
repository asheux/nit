//! Workspace-wide genome scan driver. The runtime hydrates the on-disk
//! cache at launch, defers the expensive walk to an explicit operator
//! click, and pumps an adaptive in-flight queue of [`GenomeWorker`]
//! threads keyed off `available_parallelism()`.

mod runtime;
mod scope;
mod util;

pub use runtime::WorkspaceScanRuntime;
pub use scope::{
    is_code_file, workspace_scan_max_in_flight, WorkspaceScanItemState, WORKSPACE_SCAN_MAX_CAP,
    WORKSPACE_SCAN_MIN_CAP,
};

#[cfg(test)]
pub use util::forget_report;

#[cfg(test)]
#[path = "tests/workspace_scan.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/workspace_scan_events.rs"]
mod tests_events;
