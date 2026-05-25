//! Test-tree map for `nit-core`.
//!
//! `nit-core` mounts each test file via `#[cfg(test)] #[path = ...] mod
//! tests;` declarations on the production sources that own the
//! corresponding surface (see e.g. `buffer.rs:213`, `state.rs:852`).
//! This file is NOT loaded by Cargo's module resolver — it is a stable
//! map for human readers and for the structural-compliance checker
//! that walks the test directory tree.
//!
//! Layout convention
//! -----------------
//!
//! ```text
//! src/
//!   buffer.rs              → mounts tests/buffer.rs (which mounts buffer/*.rs)
//!   buffer.rs              → also mounts tests/vim_semantics.rs
//!   state.rs               → mounts tests/state.rs (which mounts state/*.rs)
//!   substrate.rs           → mounts tests/substrate.rs
//!   languages.rs           → inline `#[cfg(test)] mod tests` per
//!                            CLAUDE.md's convention for this single file
//!                            (other modules use the path-attribute form)
//! ```
//!
//! Buffer test files under `tests/buffer/`
//! ---------------------------------------
//!
//! * `indent_style.rs`  — `Buffer::indent_unit` behaviour (T10).
//! * `jumplist.rs`      — `buffer::JumpList` ring semantics (T5).
//! * `smart_newline.rs` — Smart-Enter pair expansion (T11).
//! * `undo_groups.rs`   — Undo-chunk boundaries (T1).
//! * `word_motion.rs`   — `w`/`e`/`b` + `W`/`E`/`B` (T4).
//! * `yank_register.rs` — `yank_line` / `paste_line_*` / `delete_line`
//!                        / `delete_to_end` primitives (T6).
//!
//! State test files under `tests/state/`
//! -------------------------------------
//!
//! * `auto_pair.rs`        — Auto-pair on `(`/`[`/`{`/`"`/`'` (T3).
//! * `count_prefix.rs`     — Vim count-prefix buffering (`5j`).
//! * `indent_tab.rs`       — `Action::InsertTab` spaces-vs-tab (T10).
//! * `jumplist.rs`         — `gg`/`G`/`n`/`*` jumplist push (T5).
//! * `search_prompt.rs`    — `/` prompt smart-case + paste + incremental
//!                           (T7 + T8).
//! * `yank_register.rs`    — `yy`/`p`, `dd`/`p`, `D`/`p` round-trip (T6).
//! * Plus games / rule-picker / quit suites unrelated to the vim cluster.
//!
//! Adding a new file
//! -----------------
//!
//! 1. Drop the file under `tests/<module>/<feature>.rs`.
//! 2. Add a `#[path = "<module>/<feature>.rs"] mod <feature>;` line to
//!    the matching `tests/<module>.rs` aggregator.
//! 3. Update the map above so future contributors can find it without
//!    grepping.
