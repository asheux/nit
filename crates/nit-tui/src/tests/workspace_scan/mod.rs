//! Companion reference for the workspace_scan test directory. NOT loaded
//! as a Rust module — production source
//! `crates/nit-tui/src/workspace_scan.rs` declares the test mod via
//! `#[path = "tests/workspace_scan.rs"] mod tests;`. This sibling file
//! documents what each sub-file covers + the shared fixture constants
//! every sub-file reaches for via the parent's helpers.

#![allow(dead_code)]

pub(super) const TEMP_DIR_PREFIX: &str = "nit-ws-scan";
pub(super) const DEFAULT_DRAIN_TIMEOUT_SECS: u64 = 30;
pub(super) const DEFAULT_SCAN_CAP_SMALL: usize = 2;
pub(super) const DEFAULT_SCAN_CAP_DEFAULT: usize = 3;
pub(super) const PROBE_FILE_COUNT_LARGE: usize = 16;
pub(super) const PROBE_FILE_COUNT_MEDIUM: usize = 6;
pub(super) const PROBE_FILE_COUNT_SMALL: usize = 4;

pub(super) struct WorkspaceScanSubModule {
    pub file: &'static str,
    pub focus: &'static str,
}

pub(super) const WORKSPACE_SCAN_SUBMODULES: &[WorkspaceScanSubModule] = &[
    WorkspaceScanSubModule {
        file: "hydrate.rs",
        focus: "disk-cache hydrate / re-eval / idempotent / single-file launch",
    },
    WorkspaceScanSubModule {
        file: "change_events.rs",
        focus: "external edits / delete events / repeat-event de-dup",
    },
    WorkspaceScanSubModule {
        file: "filters.rs",
        focus: "ignored_dirs + non_source_extensions + hidden_dot + gitignored_dirs",
    },
    WorkspaceScanSubModule {
        file: "gc.rs",
        focus: "phantom report purge + external deletion + delete-event cache clear",
    },
    WorkspaceScanSubModule {
        file: "scheduling.rs",
        focus: "many-file end-to-end + drive cap + max_in_flight + is_code_file probe",
    },
    WorkspaceScanSubModule {
        file: "queue.rs",
        focus: "in-flight snapshot partition Evaluating / Queued + idle-empty",
    },
    WorkspaceScanSubModule {
        file: "runner_completions.rs",
        focus: "worker_result workspace_scan flag + backfill drain + fresh-runtime",
    },
    WorkspaceScanSubModule {
        file: "cache_lifecycle.rs",
        focus: "persist-then-delete + gate_monitor sub-view tab clicks",
    },
];
