//! Assumption primitives + the `impl SubstrateState` block that mints,
//! asserts, queries, expires and invalidates assumptions. Assumptions
//! never conflict with each other (read vs read is always safe) but get
//! invalidated when a write lands on an overlapping target.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::claims::{targets_overlap, ClaimTarget};
use super::SubstrateState;

pub type AssumptionId = String;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssumptionTarget {
    File {
        path: PathBuf,
    },
    Region {
        path: PathBuf,
        start_line: u32,
        end_line: u32,
    },
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

    /// True if this assumption's target covers `path`. `Global` covers
    /// everything; `File`/`Region` match on path equality only (line
    /// ranges are not narrowed here because writes invalidate the whole
    /// file scope).
    pub fn applies_to_path(&self, path: &Path) -> bool {
        match &self.target {
            AssumptionTarget::File { path: covered }
            | AssumptionTarget::Region { path: covered, .. } => covered == path,
            AssumptionTarget::Global => true,
        }
    }
}

/// True if an assumption target overlaps a claim target. Bridges the two
/// type hierarchies without collapsing them — same geometry as
/// [`targets_overlap`] for claims.
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

/// True if two assumption targets overlap. Same geometry as
/// [`targets_overlap`] for claims.
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
        self.assumptions
            .values()
            .filter(move |a| !a.is_expired(gen))
    }

    /// Non-expired assumptions ordered by remaining generations until TTL
    /// expiry (descending). Tiebreak on `posted_at_gen` (newest first).
    /// Mirrors `claims_sorted_by_remaining_ttl`'s stable-sort contract.
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
        self.assumptions_iter()
            .filter(move |a| a.applies_to_path(path))
    }

    pub fn expire_assumptions(&mut self, current_gen: u64) -> usize {
        let before = self.assumptions.len();
        self.assumptions.retain(|_, a| !a.is_expired(current_gen));
        before - self.assumptions.len()
    }

    /// Removes all non-expired assumptions whose target overlaps the
    /// given written path, returning them so callers can emit
    /// diagnostics. Called from the `FileWrite` branch of the agent_bus
    /// reducer.
    pub fn invalidate_assumptions_for_write(&mut self, path: &Path) -> Vec<Assumption> {
        let gen = self.generation;
        let ids_to_remove: Vec<AssumptionId> = self
            .assumptions
            .iter()
            .filter(|(_, a)| !a.is_expired(gen) && a.applies_to_path(path))
            .map(|(id, _)| id.clone())
            .collect();
        ids_to_remove
            .into_iter()
            .filter_map(|id| self.assumptions.remove(&id))
            .collect()
    }
}
