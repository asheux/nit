//! Companion reference for the genome_storage test directory. NOT loaded
//! as a Rust module — production source `crates/nit-core/src/genome_storage.rs`
//! declares the test mod via `#[path = "tests/genome_storage.rs"] mod
//! tests;`. This sibling file documents what each sub-file owns and the
//! shared fixture constants the gc / histograms / round_trip tests reach
//! for, so a future test reorganisation can read one place.

#![allow(dead_code)]

pub(super) const SAMPLE_REPORT_TIMESTAMP_MS: u64 = 1_700_000_000_000;
pub(super) const SAMPLE_REPORT_GRID_SIZE: usize = 32;
pub(super) const SAMPLE_REPORT_CONSISTENCY: f32 = 0.5;

pub(super) const FORGED_REPORT_AGE_DAYS: u64 = 4;
pub(super) const FORGED_REPORT_FRACTION_OF_CAP: u64 = 3;

pub(super) const TEST_WORKSPACE_PREFIXES: &[&str] = &[
    "genome-roundtrip",
    "genome-delete",
    "genome-partial",
    "genome-shard",
    "genome-gc-age",
    "genome-gc-bytes",
    "genome-gc-legacy",
    "genome-collision",
];

pub(super) struct SubModuleDescriptor {
    pub file: &'static str,
    pub focus: &'static str,
}

pub(super) const SUBMODULES: &[SubModuleDescriptor] = &[
    SubModuleDescriptor {
        file: "round_trip.rs",
        focus: "persist + load + delete + temp-sidecar + flattened-path collision",
    },
    SubModuleDescriptor {
        file: "gc.rs",
        focus: "age-based eviction, byte-ceiling trim, legacy layout removal",
    },
    SubModuleDescriptor {
        file: "histograms.rs",
        focus: "tier histogram + count-at-or-above ladder positions",
    },
];
