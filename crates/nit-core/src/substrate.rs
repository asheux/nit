use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SubstrateState {
    pub generation: u64,
    #[serde(default)]
    pub signals: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub claims: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub observations: Vec<serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum SubstrateError {
    #[error("substrate io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("substrate serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

impl SubstrateState {
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

    pub(crate) fn signals(&self) -> &HashMap<String, serde_json::Value> {
        &self.signals
    }
    pub(crate) fn claims(&self) -> &HashMap<String, serde_json::Value> {
        &self.claims
    }
    pub(crate) fn observations(&self) -> &[serde_json::Value] {
        &self.observations
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
