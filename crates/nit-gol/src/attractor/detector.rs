//! Stateful attractor detector tracking grid fingerprints across generations.

use std::collections::{HashMap, VecDeque};

use super::fingerprint::{self, Fingerprint};
use super::{AttractorEvent, AttractorExtra, AutoStopPolicy};
use crate::{EdgeMode, Grid, Rule};

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

    /// Clear all history and reset to the initial state.
    pub fn reset(&mut self) {
        self.seen.clear();
        self.order.clear();
        self.entry_count = 0;
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

    /// Register the initial grid state with optional protocol context mixed
    /// into the fingerprint. Use this when running a multi-phase protocol
    /// so identical grids in different phases are not conflated.
    pub fn seed_with_context(
        &mut self,
        grid: &Grid,
        gen: u64,
        rule: Rule,
        edge: EdgeMode,
        extra: Option<AttractorExtra>,
    ) {
        let (fp, secondary) = fingerprint::compute_with_secondary(
            grid,
            rule,
            edge,
            extra,
            self.cfg.confirm_on_repeat,
        );
        self.last = Some(fp);
        self.seeded = true;
        self.completed = false;
        if self.cfg.max_history == 0 {
            return;
        }
        self.insert_entry(fp, gen, secondary);
    }

    /// Observe a generation transition with optional protocol context;
    /// the protocol fields are folded into the fingerprint so phase
    /// boundaries do not masquerade as cycles.
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
            return Some(self.finish_fixed_point(next, rule, edge, extra, next_gen));
        }
        if self.cfg.max_history == 0 {
            self.last = Some(fingerprint::compute(next, rule, edge, extra));
            return None;
        }

        let (fp, secondary) = fingerprint::compute_with_secondary(
            next,
            rule,
            edge,
            extra,
            self.cfg.confirm_on_repeat,
        );
        if let Some(event) = self.check_repeat(fp, secondary, next_gen) {
            self.last = Some(fp);
            self.completed = true;
            return Some(event);
        }
        self.insert_entry(fp, next_gen, secondary);
        self.last = Some(fp);
        None
    }

    fn finish_fixed_point(
        &mut self,
        next: &Grid,
        rule: Rule,
        edge: EdgeMode,
        extra: Option<AttractorExtra>,
        next_gen: u64,
    ) -> AttractorEvent {
        self.last = Some(fingerprint::compute(next, rule, edge, extra));
        self.completed = true;
        AttractorEvent::FixedPoint { gen: next_gen }
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

    /// Remove the oldest entries when the history exceeds `max_history`.
    fn evict_if_needed(&mut self) {
        if self.cfg.max_history == 0 {
            return;
        }
        while self.entry_count > self.cfg.max_history {
            let Some((fp, gen)) = self.order.pop_front() else {
                break;
            };
            let Some(entries) = self.seen.get_mut(&fp) else {
                continue;
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

    pub(crate) fn test_fingerprint(value: u128) -> Fingerprint {
        Fingerprint::from_u128(value)
    }
}
