//! `apply_action` dispatcher and per-category helpers.
//!
//! Stub: the 1030-line `apply_action` 142-arm match still lives in `state.rs`.
//! Splitting requires factoring per-category handlers (buffer-motion,
//! visualizer, yank/paste, search, command-prompt, games) while preserving
//! exact dispatch semantics for every Action variant. Deferred to a
//! dedicated turn; tracked in the shard's risks JSON.
