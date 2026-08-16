//! Multiway engine: a parallel best-first search over a non-confluent rewrite
//! system on content-addressed world states (git commits). Pure and agent-free;
//! the real `GitWorldStore` lives here, the genome+gates `Valuer` adapter in
//! `nit-tui`. See `docs/MULTIWAY.md`.

#![forbid(unsafe_code)]

pub mod edge;
pub mod frontier;
pub mod git_store;
pub mod graph;
pub mod node;
pub mod policy;
pub mod search;
pub mod testing;
pub mod traits;
pub mod value;
pub mod valuer;

// Flat aliases for the nit-tui `Valuer` adapter; other types use module paths.
pub use traits::Valuer;
pub use value::Value;

// Phase-6 streaming-view seam the nit-tui runtime/render layer codes against.
pub use search::{Observer, RunSnapshot};
