use super::types::Matchup;

#[derive(Clone, Debug)]
pub(super) struct SchedulePlan {
    pub(super) strategy_count: usize,
    pub(super) repetitions: u32,
    pub(super) self_play: bool,
    pub(super) total_matches: usize,
}

impl SchedulePlan {
    pub(super) fn new(strategy_count: usize, repetitions: u32, self_play: bool) -> Self {
        let total_matches = total_schedule_matches(strategy_count, repetitions, self_play)
            .expect("tournament schedule size overflow");
        Self {
            strategy_count,
            repetitions,
            self_play,
            total_matches,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.total_matches
    }

    pub(super) fn is_empty(&self) -> bool {
        self.total_matches == 0
    }

    pub(super) fn matchup(&self, match_id: usize) -> Option<Matchup> {
        if match_id >= self.total_matches || self.strategy_count == 0 || self.repetitions == 0 {
            return None;
        }
        let per_rep =
            matches_per_repetition(self.strategy_count, self.self_play).expect("schedule size");
        let repetition = match_id / per_rep;
        let offset = match_id % per_rep;
        let (a_idx, b_idx) = if self.self_play {
            (offset / self.strategy_count, offset % self.strategy_count)
        } else {
            // Map a flat offset into an ordered pair (a, b) where a != b.
            // Each row `a` has `N-1` opponents; b_offset skips index `a`
            // itself so that b_offset >= a maps to b_offset + 1.
            let stride = self.strategy_count.saturating_sub(1);
            let a_idx = offset / stride;
            let b_offset = offset % stride;
            let b_idx = if b_offset >= a_idx {
                b_offset + 1
            } else {
                b_offset
            };
            (a_idx, b_idx)
        };
        Some(Matchup {
            match_id,
            a_idx,
            b_idx,
            repetition: repetition as u32,
        })
    }

    pub(super) fn matchups(&self, start: usize, count: usize) -> Vec<Matchup> {
        let end = start.saturating_add(count).min(self.total_matches);
        (start..end)
            .map(|match_id| self.matchup(match_id).expect("in-range match id"))
            .collect()
    }
}

/// Number of distinct matchups in a single repetition (N*N with self-play,
/// N*(N-1) without). Returns `None` on overflow.
pub(super) fn matches_per_repetition(strategy_count: usize, self_play: bool) -> Option<usize> {
    if strategy_count == 0 {
        return Some(0);
    }
    if self_play {
        strategy_count.checked_mul(strategy_count)
    } else {
        strategy_count.checked_mul(strategy_count.saturating_sub(1))
    }
}

/// Total matches across all repetitions. Returns `None` on overflow.
pub(super) fn total_schedule_matches(
    strategy_count: usize,
    repetitions: u32,
    self_play: bool,
) -> Option<usize> {
    matches_per_repetition(strategy_count, self_play)?.checked_mul(repetitions as usize)
}
