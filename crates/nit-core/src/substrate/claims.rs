//! Claim primitives + the `impl SubstrateState` block that mints,
//! asserts, queries, and expires claims.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::SubstrateState;

/// Stable claim id. Format: `"{claimed_at_gen}-{agent_id}-{counter}"`.
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

/// True if two targets refer to overlapping resources. `Global` overlaps
/// everything; `File` subsumes `Region` on the same path; two `Region`s
/// overlap iff they share a path AND their line ranges intersect.
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

/// True if two claims contend. Same-owner re-assertion is a no-op (the
/// coordination model only tracks contention between different agents).
/// Expiry is the caller's responsibility — this is pure kind-pair plus
/// target-overlap logic. The `matches!` body enumerates the *compatible*
/// kind pairs; the outer negation flips that to "conflict". Conflicting
/// pairs are any pair involving `ExclusiveWrite` (with anything except
/// `Soft`), so listing the small compatible set is shorter than listing
/// every conflicting pair.
pub fn claims_conflict(a: &Claim, b: &Claim) -> bool {
    if !targets_overlap(&a.target, &b.target) {
        return false;
    }
    if a.claimed_by == b.claimed_by {
        return false;
    }
    use ClaimKind::*;
    !matches!(
        (a.kind, b.kind),
        (Soft, _)
            | (_, Soft)
            | (SharedRead, SharedRead)
            | (AppendOnly, AppendOnly)
            | (AppendOnly, SharedRead)
            | (SharedRead, AppendOnly)
    )
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

impl SubstrateState {
    pub fn next_claim_id(&mut self, claimed_by: &str) -> ClaimId {
        let id = format!("{}-{}-{}", self.generation, claimed_by, self.claim_counter);
        self.claim_counter = self.claim_counter.saturating_add(1);
        id
    }

    /// Insert iff no live cross-owner claim conflicts; refresh-on-reassert
    /// drops stale same-owner claims on the same target+kind so repeated
    /// `FileWrite` events don't fan out into a growing pile of
    /// near-duplicate claims (the new claim carries a fresh TTL).
    #[allow(clippy::result_large_err)]
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
        self.claims.retain(|_, existing| {
            !(existing.claimed_by == claim.claimed_by
                && existing.kind == claim.kind
                && targets_overlap(&existing.target, &claim.target))
        });
        self.claims.insert(claim.id.clone(), claim);
        Ok(())
    }

    pub fn claims_iter(&self) -> impl Iterator<Item = &Claim> + '_ {
        let gen = self.generation;
        self.claims.values().filter(move |c| !c.is_expired(gen))
    }

    /// Non-expired claims ordered by remaining generations until TTL
    /// expiry (descending). Tiebreak on `claimed_at_gen` (newest first).
    /// Mirrors `signals_sorted_by_strength`'s stable-sort contract.
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
}
