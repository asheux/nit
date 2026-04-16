//! Attractor detection for Game of Life simulations.
//!
//! Detects fixed points and periodic cycles during grid evolution by
//! maintaining a fingerprint history of observed states. Supports both
//! simple and protocol-aware (multi-phase) observation.

use std::collections::{HashMap, VecDeque};

use crate::hash::{blake3_u64_pair, edge_tag, fnv1a, FNV_OFFSET};
use crate::{EdgeMode, Grid, Rule};

/// Additional context for protocol-aware attractor detection.
///
/// When a simulation runs a multi-phase protocol, these fields are
/// mixed into the grid fingerprint so that identical grids in
/// different protocol states are not mistaken for a cycle.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttractorExtra {
    /// Stable digest of the protocol definition.
    pub protocol_hash: u64,
    /// Zero-based index of the active phase within the protocol.
    pub phase_idx: u32,
    /// Generations elapsed since this phase began.
    pub step_in_phase: u32,
}

/// Two-word blake3-based fingerprint for grid identity.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Fingerprint([u64; 2]);

/// Events emitted when an attractor is detected.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AttractorEvent {
    /// The grid is identical to its successor (period-1 attractor).
    FixedPoint { gen: u64 },
    /// A previously seen state was observed again.
    Cycle {
        gen: u64,
        first_seen: u64,
        period: u64,
        transient: u64,
    },
}

/// Policy controlling whether the simulation auto-stops on attractors.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AutoStopPolicy {
    /// Never auto-stop.
    Off,
    /// Stop only on fixed points.
    Fixed,
    /// Stop on any repeat (fixed point or cycle).
    Repeat,
}

impl AutoStopPolicy {
    /// Returns `true` if the given event should halt the simulation.
    pub fn should_stop(self, event: &AttractorEvent) -> bool {
        match self {
            AutoStopPolicy::Off => false,
            AutoStopPolicy::Fixed => matches!(event, AttractorEvent::FixedPoint { .. }),
            AutoStopPolicy::Repeat => true,
        }
    }

    /// Cycle to the next policy variant in round-robin order.
    pub fn next(self) -> Self {
        match self {
            AutoStopPolicy::Off => AutoStopPolicy::Fixed,
            AutoStopPolicy::Fixed => AutoStopPolicy::Repeat,
            AutoStopPolicy::Repeat => AutoStopPolicy::Off,
        }
    }

    /// Human-readable label for display.
    pub fn label(self) -> &'static str {
        match self {
            AutoStopPolicy::Off => "Off",
            AutoStopPolicy::Fixed => "Fixed",
            AutoStopPolicy::Repeat => "Repeat",
        }
    }
}

impl std::fmt::Display for AutoStopPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Configuration for the attractor detector.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AttractorConfig {
    /// Which events trigger an auto-stop.
    pub policy: AutoStopPolicy,
    /// Maximum number of fingerprint entries kept in history.
    pub max_history: usize,
    /// Require a secondary hash match to confirm cycle detection.
    pub confirm_on_repeat: bool,
}

impl Default for AttractorConfig {
    fn default() -> Self {
        Self {
            policy: AutoStopPolicy::Fixed,
            max_history: 20_000,
            confirm_on_repeat: true,
        }
    }
}

/// Internal record of a previously observed fingerprint.
#[derive(Clone, Debug)]
struct SeenEntry {
    first_seen: u64,
    secondary: Option<u64>,
}

/// Stateful detector that tracks grid fingerprints across generations.
///
/// Feed grids via [`seed`](Self::seed) and [`observe`](Self::observe);
/// the detector returns an [`AttractorEvent`] when a fixed point or
/// cycle is found.
pub struct AttractorDetector {
    cfg: AttractorConfig,
    seen: HashMap<Fingerprint, Vec<SeenEntry>>,
    order: VecDeque<(Fingerprint, u64)>,
    seen_entries: usize,
    last: Option<Fingerprint>,
    seeded: bool,
    completed: bool,
}

impl AttractorDetector {
    pub fn new(cfg: AttractorConfig) -> Self {
        Self {
            cfg,
            seen: HashMap::new(),
            order: VecDeque::new(),
            seen_entries: 0,
            last: None,
            seeded: false,
            completed: false,
        }
    }

    pub fn config(&self) -> &AttractorConfig {
        &self.cfg
    }

    pub fn last_fingerprint(&self) -> Option<Fingerprint> {
        self.last
    }

