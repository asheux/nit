//! Signal primitives + the `impl SubstrateState` block that mints,
//! emits, queries, prunes and rolls up signals.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::SubstrateState;

/// Stable signal id. Format: `"{posted_at_gen}-{agent_id}-{counter}"`.
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
    InterventionEmitted,
}

impl SignalKind {
    pub fn decay_rate(self) -> f32 {
        match self {
            SignalKind::HelpNeeded => 0.5,
            SignalKind::Lead => 0.7,
            SignalKind::Warning => 0.8,
            SignalKind::ClaimViolation => 0.85,
            SignalKind::Deadend => 0.9,
            SignalKind::InterventionEmitted => 0.9,
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
    /// Effective strength using the kind's native decay rate. Equivalent
    /// to `_with_multiplier(..., 1.0)` — kept for callers that don't need
    /// the mood-adjusted rate.
    pub fn effective_strength(&self, current_gen: u64) -> f32 {
        self.effective_strength_with_multiplier(current_gen, 1.0)
    }

    /// Mood-aware effective strength. `multiplier > 1` accelerates decay
    /// (Exploration mood forgets faster), `< 1` slows it (Defensive holds
    /// on longer). Internally the kind's native decay rate is *divided*
    /// by `multiplier`, so per-generation retention moves toward 1.0 as
    /// the multiplier shrinks. The clamp `(0.01, 0.999)` keeps decay
    /// well-defined regardless of how extreme the modulation becomes.
    pub fn effective_strength_with_multiplier(&self, current_gen: u64, multiplier: f32) -> f32 {
        let delta = current_gen.saturating_sub(self.posted_at_gen) as i32;
        let safe_multiplier = if multiplier.abs() < f32::EPSILON {
            1.0
        } else {
            multiplier
        };
        let effective_rate = self.kind.decay_rate() / safe_multiplier;
        let clamped = effective_rate.clamp(0.01, 0.999);
        self.initial_strength * clamped.powi(delta)
    }
}

impl SubstrateState {
    pub fn next_signal_id(&mut self, agent_id: &str) -> SignalId {
        let id = format!("{}-{}-{}", self.generation, agent_id, self.signal_counter);
        self.signal_counter = self.signal_counter.saturating_add(1);
        id
    }

    pub fn emit_signal(&mut self, signal: Signal) {
        self.signals.insert(signal.id.clone(), signal);
    }

    pub fn signals_iter(&self) -> impl Iterator<Item = (&Signal, f32)> + '_ {
        let gen = self.generation;
        let multiplier = self.mood.modulation().signal_decay_multiplier;
        self.signals
            .values()
            .map(move |s| (s, s.effective_strength_with_multiplier(gen, multiplier)))
    }

    /// Signals ordered by effective strength (descending) with a stable
    /// tiebreak on `posted_at_gen` (newest first). Reads the mood-adjusted
    /// decay multiplier so Defensive preserves warnings longer and
    /// Exploration forgets faster.
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
        self.signals_iter()
            .filter(move |(s, _)| &s.target == target)
    }

    pub fn prune_signals_below(&mut self, threshold: f32) -> usize {
        let gen = self.generation;
        let multiplier = self.mood.modulation().signal_decay_multiplier;
        let before = self.signals.len();
        self.signals
            .retain(|_, s| s.effective_strength_with_multiplier(gen, multiplier) >= threshold);
        before - self.signals.len()
    }

    /// Count of `ClaimViolation` + `Warning` + `HelpNeeded` signals posted
    /// within the last `gens` generations. Used by mood auto-transition.
    pub fn pressure_in_window(&self, gens: u64) -> usize {
        let current_gen = self.generation;
        let window_start = current_gen.saturating_sub(gens);
        self.signals
            .values()
            .filter(|s| s.posted_at_gen >= window_start)
            .filter(|s| {
                matches!(
                    s.kind,
                    SignalKind::ClaimViolation | SignalKind::Warning | SignalKind::HelpNeeded
                )
            })
            .count()
    }
}
