//! Tunable thresholds for the recommendation analyzers. Centralized so that
//! tuning runs read one file and so cross-analyzer constants like
//! `RANGE_GAP` (shared between nesting and entropy) live in a single place.

pub(super) const DENSITY_LOW: f32 = 0.15;
pub(super) const MONOLITHIC_COMPONENTS: usize = 3;

pub(super) const CYCLOMATIC_CRITICAL: u32 = 10;
pub(super) const IDENTIFIER_UNIQUENESS_MIN: f32 = 0.5;

pub(super) const NESTING_DEPTH_WARN: usize = 4;

pub(super) const ENTROPY_WINDOW_LINES: usize = 10;
pub(super) const ENTROPY_MIN_TOKENS: usize = 5;
pub(super) const ENTROPY_LOW: f32 = 3.0;

/// Allowed gap (in lines) when coalescing contiguous flagged ranges into a
/// single recommendation. Used by both nesting and entropy analyzers so a
/// 1-2 line breather inside an otherwise-deep block doesn't fragment the
/// warning into many small ranges.
pub(super) const RANGE_GAP: usize = 2;
