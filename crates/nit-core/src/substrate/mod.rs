//! Substrate state — signals, claims, assumptions, mood, and on-disk
//! `.nit/substrate/state.json` snapshots.
//!
//! Single-writer invariant: id minting (`next_signal_id`,
//! `next_claim_id`, `next_assumption_id`) is concentrated on
//! `SubstrateState` so the `agent_bus::AgentBusEvent::apply` reducer can
//! mint without races. The on-disk and JSON-RPC wire shapes (consumed by
//! `nit-mcp`) are pinned by serde tags on `SignalTarget`, `ClaimTarget`,
//! `AssumptionTarget` — do not reshape without a coordinated migration.
//!
//! The methods on `SubstrateState` live alongside their domain types:
//! signal mint/emit/query in [`signals`], claim mint/assert/expire in
//! [`claims`], assumption mint/assert/invalidate in [`assumptions`].
//! Only state-lifecycle (new, advance, persistence) lives here.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub mod assumptions;
pub mod claims;
pub mod signals;

pub use assumptions::{
    assumption_target_overlaps_claim, assumption_targets_overlap, Assumption, AssumptionId,
    AssumptionTarget,
};
pub use claims::{
    claims_conflict, targets_overlap, Claim, ClaimConflict, ClaimId, ClaimKind, ClaimTarget,
};
pub use signals::{Signal, SignalId, SignalKind, SignalTarget};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SubstrateState {
    pub generation: u64,
    #[serde(default)]
    pub signals: HashMap<SignalId, Signal>,
    #[serde(default)]
    pub claims: HashMap<ClaimId, Claim>,
    #[serde(default)]
    pub observations: Vec<serde_json::Value>,
    #[serde(default)]
    pub signal_counter: u64,
    #[serde(default)]
    pub claim_counter: u64,
    #[serde(default)]
    pub assumptions: HashMap<AssumptionId, Assumption>,
    #[serde(default)]
    pub assumption_counter: u64,
    #[serde(default)]
    pub mood: crate::mood::Mood,
    #[serde(default)]
    pub mood_override_until_gen: u64,
    #[serde(default)]
    pub mood_quiet_streak: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum SubstrateError {
    #[error("substrate io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("substrate serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

impl SubstrateState {
    pub const DEFAULT_PRUNE_THRESHOLD: f32 = 0.05;
    pub const DEFAULT_INITIAL_STRENGTH: f32 = 1.0;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn current_generation(&self) -> u64 {
        self.generation
    }

    pub fn advance_generation(&mut self) -> u64 {
        self.generation = self.generation.saturating_add(1);
        self.generation
    }

    fn state_path(workspace_root: &Path) -> PathBuf {
        workspace_root
            .join(".nit")
            .join("substrate")
            .join("state.json")
    }

    /// Tolerant load: missing or corrupt file returns `Default`.
    pub fn load(workspace_root: &Path) -> Self {
        let path = Self::state_path(workspace_root);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, workspace_root: &Path) -> Result<(), SubstrateError> {
        let path = Self::state_path(workspace_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        nit_utils::fs::write_atomic(&path, |w| w.write_all(&bytes))?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/substrate.rs"]
mod tests;
