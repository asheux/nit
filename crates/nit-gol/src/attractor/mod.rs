//! Attractor detection: grid fingerprints track fixed points and cycles.
//!
//! Protocol-aware mode mixes an extra context word into the fingerprint
//! so identical grids observed across distinct phases do not collapse
//! into spurious repeats.
//!
//! The byte order emitted by `feed_fingerprint_payload` and the domain
//! tags below are stability-critical: altering either invalidates
//! persisted attractor history and on-disk snapshot dedup keys, and
//! must be paired with a domain-tag version bump.

use std::fmt;

mod detector;

pub use detector::{AttractorConfig, AttractorDetector};

use crate::hash::{blake3_u64_pair, edge_tag, fnv1a, FNV_OFFSET};
use crate::{EdgeMode, Grid, Rule};

const FINGERPRINT_DOMAIN: &[u8] = b"nit-gol-attractor-v1";
const PROTOCOL_TAG: &[u8] = b"proto";

/// Extra context folded into the grid fingerprint during protocol-aware runs.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttractorExtra {
    pub protocol_hash: u64,
    pub phase_idx: u32,
    /// Generations elapsed since the current phase began.
    pub step_in_phase: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AttractorEvent {
    /// Grid evolved into itself — a period-1 attractor.
    FixedPoint { gen: u64 },
    /// A previously observed state reappeared.
    Cycle {
        gen: u64,
        first_seen: u64,
        period: u64,
        transient: u64,
    },
}

/// Controls whether detector events halt the simulation.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AutoStopPolicy {
    Off,
    Fixed,
    Repeat,
}

impl AutoStopPolicy {
    pub fn should_stop(self, event: &AttractorEvent) -> bool {
        match self {
            Self::Off => false,
            Self::Fixed => matches!(event, AttractorEvent::FixedPoint { .. }),
            Self::Repeat => true,
        }
    }

    /// Advance to the next policy for the UI round-robin toggle.
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Fixed,
            Self::Fixed => Self::Repeat,
            Self::Repeat => Self::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Fixed => "Fixed",
            Self::Repeat => "Repeat",
        }
    }
}

impl fmt::Display for AutoStopPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Two-word blake3-based fingerprint identifying a grid state.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Fingerprint([u64; 2]);

/// Compute the primary blake3 fingerprint, and optionally an FNV-1a
/// secondary hash used as a collision-confirmation guard.
pub(crate) fn compute_fingerprint(
    grid: &Grid,
    rule: Rule,
    edge: EdgeMode,
    extra: Option<AttractorExtra>,
    include_secondary: bool,
) -> (Fingerprint, Option<u64>) {
    let mut blake = blake3::Hasher::new();
    blake.update(FINGERPRINT_DOMAIN);
    let mut fnv = FNV_OFFSET;
    feed_fingerprint_payload(grid, rule, edge, extra, |bytes| {
        blake.update(bytes);
        if include_secondary {
            fnv = fnv1a(fnv, bytes);
        }
    });
    let primary = Fingerprint(blake3_u64_pair(&blake.finalize()));
    let secondary = include_secondary.then_some(fnv);
    (primary, secondary)
}

fn feed_fingerprint_payload(
    grid: &Grid,
    rule: Rule,
    edge: EdgeMode,
    extra: Option<AttractorExtra>,
    mut push: impl FnMut(&[u8]),
) {
    push(&grid.width().to_le_bytes());
    push(&grid.height().to_le_bytes());
    push(&rule.births_mask().to_le_bytes());
    push(&rule.survives_mask().to_le_bytes());
    push(&[edge_tag(edge)]);
    if let Some(ctx) = extra {
        push(PROTOCOL_TAG);
        push(&ctx.protocol_hash.to_le_bytes());
        push(&ctx.phase_idx.to_le_bytes());
        push(&ctx.step_in_phase.to_le_bytes());
    }
    push(grid.cells());
}

#[cfg(test)]
impl Fingerprint {
    pub(crate) fn from_u128(value: u128) -> Self {
        let bytes = value.to_le_bytes();
        let lo = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let hi = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        Fingerprint([lo, hi])
    }
}

#[cfg(test)]
#[path = "../test_modules/attractor.rs"]
mod tests;
