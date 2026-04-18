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
    /// Default effective-strength computation using the signal kind's
    /// native decay rate — equivalent to `_with_multiplier(..., 1.0)`.
    /// Kept for backward compatibility with callers that don't care about
    /// the mood-adjusted rate.
    pub fn effective_strength(&self, current_gen: u64) -> f32 {
        self.effective_strength_with_multiplier(current_gen, 1.0)
    }

    /// Mood-aware effective strength. `multiplier` scales how quickly the
    /// signal fades: values >1 accelerate decay (fade faster), <1 slow
    /// it (preserve longer). This maps the Mood semantics directly —
    /// Exploration (1.1) forgets faster, Defensive (0.85) holds on longer.
    ///
    /// Internally, the kind's native decay rate is *divided* by the
    /// multiplier so the per-generation retention ratio moves toward 1.0
    /// as the multiplier shrinks. The result is clamped to (0.01, 0.999)
    /// so decay remains well-defined regardless of how extreme the mood
    /// modulation becomes.
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

/// Stable claim id. Format: "{claimed_at_gen}-{agent_id}-{counter}".
/// Collision-free under the single-writer `apply()` invariant.
pub type ClaimId = String;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimKind {
    ExclusiveWrite,
    SharedRead,
    AppendOnly,
    Soft,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClaimTarget {
    File { path: PathBuf },
    Region { path: PathBuf, start_line: u32, end_line: u32 },
    Global,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Claim {
    pub id: ClaimId,
    pub kind: ClaimKind,
    pub target: ClaimTarget,
    pub claimed_by: String,
    pub claimed_at_gen: u64,
    pub ttl_gens: u64,
    pub rationale: String,
}

impl Claim {
    pub fn is_expired(&self, current_gen: u64) -> bool {
        current_gen >= self.claimed_at_gen.saturating_add(self.ttl_gens)
    }
}

/// Returns true if two targets refer to overlapping resources.
pub fn targets_overlap(a: &ClaimTarget, b: &ClaimTarget) -> bool {
    use ClaimTarget::*;
    match (a, b) {
        (Global, _) | (_, Global) => true,
        (File { path: p1 }, File { path: p2 }) => p1 == p2,
        (File { path: p1 }, Region { path: p2, .. })
        | (Region { path: p1, .. }, File { path: p2 }) => p1 == p2,
        (
            Region {
                path: p1,
                start_line: s1,
                end_line: e1,
            },
            Region {
                path: p2,
                start_line: s2,
                end_line: e2,
            },
        ) => p1 == p2 && s1 <= e2 && s2 <= e1,
    }
}

/// Returns true if two claims conflict.  Expiry is the caller's
/// responsibility — this is pure kind-pair + target-overlap logic.
pub fn claims_conflict(a: &Claim, b: &Claim) -> bool {
    if !targets_overlap(&a.target, &b.target) {
        return false;
    }
    use ClaimKind::*;
    match (a.kind, b.kind) {
        (Soft, _) | (_, Soft) => false,
        (SharedRead, SharedRead) => false,
        (AppendOnly, AppendOnly) => false,
        (AppendOnly, SharedRead) | (SharedRead, AppendOnly) => false,
        _ => true, // any ExclusiveWrite, or Shared×Exclusive, Append×Exclusive
    }
}

#[derive(Debug, Clone)]
pub struct ClaimConflict {
    pub attempted: Claim,
    pub conflicts: Vec<Claim>,
}

impl std::fmt::Display for ClaimConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "claim conflict: {} existing claim(s) overlap",
            self.conflicts.len()
        )
    }
}

impl std::error::Error for ClaimConflict {}

