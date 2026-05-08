//! Cross-mission structural memory.
//!
//! Indexes mission metadata + task summaries from `.nit/swarm/` and
//! retrieves similar past missions at planner time. Pure keyword-based
//! retrieval — no embeddings, no new deps.
//!
//! On-disk persistence layout (`<workspace>/.nit/memory/index.json`)
//! and the serde fields on [`IndexedMission`] / [`MissionMemoryIndex`]
//! are part of the inter-version contract; do not reshape without a
//! coordinated migration.

use serde::{Deserialize, Serialize};

pub mod index;
pub mod io;
pub mod search;

pub use index::{build_index, load_or_build, upsert_mission};
pub use io::{load_index, save_index};
pub use search::{path_tokens, retrieve_similar};

// Internal helpers re-exported at the module root so the centralized
// tests in `tests/mission_memory.rs` can reach them via `use super::*;`.
#[cfg(test)]
pub(crate) use search::{idf_weight, tokenize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexedMission {
    pub mission_id: String,
    pub title: String,
    pub template: String,
    pub status: String,
    pub updated_at: String,
    pub task_ids: Vec<String>,
    pub task_titles: Vec<String>,
    pub task_summaries: Vec<String>,
    pub files_touched: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MissionMemoryIndex {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub missions: Vec<IndexedMission>,
}

fn default_version() -> u32 {
    1
}

#[derive(Clone, Debug)]
pub struct MissionHit {
    pub mission: IndexedMission,
    pub score: f32,
}

#[cfg(test)]
#[path = "../tests/mission_memory.rs"]
mod tests;
