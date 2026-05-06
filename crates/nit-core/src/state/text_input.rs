//! `EditorSearch`, `SearchPrompt`, `CommandLine` text-input widgets.
//!
//! Stub: the three single-line text-entry widgets and their shared
//! `char_idx_to_byte` helpers (~120 lines) still live in `state.rs`.
//! Both `SearchPrompt` and `CommandLine` carry byte-identical copies of
//! `char_idx_to_byte`; consolidating them into one free fn next to the
//! widget definitions is the natural cleanup for the dedicated turn.
//! Deferred; tracked in the shard's risks JSON.
