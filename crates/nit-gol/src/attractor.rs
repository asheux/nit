use std::collections::{HashMap, VecDeque};

use crate::{EdgeMode, Grid, Rule};

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttractorExtra {
    pub protocol_hash: u64,
    pub phase_idx: u32,
    pub step_in_phase: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Fingerprint([u64; 2]);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AttractorEvent {
    FixedPoint {
        gen: u64,
    },
    Cycle {
        gen: u64,
        first_seen: u64,
        period: u64,
        transient: u64,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AutoStopPolicy {
    Off,
    Fixed,
    Repeat,
}

impl AutoStopPolicy {
    pub fn should_stop(self, event: &AttractorEvent) -> bool {
        match self {
            AutoStopPolicy::Off => false,
            AutoStopPolicy::Fixed => matches!(event, AttractorEvent::FixedPoint { .. }),
            AutoStopPolicy::Repeat => true,
        }
    }

    pub fn next(self) -> Self {
        match self {
            AutoStopPolicy::Off => AutoStopPolicy::Fixed,
            AutoStopPolicy::Fixed => AutoStopPolicy::Repeat,
            AutoStopPolicy::Repeat => AutoStopPolicy::Off,
        }
    }

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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AttractorConfig {
    pub policy: AutoStopPolicy,
    pub max_history: usize,
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

    pub fn reset(&mut self) {
        self.seen.clear();
        self.order.clear();
        self.seen_entries = 0;
        self.last = None;
        self.seeded = false;
        self.completed = false;
    }

    pub fn seed(&mut self, grid: &Grid, gen: u64, rule: Rule, edge: EdgeMode) {
        let (fingerprint, secondary) =
            fingerprint_with_secondary(grid, rule, edge, None, self.cfg.confirm_on_repeat);
        self.last = Some(fingerprint);
        self.seeded = true;
        self.completed = false;
        if self.cfg.max_history == 0 {
            return;
        }
        self.insert_entry(fingerprint, gen, secondary);
    }

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

    pub fn seed_with_context(
        &mut self,
        grid: &Grid,
        gen: u64,
        rule: Rule,
        edge: EdgeMode,
        extra: Option<AttractorExtra>,
    ) {
        let (fingerprint, secondary) =
            fingerprint_with_secondary(grid, rule, edge, extra, self.cfg.confirm_on_repeat);
        self.last = Some(fingerprint);
        self.seeded = true;
        self.completed = false;
        if self.cfg.max_history == 0 {
            return;
        }
        self.insert_entry(fingerprint, gen, secondary);
    }

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
            self.last = Some(fingerprint(next, rule, edge, extra));
            self.completed = true;
            return Some(event);
        }

        if self.cfg.max_history == 0 {
            self.last = Some(fingerprint(next, rule, edge, extra));
            return None;
        }

        let (fingerprint, secondary) =
            fingerprint_with_secondary(next, rule, edge, extra, self.cfg.confirm_on_repeat);
        if let Some(event) = self.check_repeat(fingerprint, secondary, next_gen) {
            self.last = Some(fingerprint);
            self.completed = true;
            return Some(event);
        }

        self.insert_entry(fingerprint, next_gen, secondary);
        self.last = Some(fingerprint);
        None
    }

    fn check_repeat(
        &self,
        fingerprint: Fingerprint,
        secondary: Option<u64>,
        next_gen: u64,
    ) -> Option<AttractorEvent> {
        let entries = self.seen.get(&fingerprint)?;
        if self.cfg.confirm_on_repeat {
            let secondary = secondary?;
            let entry = entries
                .iter()
                .find(|entry| entry.secondary == Some(secondary))?;
            let first_seen = entry.first_seen;
            let period = next_gen.saturating_sub(first_seen);
            let transient = first_seen;
            return Some(AttractorEvent::Cycle {
                gen: next_gen,
                first_seen,
                period,
                transient,
            });
        }
        let entry = entries.first()?;
        let first_seen = entry.first_seen;
        let period = next_gen.saturating_sub(first_seen);
        let transient = first_seen;
        Some(AttractorEvent::Cycle {
            gen: next_gen,
            first_seen,
            period,
            transient,
        })
    }

    fn insert_entry(&mut self, fingerprint: Fingerprint, gen: u64, secondary: Option<u64>) {
        if self.cfg.max_history == 0 {
            return;
        }
        let entry = SeenEntry {
            first_seen: gen,
            secondary,
        };
        self.seen.entry(fingerprint).or_default().push(entry);
        self.order.push_back((fingerprint, gen));
        self.seen_entries = self.seen_entries.saturating_add(1);
        self.evict_if_needed();
    }

    fn evict_if_needed(&mut self) {
        if self.cfg.max_history == 0 {
            return;
        }
        while self.seen_entries > self.cfg.max_history {
            let Some((fingerprint, gen)) = self.order.pop_front() else {
                break;
            };
            let Some(entries) = self.seen.get_mut(&fingerprint) else {
                continue;
            };
            if let Some(pos) = entries.iter().position(|entry| entry.first_seen == gen) {
                entries.remove(pos);
                self.seen_entries = self.seen_entries.saturating_sub(1);
            }
            if entries.is_empty() {
                self.seen.remove(&fingerprint);
            }
        }
    }
}

fn fingerprint(
    grid: &Grid,
    rule: Rule,
    edge: EdgeMode,
    extra: Option<AttractorExtra>,
) -> Fingerprint {
    fingerprint_with_secondary(grid, rule, edge, extra, false).0
}

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
    let hash = hasher.finalize();
    let bytes = hash.as_bytes();
    let mut a = [0u8; 8];
    let mut b = [0u8; 8];
    a.copy_from_slice(&bytes[0..8]);
    b.copy_from_slice(&bytes[8..16]);
    let fingerprint = Fingerprint([u64::from_le_bytes(a), u64::from_le_bytes(b)]);
    let secondary = if include_secondary {
        Some(secondary_hash(grid, rule, edge, extra))
    } else {
        None
    };
    (fingerprint, secondary)
}

fn secondary_hash(grid: &Grid, rule: Rule, edge: EdgeMode, extra: Option<AttractorExtra>) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
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
    for &byte in grid.cells() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    const FNV_PRIME: u64 = 0x100000001b3;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn edge_tag(edge: EdgeMode) -> u8 {
    match edge {
        EdgeMode::Dead => 0,
        EdgeMode::Toroid => 1,
    }
}

#[cfg(test)]
impl AttractorDetector {
    pub(crate) fn seed_with_fingerprint(
        &mut self,
        gen: u64,
        fingerprint: Fingerprint,
        secondary: Option<u64>,
    ) {
        self.seeded = true;
        self.last = Some(fingerprint);
        self.completed = false;
        if self.cfg.max_history == 0 {
            return;
        }
        self.insert_entry(fingerprint, gen, secondary);
    }

    pub(crate) fn observe_with_fingerprint(
        &mut self,
        current: &Grid,
        next: &Grid,
        next_gen: u64,
        fingerprint: Fingerprint,
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
            self.last = Some(fingerprint);
            self.completed = true;
            return Some(event);
        }
        if self.cfg.max_history == 0 {
            self.last = Some(fingerprint);
            return None;
        }
        if let Some(event) = self.check_repeat(fingerprint, secondary, next_gen) {
            self.last = Some(fingerprint);
            self.completed = true;
            return Some(event);
        }
        self.insert_entry(fingerprint, next_gen, secondary);
        self.last = Some(fingerprint);
        None
    }

    pub(crate) fn test_fingerprint(value: u128) -> Fingerprint {
        let bytes = value.to_le_bytes();
        let mut a = [0u8; 8];
        let mut b = [0u8; 8];
        a.copy_from_slice(&bytes[0..8]);
        b.copy_from_slice(&bytes[8..16]);
        Fingerprint([u64::from_le_bytes(a), u64::from_le_bytes(b)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::step::step;

    #[test]
    fn repeat_requires_matching_protocol_phase() {
        let rule = Rule::conway();
        let mut grid = Grid::new(5, 5);
        grid.set(2, 1, true);
        grid.set(2, 2, true);
        grid.set(2, 3, true);

        let g0 = grid.clone();
        let g1 = step(&g0, rule, EdgeMode::Dead);
        let g2 = step(&g1, rule, EdgeMode::Dead);
        let g3 = step(&g2, rule, EdgeMode::Dead);
        let g4 = step(&g3, rule, EdgeMode::Dead);

        let mut detector = AttractorDetector::new(AttractorConfig {
            policy: AutoStopPolicy::Repeat,
            ..AttractorConfig::default()
        });
        let proto_hash = 0xabcdu64;
        detector.seed_with_context(
            &g0,
            0,
            rule,
            EdgeMode::Dead,
            Some(AttractorExtra {
                protocol_hash: proto_hash,
                phase_idx: 0,
                step_in_phase: 0,
            }),
        );

        let event1 = detector.observe_with_context(
            &g0,
            &g1,
            1,
            rule,
            EdgeMode::Dead,
            Some(AttractorExtra {
                protocol_hash: proto_hash,
                phase_idx: 1,
                step_in_phase: 0,
            }),
        );
        assert!(event1.is_none());

        let event2 = detector.observe_with_context(
            &g1,
            &g2,
            2,
            rule,
            EdgeMode::Dead,
            Some(AttractorExtra {
                protocol_hash: proto_hash,
                phase_idx: 2,
                step_in_phase: 0,
            }),
        );
        assert!(event2.is_none());

        let event3 = detector.observe_with_context(
            &g2,
            &g3,
            3,
            rule,
            EdgeMode::Dead,
            Some(AttractorExtra {
                protocol_hash: proto_hash,
                phase_idx: 3,
                step_in_phase: 0,
            }),
        );
        assert!(event3.is_none());

        let event4 = detector.observe_with_context(
            &g3,
            &g4,
            4,
            rule,
            EdgeMode::Dead,
            Some(AttractorExtra {
                protocol_hash: proto_hash,
                phase_idx: 0,
                step_in_phase: 0,
            }),
        );
        assert!(matches!(
            event4,
            Some(AttractorEvent::Cycle { period, .. }) if period == 4
        ));
    }
}
