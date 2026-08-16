//! nit-tui integration for the `nit-multiway` engine. The pure search engine
//! and the real `GitWorldStore` live in `nit-multiway`; this module supplies the
//! one seam that needs nit-tui's gate machinery — the genome+gates [`Valuer`].
//!
//! Additive and opt-in: the Phase 3 [`runtime::MultiwayRuntime`] runs the engine
//! behind the `NIT_MULTIWAY` flag (default off). With the flag off nothing here
//! is constructed and the existing chat-dispatch path is unchanged.
//!
//! [`Valuer`]: nit_multiway::Valuer

pub mod dispatch;
pub mod executor;
pub mod render;
pub mod runtime;
pub mod valuer;
pub mod view_build;

pub use runtime::{MultiwayEvent, MultiwayRuntime, RunError};
pub use valuer::{GenomeValuer, ValuerError};
pub use view_build::build_multiway_view;

// Integration facade: the `nit-multiway` engine surface this crate drives,
// re-exported so nit-tui call sites (and the Phase 3 runtime wiring) import from
// `crate::multiway` instead of reaching into the engine crate directly. `Value`
// and `Valuer` are what a caller needs to consume `GenomeValuer`; the store types
// and `WorldStore` / `Task` / `NodeId` / `MergeOutcome` are the surface a mission
// constructs and steps.
pub use nit_multiway::git_store::{GitStoreError, GitWorktree, GitWorldStore};
pub use nit_multiway::node::{Node, NodeId, NodeStatus};
pub use nit_multiway::policy::{Budget, Mood, SearchPolicy};
pub use nit_multiway::search::{Engine, RunSnapshot, SearchOutcome, StopReason};
pub use nit_multiway::traits::{MergeOutcome, Task, WorktreeHandle, WorldStore};
pub use nit_multiway::{Value, Valuer};
