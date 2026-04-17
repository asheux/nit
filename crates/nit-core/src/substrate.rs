use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Stable signal id. Format: "{posted_at_gen}-{agent_id}-{counter}".
/// Collision-free under the single-writer `apply()` invariant.
pub type SignalId = String;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    Warning,
    Lead,
    Deadend,
    HelpNeeded,
    ClaimViolation,
    DoneMarker,
}

impl SignalKind {
    pub fn decay_rate(self) -> f32 {
        match self {
            SignalKind::HelpNeeded => 0.5,
            SignalKind::Lead => 0.7,
            SignalKind::Warning => 0.8,
            SignalKind::ClaimViolation => 0.85,
            SignalKind::Deadend => 0.9,
            SignalKind::DoneMarker => 0.95,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignalTarget {
    File { path: PathBuf },
    Agent { agent_id: String },
    Global,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Signal {
    pub id: SignalId,
    pub kind: SignalKind,
    pub posted_by: String,
    pub posted_at_gen: u64,
    pub target: SignalTarget,
    pub initial_strength: f32,
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl Signal {
    pub fn effective_strength(&self, current_gen: u64) -> f32 {
        let delta = current_gen.saturating_sub(self.posted_at_gen) as i32;
        self.initial_strength * self.kind.decay_rate().powi(delta)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SubstrateState {
    pub generation: u64,
    #[serde(default)]
    pub signals: HashMap<SignalId, Signal>,
    #[serde(default)]
    pub claims: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub observations: Vec<serde_json::Value>,
    #[serde(default)]
    pub signal_counter: u64,
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

    pub fn next_signal_id(&mut self, agent_id: &str) -> SignalId {
        let id = format!("{}-{}-{}", self.generation, agent_id, self.signal_counter);
        self.signal_counter = self.signal_counter.saturating_add(1);
        id
    }

    pub fn emit_signal(&mut self, signal: Signal) {
        self.signals.insert(signal.id.clone(), signal);
    }

    pub(crate) fn signals(&self) -> &HashMap<SignalId, Signal> {
        &self.signals
    }
    pub(crate) fn claims(&self) -> &HashMap<String, serde_json::Value> {
        &self.claims
    }
    pub(crate) fn observations(&self) -> &[serde_json::Value] {
        &self.observations
    }

    pub fn signals_iter(&self) -> impl Iterator<Item = (&Signal, f32)> + '_ {
        let gen = self.generation;
        self.signals
            .values()
            .map(move |s| (s, s.effective_strength(gen)))
    }

    /// Signals ordered by effective strength (descending) with a stable
    /// tiebreak on `posted_at_gen` (newest first).
    pub fn signals_sorted_by_strength(&self) -> Vec<(&Signal, f32)> {
        let mut v: Vec<_> = self.signals_iter().collect();
        v.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.0.posted_at_gen.cmp(&a.0.posted_at_gen))
        });
        v
    }

    pub fn signals_by_kind(&self, kind: SignalKind) -> impl Iterator<Item = (&Signal, f32)> + '_ {
        self.signals_iter().filter(move |(s, _)| s.kind == kind)
    }

    pub fn signals_by_target<'a>(
        &'a self,
        target: &'a SignalTarget,
    ) -> impl Iterator<Item = (&'a Signal, f32)> + 'a {
        self.signals_iter().filter(move |(s, _)| &s.target == target)
    }

    pub fn prune_signals_below(&mut self, threshold: f32) -> usize {
        let gen = self.generation;
        let before = self.signals.len();
        self.signals
            .retain(|_, s| s.effective_strength(gen) >= threshold);
        before - self.signals.len()
    }

    fn state_path(workspace_root: &Path) -> PathBuf {
        workspace_root.join(".nit").join("substrate").join("state.json")
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
            nit_utils::fs::ensure_dir(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        nit_utils::fs::write_atomic(&path, |w| w.write_all(&bytes))?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/substrate.rs"]
mod tests;