    pub fn set_policy(&mut self, policy: AutoStopPolicy) {
        self.cfg.policy = policy;
    }

    /// Clear all history and reset to the initial state.
    pub fn reset(&mut self) {
        self.seen.clear();
        self.order.clear();
        self.seen_entries = 0;
        self.last = None;
        self.seeded = false;
        self.completed = false;
    }

    /// Register the initial grid state before observation begins.
    pub fn seed(&mut self, grid: &Grid, gen: u64, rule: Rule, edge: EdgeMode) {
        self.seed_with_context(grid, gen, rule, edge, None);
    }

    /// Observe a generation transition and check for attractors.
    pub fn observe(
        &mut self,
        current: &Grid,
        next: &Grid,
        next_gen: u64,
        rule: Rule,
        edge: EdgeMode,
    ) -> Option<AttractorEvent> {
        self.observe_with_context(current, next, next_gen, rule, edge, None)
    }

    /// Seed with protocol-aware extra context.
    pub fn seed_with_context(
        &mut self,
        grid: &Grid,
        gen: u64,
        rule: Rule,
        edge: EdgeMode,
        extra: Option<AttractorExtra>,
    ) {
        let (fp, secondary) =
            fingerprint_with_secondary(grid, rule, edge, extra, self.cfg.confirm_on_repeat);
        self.last = Some(fp);
        self.seeded = true;
        self.completed = false;
        if self.cfg.max_history == 0 {
            return;
        }
        self.insert_entry(fp, gen, secondary);
    }

    /// Observe with protocol-aware extra context.
    pub fn observe_with_context(
        &mut self,
        current: &Grid,
        next: &Grid,
        next_gen: u64,
        rule: Rule,
        edge: EdgeMode,
        extra: Option<AttractorExtra>,
    ) -> Option<AttractorEvent> {
        if self.completed {
            return None;
        }
        if !self.seeded {
            let seed_gen = next_gen.saturating_sub(1);
            self.seed_with_context(current, seed_gen, rule, edge, extra);
        }

        if current == next {
            let event = AttractorEvent::FixedPoint { gen: next_gen };
            self.last = Some(compute_fingerprint(next, rule, edge, extra));
            self.completed = true;
            return Some(event);
        }

        if self.cfg.max_history == 0 {
            self.last = Some(compute_fingerprint(next, rule, edge, extra));
            return None;
        }

        let (fp, secondary) =
            fingerprint_with_secondary(next, rule, edge, extra, self.cfg.confirm_on_repeat);
        if let Some(event) = self.check_repeat(fp, secondary, next_gen) {
            self.last = Some(fp);
            self.completed = true;
            return Some(event);
        }

        self.insert_entry(fp, next_gen, secondary);
        self.last = Some(fp);
        None
    }

    /// Check whether `fingerprint` matches a previously seen entry.
    fn check_repeat(
        &self,
        fp: Fingerprint,
        secondary: Option<u64>,
        next_gen: u64,
    ) -> Option<AttractorEvent> {
        let entries = self.seen.get(&fp)?;
        let entry = if self.cfg.confirm_on_repeat {
            let secondary = secondary?;
            entries.iter().find(|e| e.secondary == Some(secondary))?
        } else {
            entries.first()?
        };
        let first_seen = entry.first_seen;
        Some(AttractorEvent::Cycle {
            gen: next_gen,
            first_seen,
            period: next_gen.saturating_sub(first_seen),
            transient: first_seen,
        })
    }

    /// Record a new fingerprint in the history.
    fn insert_entry(&mut self, fp: Fingerprint, gen: u64, secondary: Option<u64>) {
        if self.cfg.max_history == 0 {
            return;
        }
        let entry = SeenEntry {
            first_seen: gen,
            secondary,
        };
        self.seen.entry(fp).or_default().push(entry);
        self.order.push_back((fp, gen));
        self.seen_entries = self.seen_entries.saturating_add(1);
        self.evict_if_needed();
    }

    /// Remove the oldest entries when the history exceeds `max_history`.
    fn evict_if_needed(&mut self) {
        if self.cfg.max_history == 0 {
            return;
        }
        while self.seen_entries > self.cfg.max_history {
            let Some((fp, gen)) = self.order.pop_front() else {
                break;
            };
            let Some(entries) = self.seen.get_mut(&fp) else {
                continue;
            };
            if let Some(pos) = entries.iter().position(|e| e.first_seen == gen) {
                entries.remove(pos);
                self.seen_entries = self.seen_entries.saturating_sub(1);
            }
            if entries.is_empty() {
                self.seen.remove(&fp);
            }
        }
    }
}

