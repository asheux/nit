//! `:` command-line dispatcher.
//!
//! Stub: the 1156-line `handle_command_line` match still lives in `state.rs`.
//! It must remain `pub(super)` and reachable by `tests/state.rs` via the
//! existing `#[path]` redirector. Splitting requires breaking the giant
//! match into per-concern handlers (`cmd_substrate`, `cmd_quit`, `cmd_tree`,
//! `cmd_search`, `cmd_run`, `cmd_games`, `cmd_gol`, `cmd_petri`) while
//! preserving 17 callsites in tests. Deferred to a dedicated turn; tracked
//! in the shard's risks JSON.
