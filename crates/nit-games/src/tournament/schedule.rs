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
            // Each row `a` has `N-1` opponents; the inner offset skips `a`
            // itself so that `b_offset >= a` maps to `b_offset + 1`.
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

/// Match ID assigned to an ordered pair in the first repetition.
pub fn first_match_id(strategy_count: usize, self_play: bool, a: usize, b: usize) -> Option<usize> {
    if a >= strategy_count || b >= strategy_count || (!self_play && a == b) {
        return None;
    }
    let row_width = strategy_count - usize::from(!self_play);
    let column = b - usize::from(!self_play && b > a);
    a.checked_mul(row_width)?.checked_add(column)
}

// Distinct matchups in a single repetition: N*N with self-play, N*(N-1)
// otherwise. Returns `None` on overflow so callers can refuse oversized
// schedules instead of panicking deep inside scheduling.
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

pub(super) fn total_schedule_matches(
    strategy_count: usize,
    repetitions: u32,
    self_play: bool,
) -> Option<usize> {
    matches_per_repetition(strategy_count, self_play)?.checked_mul(repetitions as usize)
}

#[cfg(test)]
mod first_match_tests {
    use super::*;

    #[test]
    fn first_match_ids_identify_the_scheduled_pair() {
        for n in 1..8 {
            for self_play in [true, false] {
                let schedule = SchedulePlan::new(n, 2, self_play);
                for a in 0..n {
                    for b in 0..n {
                        if !self_play && a == b {
                            assert_eq!(first_match_id(n, self_play, a, b), None);
                            continue;
                        }
                        let id = first_match_id(n, self_play, a, b).unwrap();
                        let m = schedule.matchup(id).unwrap();
                        assert_eq!((m.a_idx, m.b_idx, m.repetition), (a, b, 0));
                    }
                }
            }
        }
    }
}