// ── Fingerprinting ──────────────────────────────────────────────────

/// Compute a primary fingerprint (blake3-based) without a secondary hash.
fn compute_fingerprint(
    grid: &Grid,
    rule: Rule,
    edge: EdgeMode,
    extra: Option<AttractorExtra>,
) -> Fingerprint {
    fingerprint_with_secondary(grid, rule, edge, extra, false).0
}

/// Compute a blake3-based primary fingerprint and an optional FNV-1a
/// secondary hash for collision confirmation.
fn fingerprint_with_secondary(
    grid: &Grid,
    rule: Rule,
    edge: EdgeMode,
    extra: Option<AttractorExtra>,
    include_secondary: bool,
) -> (Fingerprint, Option<u64>) {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"nit-gol-attractor-v1");
    hasher.update(&grid.width().to_le_bytes());
    hasher.update(&grid.height().to_le_bytes());
    hasher.update(&rule.births_mask().to_le_bytes());
    hasher.update(&rule.survives_mask().to_le_bytes());
    hasher.update(&[edge_tag(edge)]);
    if let Some(extra) = extra {
        hasher.update(b"proto");
        hasher.update(&extra.protocol_hash.to_le_bytes());
        hasher.update(&extra.phase_idx.to_le_bytes());
        hasher.update(&extra.step_in_phase.to_le_bytes());
    }
    hasher.update(grid.cells());
    let fp = Fingerprint(blake3_u64_pair(&hasher.finalize()));
    let secondary = if include_secondary {
        Some(secondary_hash(grid, rule, edge, extra))
    } else {
        None
    };
    (fp, secondary)
}

/// FNV-1a secondary hash for double-checking blake3 fingerprint matches.
fn secondary_hash(grid: &Grid, rule: Rule, edge: EdgeMode, extra: Option<AttractorExtra>) -> u64 {
    let mut hash = FNV_OFFSET;
    hash = fnv1a(hash, &grid.width().to_le_bytes());
    hash = fnv1a(hash, &grid.height().to_le_bytes());
    hash = fnv1a(hash, &rule.births_mask().to_le_bytes());
    hash = fnv1a(hash, &rule.survives_mask().to_le_bytes());
    hash = fnv1a(hash, &[edge_tag(edge)]);
    if let Some(extra) = extra {
        hash = fnv1a(hash, b"proto");
        hash = fnv1a(hash, &extra.protocol_hash.to_le_bytes());
        hash = fnv1a(hash, &extra.phase_idx.to_le_bytes());
        hash = fnv1a(hash, &extra.step_in_phase.to_le_bytes());
    }
    fnv1a(hash, grid.cells())
}

// ── Test helpers ────────────────────────────────────────────────────

#[cfg(test)]
impl AttractorDetector {
    pub(crate) fn seed_with_fingerprint(
        &mut self,
        gen: u64,
        fp: Fingerprint,
        secondary: Option<u64>,
    ) {
        self.seeded = true;
        self.last = Some(fp);
        self.completed = false;
        if self.cfg.max_history == 0 {
            return;
        }
        self.insert_entry(fp, gen, secondary);
    }

    pub(crate) fn observe_with_fingerprint(
        &mut self,
        current: &Grid,
        next: &Grid,
        next_gen: u64,
        fp: Fingerprint,
        secondary: Option<u64>,
    ) -> Option<AttractorEvent> {
        if self.completed {
            return None;
        }
        if !self.seeded {
            return None;
        }
        if current == next {
            let event = AttractorEvent::FixedPoint { gen: next_gen };
            self.last = Some(fp);
            self.completed = true;
            return Some(event);
        }
        if self.cfg.max_history == 0 {
            self.last = Some(fp);
            return None;
        }
        if let Some(event) = self.check_repeat(fp, secondary, next_gen) {
            self.last = Some(fp);
            self.completed = true;
            return Some(event);
        }
        self.insert_entry(fp, next_gen, secondary);
        self.last = Some(fp);
        None
    }

    /// Create a test fingerprint from a raw `u128` value.
    pub(crate) fn test_fingerprint(value: u128) -> Fingerprint {
        let bytes = value.to_le_bytes();
        let lo = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let hi = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        Fingerprint([lo, hi])
    }
}

#[cfg(test)]
#[path = "test_modules/attractor.rs"]
mod tests;
