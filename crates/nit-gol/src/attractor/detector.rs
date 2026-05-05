//! Stateful attractor detector tracking grid fingerprints across generations.

use std::collections::{HashMap, VecDeque};

use super::{compute_fingerprint, AttractorEvent, AttractorExtra, AutoStopPolicy, Fingerprint};
use crate::{EdgeMode, Grid, Rule};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AttractorConfig {
    pub policy: AutoStopPolicy,
    /// Maximum number of fingerprint entries retained before FIFO eviction kicks in.
    pub max_history: usize,
    /// When true, a primary-hash match must also agree on the secondary
    /// FNV hash before a cycle is reported. Collision guard against
    /// accidental blake3 prefix clashes masquerading as genuine repeats.
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

#[derive(Clone, Debug)]
struct SeenEntry {
    first_seen: u64,
    secondary: Option<u64>,
}

/// Stateful detector that tracks grid fingerprints across generations.
///
/// Feed grids via [`seed`](Self::seed) and [`observe`](Self::observe);
/// an [`AttractorEvent`] is returned the first time a fixed point or
/// cycle is found. After emitting an event the detector latches into
/// `completed` so callers can keep driving without double-firing.
pub struct AttractorDetector {
    cfg: AttractorConfig,
    seen: HashMap<Fingerprint, Vec<SeenEntry>>,
    order: VecDeque<(Fingerprint, u64)>,
    entry_count: usize,
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
            entry_count: 0,
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

    pub fn reset(&mut self) {
        self.seen.clear();
        self.order.clear();
        self.entry_count = 0;
        self.last = None;
        self.seeded = false;
        self.completed = false;
    }

    #[inline]
    pub fn seed(&mut self, grid: &Grid, gen: u64, rule: Rule, edge: EdgeMode) {
        self.seed_with_context(grid, gen, rule, edge, None);
    }

    #[inline]
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

    /// Seed the detector with the initial grid, mixing optional protocol
    /// context into the fingerprint so phase boundaries remain distinct.
    pub fn seed_with_context(
        &mut self,
        grid: &Grid,
        gen: u64,
        rule: Rule,
        edge: EdgeMode,
        extra: Option<AttractorExtra>,
    ) {
        let (fp, secondary) = self.compute_fingerprint(grid, rule, edge, extra);
        self.last = Some(fp);
        self.seeded = true;
        self.completed = false;
        self.insert_entry(fp, gen, secondary);
    }

    /// Observe a transition with optional protocol context; see
    /// [`seed_with_context`](Self::seed_with_context) for why the
    /// context matters during multi-phase runs.
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
        let (fp, secondary) = self.compute_fingerprint(next, rule, edge, extra);
        self.observe_inner(current, next, next_gen, fp, secondary)
    }

    /// Core observation logic shared by the context and fingerprint paths.
    ///
    /// Callers are responsible for having already computed a fingerprint
    /// that matches `next`; this routine performs only the detection
    /// bookkeeping. Does NOT consult the `seeded` flag — public entry
    /// points decide whether to auto-seed or short-circuit.
    fn observe_inner(
        &mut self,
        current: &Grid,
        next: &Grid,
        next_gen: u64,
        fp: Fingerprint,
        secondary: Option<u64>,
    ) -> Option<AttractorEvent> {
        if current == next {
            self.last = Some(fp);
            self.completed = true;
            return Some(AttractorEvent::FixedPoint { gen: next_gen });
        }
        self.last = Some(fp);
        if self.cfg.max_history == 0 {
            return None;
        }
        if let Some(event) = self.check_repeat(fp, secondary, next_gen) {
            self.completed = true;
            return Some(event);
        }
        self.insert_entry(fp, next_gen, secondary);
        None
    }

    fn compute_fingerprint(
        &self,
        grid: &Grid,
        rule: Rule,
        edge: EdgeMode,
        extra: Option<AttractorExtra>,
    ) -> (Fingerprint, Option<u64>) {
        compute_fingerprint(grid, rule, edge, extra, self.cfg.confirm_on_repeat)
    }

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

    fn insert_entry(&mut self, fp: Fingerprint, gen: u64, secondary: Option<u64>) {
        if self.cfg.max_history == 0 {
            return;
        }
        self.seen.entry(fp).or_default().push(SeenEntry {
            first_seen: gen,
            secondary,
        });
        self.order.push_back((fp, gen));
        self.entry_count = self.entry_count.saturating_add(1);
        self.evict_if_needed();
    }

    fn evict_if_needed(&mut self) {
        while self.entry_count > self.cfg.max_history {
            let Some((fp, gen)) = self.order.pop_front() else {
                break;
            };
            self.drop_stale_entry(fp, gen);
        }
    }

    fn drop_stale_entry(&mut self, fp: Fingerprint, gen: u64) {
        let Some(entries) = self.seen.get_mut(&fp) else {
            return;
        };
        if let Some(pos) = entries.iter().position(|e| e.first_seen == gen) {
            entries.remove(pos);
            self.entry_count = self.entry_count.saturating_sub(1);
        }
        if entries.is_empty() {
            self.seen.remove(&fp);
        }
    }
}

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
        if self.completed || !self.seeded {
            return None;
        }
        self.observe_inner(current, next, next_gen, fp, secondary)
    }

    pub(crate) fn test_fingerprint(value: u128) -> Fingerprint {
        Fingerprint::from_u128(value)
    }
}
