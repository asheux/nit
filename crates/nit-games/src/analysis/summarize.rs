use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use super::{MatchSummary, OutcomeCounts, StrategySummary};

#[derive(Deserialize)]
pub(super) struct MatchHistoryLite {
    #[serde(default)]
    pub match_id: usize,
    #[serde(default)]
    pub match_index: usize,
    #[serde(default)]
    pub total_matches: usize,
    pub a: String,
    pub b: String,
    #[serde(default)]
    pub repetition: u32,
    #[serde(default, alias = "outcomes")]
    pub score_idx: String,
    #[serde(default)]
    pub a_score: i64,
    #[serde(default)]
    pub b_score: i64,
}

#[derive(Default)]
pub(super) struct StrategyAgg {
    matches: u32,
    rounds: u64,
    coop_rounds: u64,
    tail_rounds: u64,
    tail_coop_rounds: u64,
    total_score: i64,
}

pub(super) fn summarize_record(
    record: &MatchHistoryLite,
    tail_rounds: usize,
    running_total: usize,
) -> MatchSummary {
    let outcomes = record.score_idx.as_bytes();
    let rounds = outcomes.len().min(u32::MAX as usize) as u32;
    let tail_len = tail_rounds.min(outcomes.len());

    let counts = count_outcomes(outcomes);
    let (a_coop, b_coop) = coop_counts(&counts);
    let (a_rate, b_rate) = coop_rates(a_coop, b_coop, rounds);

    let tail_counts = if tail_len > 0 {
        count_outcomes(&outcomes[outcomes.len() - tail_len..])
    } else {
        OutcomeCounts::default()
    };
    let (a_tail, b_tail) = coop_counts(&tail_counts);
    let (a_tail_rate, b_tail_rate) = coop_rates(a_tail, b_tail, tail_len as u32);
    let (a_initial, b_initial) = initial_actions(&record.score_idx);

    let match_index = if record.match_index == 0 {
        record.match_id.saturating_add(1)
    } else {
        record.match_index
    };
    let total_matches_reported = if record.total_matches == 0 {
        match_index.max(running_total.saturating_add(1))
    } else {
        record.total_matches
    };

    MatchSummary {
        match_id: record.match_id,
        match_index,
        total_matches: total_matches_reported,
        repetition: record.repetition,
        rounds,
        a: record.a.clone(),
        b: record.b.clone(),
        a_score: record.a_score,
        b_score: record.b_score,
        outcomes: counts,
        a_coop_rate: a_rate,
        b_coop_rate: b_rate,
        tail_rounds: tail_len as u32,
        tail_outcomes: tail_counts,
        a_tail_coop_rate: a_tail_rate,
        b_tail_coop_rate: b_tail_rate,
        a_initial,
        b_initial,
    }
}

/// Strips the `history__` prefix and `.ndjson` suffix from the source
/// path so the analysis output filenames pair visually with the run
/// they came from.
pub(super) fn analysis_base_name(history_path: &Path) -> String {
    let name = history_path
        .file_name()
        .map(|s| s.to_string_lossy())
        .unwrap_or_else(|| "history".into());
    let mut stem = name.as_ref();
    if let Some(stripped) = stem.strip_suffix(".ndjson") {
        stem = stripped;
    }
    if let Some(stripped) = stem.strip_prefix("history__") {
        stem = stripped;
    }
    if stem.is_empty() {
        "run".to_string()
    } else {
        stem.to_string()
    }
}

fn count_outcomes(bytes: &[u8]) -> OutcomeCounts {
    let mut counts = OutcomeCounts::default();
    for &b in bytes {
        match b {
            b'0' => counts.cc += 1,
            b'1' => counts.cd += 1,
            b'2' => counts.dc += 1,
            b'3' => counts.dd += 1,
            _ => {}
        }
    }
    counts
}

fn initial_actions(outcomes: &str) -> (Option<char>, Option<char>) {
    match outcomes.as_bytes().first().copied() {
        Some(b'0') => (Some('C'), Some('C')),
        Some(b'1') => (Some('C'), Some('D')),
        Some(b'2') => (Some('D'), Some('C')),
        Some(b'3') => (Some('D'), Some('D')),
        _ => (None, None),
    }
}

pub(super) fn coop_counts(counts: &OutcomeCounts) -> (u32, u32) {
    let a = counts.cc + counts.cd;
    let b = counts.cc + counts.dc;
    (a, b)
}

fn coop_rates(a: u32, b: u32, rounds: u32) -> (f64, f64) {
    if rounds == 0 {
        return (0.0, 0.0);
    }
    let denom = rounds as f64;
    (a as f64 / denom, b as f64 / denom)
}

pub(super) fn update_strategy(
    map: &mut HashMap<String, StrategyAgg>,
    id: &str,
    rounds: u32,
    coop_rounds: u32,
    tail_rounds: u32,
    tail_coop_rounds: u32,
    score: i64,
) {
    let entry = map.entry(id.to_string()).or_default();
    entry.matches = entry.matches.saturating_add(1);
    entry.rounds = entry.rounds.saturating_add(rounds as u64);
    entry.coop_rounds = entry.coop_rounds.saturating_add(coop_rounds as u64);
    entry.tail_rounds = entry.tail_rounds.saturating_add(tail_rounds as u64);
    entry.tail_coop_rounds = entry
        .tail_coop_rounds
        .saturating_add(tail_coop_rounds as u64);
    entry.total_score = entry.total_score.saturating_add(score);
}

pub(super) fn agg_to_summary(id: String, agg: StrategyAgg) -> StrategySummary {
    let safe_div = |num: f64, den: u64| if den == 0 { 0.0 } else { num / den as f64 };
    let coop_rate = safe_div(agg.coop_rounds as f64, agg.rounds);
    let tail_coop_rate = safe_div(agg.tail_coop_rounds as f64, agg.tail_rounds);
    let avg_score_per_round = safe_div(agg.total_score as f64, agg.rounds);
    StrategySummary {
        id,
        matches: agg.matches,
        rounds: agg.rounds,
        coop_rounds: agg.coop_rounds,
        coop_rate,
        tail_rounds: agg.tail_rounds,
        tail_coop_rounds: agg.tail_coop_rounds,
        tail_coop_rate,
        total_score: agg.total_score,
        avg_score_per_round,
    }
}

pub(super) fn is_random_match(a: &str, b: &str, needles: &[String]) -> bool {
    let a_lower = a.to_ascii_lowercase();
    let b_lower = b.to_ascii_lowercase();
    needles
        .iter()
        .any(|needle| a_lower.contains(needle.as_str()) || b_lower.contains(needle.as_str()))
}