pub type AssumptionId = String;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssumptionTarget {
    File { path: PathBuf },
    Region { path: PathBuf, start_line: u32, end_line: u32 },
    Global,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Assumption {
    pub id: AssumptionId,
    pub target: AssumptionTarget,
    pub fact: serde_json::Value,
    pub posted_by: String,
    pub posted_at_gen: u64,
    pub ttl_gens: u64,
    pub rationale: String,
}

impl Assumption {
    pub fn is_expired(&self, current_gen: u64) -> bool {
        current_gen >= self.posted_at_gen.saturating_add(self.ttl_gens)
    }
}

/// Returns true if an assumption target overlaps a claim target.
/// Bridges the two type hierarchies without collapsing them.
pub fn assumption_target_overlaps_claim(a: &AssumptionTarget, b: &ClaimTarget) -> bool {
    let a_as_claim = match a {
        AssumptionTarget::File { path } => ClaimTarget::File { path: path.clone() },
        AssumptionTarget::Region {
            path,
            start_line,
            end_line,
        } => ClaimTarget::Region {
            path: path.clone(),
            start_line: *start_line,
            end_line: *end_line,
        },
        AssumptionTarget::Global => ClaimTarget::Global,
    };
    targets_overlap(&a_as_claim, b)
}

/// Returns true if two assumption targets overlap. Uses the same
/// geometry as `targets_overlap` for claims.
pub fn assumption_targets_overlap(a: &AssumptionTarget, b: &AssumptionTarget) -> bool {
    use AssumptionTarget::*;
    match (a, b) {
        (Global, _) | (_, Global) => true,
        (File { path: p1 }, File { path: p2 }) => p1 == p2,
        (File { path: p1 }, Region { path: p2, .. })
        | (Region { path: p1, .. }, File { path: p2 }) => p1 == p2,
        (
            Region {
                path: p1,
                start_line: s1,
                end_line: e1,
            },
            Region {
                path: p2,
                start_line: s2,
                end_line: e2,
            },
        ) => p1 == p2 && s1 <= e2 && s2 <= e1,
    }
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
        self.signals_iter().filter(move |(s, _)| &s.target == target)
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
    /// within the last `gens` generations.  Used by mood auto-transition.
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

    pub fn next_claim_id(&mut self, claimed_by: &str) -> ClaimId {
        let id = format!("{}-{}-{}", self.generation, claimed_by, self.claim_counter);
        self.claim_counter = self.claim_counter.saturating_add(1);
        id
    }

    pub fn assert_claim(&mut self, claim: Claim) -> Result<(), ClaimConflict> {
        let current_gen = self.generation;
        let conflicts: Vec<Claim> = self
            .claims
            .values()
            .filter(|existing| !existing.is_expired(current_gen))
            .filter(|existing| existing.id != claim.id)
            .filter(|existing| claims_conflict(&claim, existing))
            .cloned()
            .collect();
        if !conflicts.is_empty() {
            return Err(ClaimConflict {
                attempted: claim,
                conflicts,
            });
        }
        self.claims.insert(claim.id.clone(), claim);
        Ok(())
    }

    pub fn claims_iter(&self) -> impl Iterator<Item = &Claim> + '_ {
        let gen = self.generation;
        self.claims.values().filter(move |c| !c.is_expired(gen))
    }

    /// Non-expired claims ordered by remaining generations until TTL expiry
    /// (descending). Tiebreak on `claimed_at_gen` (newest first). Mirrors
    /// `signals_sorted_by_strength` — same stable-sort contract.
    pub fn claims_sorted_by_remaining_ttl(&self) -> Vec<(&Claim, u64)> {
        let current_gen = self.generation;
        let mut v: Vec<_> = self
            .claims
            .values()
            .filter(|c| !c.is_expired(current_gen))
            .map(|c| {
                let expiry_gen = c.claimed_at_gen.saturating_add(c.ttl_gens);
                let remaining = expiry_gen.saturating_sub(current_gen);
                (c, remaining)
            })
            .collect();
        // Most-remaining-TTL first; tiebreak by claimed_at_gen descending
        // (newest first).
        v.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then(b.0.claimed_at_gen.cmp(&a.0.claimed_at_gen))
        });
        v
    }

    pub fn claims_for_path<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a Claim> + 'a {
        self.claims_iter().filter(move |c| match &c.target {
            ClaimTarget::File { path: p } => p == path,
            ClaimTarget::Region { path: p, .. } => p == path,
            ClaimTarget::Global => true,
        })
    }

    pub fn expire_claims(&mut self, current_gen: u64) -> usize {
        let before = self.claims.len();
        self.claims.retain(|_, c| !c.is_expired(current_gen));
        before - self.claims.len()
    }

    pub fn next_assumption_id(&mut self, posted_by: &str) -> AssumptionId {
        let id = format!(
            "{}-{}-{}",
            self.generation, posted_by, self.assumption_counter
        );
        self.assumption_counter = self.assumption_counter.saturating_add(1);
        id
    }

    /// Infallible insert. Assumptions don't form a lattice (read-vs-read
    /// never conflicts); they can coexist freely.
    pub fn assert_assumption(&mut self, assumption: Assumption) {
        self.assumptions.insert(assumption.id.clone(), assumption);
    }

    pub fn assumptions_iter(&self) -> impl Iterator<Item = &Assumption> + '_ {
        let gen = self.generation;
        self.assumptions.values().filter(move |a| !a.is_expired(gen))
    }

    /// Non-expired assumptions ordered by remaining generations until TTL
    /// expiry (descending). Tiebreak on `posted_at_gen` (newest first).
    /// Mirrors `claims_sorted_by_remaining_ttl` — same stable-sort contract.
    pub fn assumptions_sorted_by_remaining_ttl(&self) -> Vec<(&Assumption, u64)> {
        let current_gen = self.generation;
        let mut v: Vec<_> = self
            .assumptions
            .values()
            .filter(|a| !a.is_expired(current_gen))
            .map(|a| {
                let expiry_gen = a.posted_at_gen.saturating_add(a.ttl_gens);
                let remaining = expiry_gen.saturating_sub(current_gen);
                (a, remaining)
            })
            .collect();
        v.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then(b.0.posted_at_gen.cmp(&a.0.posted_at_gen))
        });
        v
    }

    pub fn assumptions_for_path<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl Iterator<Item = &'a Assumption> + 'a {
        self.assumptions_iter().filter(move |a| match &a.target {
            AssumptionTarget::File { path: p } => p == path,
            AssumptionTarget::Region { path: p, .. } => p == path,
            AssumptionTarget::Global => true,
        })
    }

    pub fn expire_assumptions(&mut self, current_gen: u64) -> usize {
        let before = self.assumptions.len();
        self.assumptions
            .retain(|_, a| !a.is_expired(current_gen));
        before - self.assumptions.len()
    }

    /// Removes all non-expired assumptions whose target overlaps the given
    /// written path, and returns them so callers can emit diagnostics.
    pub fn invalidate_assumptions_for_write(&mut self, path: &Path) -> Vec<Assumption> {
        let gen = self.generation;
        let ids_to_remove: Vec<AssumptionId> = self
            .assumptions
            .iter()
            .filter(|(_, a)| !a.is_expired(gen))
            .filter(|(_, a)| match &a.target {
                AssumptionTarget::File { path: p } => p == path,
                AssumptionTarget::Region { path: p, .. } => p == path,
                AssumptionTarget::Global => true,
            })
            .map(|(id, _)| id.clone())
            .collect();
        ids_to_remove
            .into_iter()
            .filter_map(|id| self.assumptions.remove(&id))
            .collect()
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
