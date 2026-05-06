//! Visualizer state — view modes, render modes, rule entries, the
//! `VisualizerState` aggregate, plus seed-overlay cycling and the
//! Hilbert-order inspector helpers (`move_inspector`, `set_inspector_pos`,
//! `hilbert_index_to_xy`, `rot`, `clamp_signed`).
//!
//! Stub: the ~280-line subsystem still lives in `state.rs`.
//! `move_inspector` and `set_inspector_pos` carry a duplicate 6-arm
//! `SeedEncoderId` match; the dedicated turn should extract
//! `inspector_xy_fields(viz, encoder)` to dedupe both call sites.
//! Deferred; tracked in the shard's risks JSON.
