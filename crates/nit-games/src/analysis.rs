//! Post-tournament history analysis.
//!
//! Given an NDJSON history log produced by a tournament run, this module
//! computes per-match summaries (outcome counts, cooperation rates),
//! per-strategy aggregates, and trajectory samples for randomness-tagged
//! matches. Results are written as CSV, NDJSON, and a JSON summary file
//! under a configurable output directory.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::events::EventWriter;
use nit_utils::fs::write_atomic;

const ANALYSIS_SCHEMA_VERSION: u32 = 1;
const DEFAULT_TAIL_ROUNDS: usize = 10_000;
const DEFAULT_TRAJECTORY_SAMPLES: usize = 50;
const DEFAULT_PREVIEW_LIMIT: usize = 3;

/// Configuration controlling the history analysis pass: how many tail rounds
/// to consider, how many trajectory sample buckets to produce, and which
/// strategy-ID substrings identify "random" matches.
#[derive(Clone, Debug)]
pub struct AnalysisConfig {
    pub tail_rounds: usize,
    pub trajectory_samples: usize,
    pub random_match_substrings: Vec<String>,
    pub preview_limit: usize,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            tail_rounds: DEFAULT_TAIL_ROUNDS,
            trajectory_samples: DEFAULT_TRAJECTORY_SAMPLES,
            random_match_substrings: vec!["rand".into(), "random".into()],
            preview_limit: DEFAULT_PREVIEW_LIMIT,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalysisPaths {
    pub summary: String,
    pub matches_csv: String,
    pub matches_ndjson: String,
    pub strategies_csv: String,
    pub trajectories_csv: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OutcomeCounts {
    pub cc: u32,
    pub cd: u32,
    pub dc: u32,
    pub dd: u32,
}

/// Per-match analysis record: outcome counts, cooperation rates, and
/// tail-window statistics for a single match between two strategies.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchSummary {
    pub match_id: usize,
    pub match_index: usize,
    pub total_matches: usize,
    pub repetition: u32,
    pub rounds: u32,
    pub a: String,
    pub b: String,
    pub a_score: i64,
    pub b_score: i64,
    pub outcomes: OutcomeCounts,
    pub a_coop_rate: f64,
    pub b_coop_rate: f64,
    pub tail_rounds: u32,
    pub tail_outcomes: OutcomeCounts,
    pub a_tail_coop_rate: f64,
    pub b_tail_coop_rate: f64,
    pub a_initial: Option<char>,
    pub b_initial: Option<char>,
}

/// Aggregate analysis for a single strategy across all its matches:
/// total rounds, cooperation rate, tail cooperation rate, and average score.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategySummary {
    pub id: String,
    pub matches: u32,
    pub rounds: u64,
    pub coop_rounds: u64,
    pub coop_rate: f64,
    pub tail_rounds: u64,
    pub tail_coop_rounds: u64,
    pub tail_coop_rate: f64,
    pub total_score: i64,
    pub avg_score_per_round: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrajectorySample {
    pub match_id: usize,
    pub match_index: usize,
    pub a: String,
    pub b: String,
    pub sample_index: usize,
    pub round_start: u32,
    pub round_end: u32,
    pub window_rounds: u32,
    pub a_coop_rate: f64,
    pub b_coop_rate: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryAnalysisSummary {
    pub schema_version: u32,
    pub generated_at: String,
    pub source_history: String,
    pub total_matches: usize,
    pub total_rounds: u64,
    pub min_rounds: u32,
    pub max_rounds: u32,
    pub tail_rounds: usize,
    pub trajectory_samples: usize,
    pub random_match_substrings: Vec<String>,
    pub paths: AnalysisPaths,
    pub strategies: Vec<StrategySummary>,
    pub random_match_ids: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct TrajectoryPreview {
    pub match_id: usize,
    pub match_index: usize,
    pub a: String,
    pub b: String,
    pub rounds: u32,
    pub a_rates: Vec<f64>,
    pub b_rates: Vec<f64>,
}

#[derive(Clone, Debug)]
pub struct HistoryAnalysisPreview {
    pub strategies: Vec<StrategySummary>,
    pub trajectories: Vec<TrajectoryPreview>,
}

#[derive(Clone, Debug)]
pub struct HistoryAnalysis {
    pub summary: HistoryAnalysisSummary,
    pub preview: HistoryAnalysisPreview,
}

#[derive(Deserialize)]
struct MatchHistoryLite {
    #[serde(default)]
    match_id: usize,
    #[serde(default)]
    match_index: usize,
    #[serde(default)]
    total_matches: usize,
    a: String,
    b: String,
    #[serde(default)]
    repetition: u32,
    #[serde(default, alias = "outcomes")]
    score_idx: String,
    #[serde(default)]
    a_score: i64,
    #[serde(default)]
    b_score: i64,
}

#[derive(Default)]
struct StrategyAgg {
    matches: u32,
    rounds: u64,
    coop_rounds: u64,
    tail_rounds: u64,
    tail_coop_rounds: u64,
    total_score: i64,
}

/// Run the full analysis pipeline over the given NDJSON history log, writing
/// CSV and JSON artefacts into `out_dir`.
///
/// Returns a [`HistoryAnalysis`] containing both the serialised summary and a
/// small preview suitable for TUI display.
pub fn analyze_history(
    history_path: &Path,
    out_dir: &Path,
    mut config: AnalysisConfig,
) -> Result<HistoryAnalysis, String> {
    if config.tail_rounds == 0 {
        config.tail_rounds = DEFAULT_TAIL_ROUNDS;
    }
    if config.trajectory_samples == 0 {
        config.trajectory_samples = DEFAULT_TRAJECTORY_SAMPLES;
    }
    if config.preview_limit == 0 {
        config.preview_limit = DEFAULT_PREVIEW_LIMIT;
    }
    config
        .random_match_substrings
        .iter_mut()
        .for_each(|s| s.make_ascii_lowercase());

    fs::create_dir_all(out_dir)
        .map_err(|err| format!("Failed to create analysis dir {}: {err}", out_dir.display()))?;

    let base = analysis_base_name(history_path);
    let paths = AnalysisPaths {
        summary: out_dir
            .join(format!("analysis__{base}.json"))
            .display()
            .to_string(),
        matches_csv: out_dir
            .join(format!("analysis_matches__{base}.csv"))
            .display()
            .to_string(),
        matches_ndjson: out_dir
            .join(format!("analysis_matches__{base}.ndjson"))
            .display()
            .to_string(),
        strategies_csv: out_dir
            .join(format!("analysis_strategies__{base}.csv"))
            .display()
            .to_string(),
        trajectories_csv: out_dir
            .join(format!("analysis_trajectories__{base}.csv"))
            .display()
            .to_string(),
    };

    let history_file =
        File::open(history_path).map_err(|err| format!("Failed to open history log: {err}"))?;
    let reader = BufReader::new(history_file);

    let mut matches_csv =
        BufWriter::new(File::create(&paths.matches_csv).map_err(|err| err.to_string())?);
    let mut matches_ndjson =
        BufWriter::new(File::create(&paths.matches_ndjson).map_err(|err| err.to_string())?);
    let mut trajectories_csv =
        BufWriter::new(File::create(&paths.trajectories_csv).map_err(|err| err.to_string())?);

    write_matches_csv_header(&mut matches_csv)?;
    write_trajectories_csv_header(&mut trajectories_csv)?;

    let mut strategy_map: HashMap<String, StrategyAgg> = HashMap::new();
    let mut preview_trajectories = Vec::new();
    let mut random_match_ids = Vec::new();

    let mut total_matches = 0usize;
    let mut total_rounds = 0u64;
    let mut min_rounds = u32::MAX;
    let mut max_rounds = 0u32;

    for line in reader.lines() {
        let line = line.map_err(|err| format!("Failed to read history log: {err}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let record: MatchHistoryLite =
            serde_json::from_str(&line).map_err(|err| format!("History parse error: {err}"))?;

        let summary = summarize_record(&record, config.tail_rounds, total_matches);
        let rounds = summary.rounds;

        write_match_summary(&mut matches_ndjson, &summary)?;
        write_match_csv_row(&mut matches_csv, &summary)?;

        let (a_coop, b_coop) = coop_counts(&summary.outcomes);
        let (a_tail, b_tail) = coop_counts(&summary.tail_outcomes);

        update_strategy(
            &mut strategy_map,
            &record.a,
            rounds,
            a_coop,
            summary.tail_rounds,
            a_tail,
            record.a_score,
        );
        update_strategy(
            &mut strategy_map,
            &record.b,
            rounds,
            b_coop,
            summary.tail_rounds,
            b_tail,
            record.b_score,
        );

        if is_random_match(&record.a, &record.b, &config.random_match_substrings) {
            random_match_ids.push(record.match_id);
            let trajectory =
                build_trajectory(record.score_idx.as_bytes(), config.trajectory_samples);
            write_trajectory_samples(
                &mut trajectories_csv,
                record.match_id,
                summary.match_index,
                &record.a,
                &record.b,
                rounds,
                &trajectory,
            )?;
            if preview_trajectories.len() < config.preview_limit {
                preview_trajectories.push(TrajectoryPreview {
                    match_id: record.match_id,
                    match_index: summary.match_index,
                    a: record.a.clone(),
                    b: record.b.clone(),
                    rounds,
                    a_rates: trajectory.a_rates.clone(),
                    b_rates: trajectory.b_rates.clone(),
                });
            }
        }

        total_matches += 1;
        total_rounds = total_rounds.saturating_add(rounds as u64);
        min_rounds = min_rounds.min(rounds);
        max_rounds = max_rounds.max(rounds);
    }

    matches_csv.flush().map_err(|err| err.to_string())?;
    matches_ndjson.flush().map_err(|err| err.to_string())?;
    trajectories_csv.flush().map_err(|err| err.to_string())?;

    let mut strategies: Vec<StrategySummary> = strategy_map
        .into_iter()
        .map(|(id, agg)| agg_to_summary(id, agg))
        .collect();
    strategies.sort_by(|a, b| {
        b.coop_rate
            .partial_cmp(&a.coop_rate)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });

    write_strategies_csv(&paths.strategies_csv, &strategies)?;

    if min_rounds == u32::MAX {
        min_rounds = 0;
    }

    let summary = HistoryAnalysisSummary {
        schema_version: ANALYSIS_SCHEMA_VERSION,
        generated_at: EventWriter::timestamp(),
        source_history: history_path.display().to_string(),
        total_matches,
        total_rounds,
        min_rounds,
        max_rounds,
        tail_rounds: config.tail_rounds,
        trajectory_samples: config.trajectory_samples,
        random_match_substrings: config.random_match_substrings.clone(),
        paths: paths.clone(),
        strategies: strategies.clone(),
        random_match_ids,
    };

    let summary_path = Path::new(&paths.summary).to_path_buf();
    write_atomic(&summary_path, |writer| {
        serde_json::to_writer_pretty(writer, &summary).map_err(std::io::Error::other)
    })
    .map_err(|err| format!("Failed to write analysis summary: {err}"))?;

    Ok(HistoryAnalysis {
        summary,
        preview: HistoryAnalysisPreview {
            strategies,
            trajectories: preview_trajectories,
        },
    })
}

/// Build a [`MatchSummary`] from a parsed history record.
fn summarize_record(
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

fn analysis_base_name(history_path: &Path) -> String {
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

fn coop_counts(counts: &OutcomeCounts) -> (u32, u32) {
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

fn update_strategy(
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

/// Convert a [`StrategyAgg`] accumulator into the public [`StrategySummary`],
/// computing derived rates.
fn agg_to_summary(id: String, aggregate: StrategyAgg) -> StrategySummary {
    let coop_rate = if aggregate.rounds == 0 {
        0.0
    } else {
        aggregate.coop_rounds as f64 / aggregate.rounds as f64
    };
    let tail_coop_rate = if aggregate.tail_rounds == 0 {
        0.0
    } else {
        aggregate.tail_coop_rounds as f64 / aggregate.tail_rounds as f64
    };
    let avg_score_per_round = if aggregate.rounds == 0 {
        0.0
    } else {
        aggregate.total_score as f64 / aggregate.rounds as f64
    };
    StrategySummary {
        id,
        matches: aggregate.matches,
        rounds: aggregate.rounds,
        coop_rounds: aggregate.coop_rounds,
        coop_rate,
        tail_rounds: aggregate.tail_rounds,
        tail_coop_rounds: aggregate.tail_coop_rounds,
        tail_coop_rate,
        total_score: aggregate.total_score,
        avg_score_per_round,
    }
}

fn is_random_match(a: &str, b: &str, needles: &[String]) -> bool {
    let a_lower = a.to_ascii_lowercase();
    let b_lower = b.to_ascii_lowercase();
    needles.iter().any(|needle| {
        let needle = needle.as_str();
        a_lower.contains(needle) || b_lower.contains(needle)
    })
}

struct TrajectoryData {
    a_rates: Vec<f64>,
    b_rates: Vec<f64>,
    starts: Vec<u32>,
    ends: Vec<u32>,
}

fn build_trajectory(outcomes: &[u8], samples: usize) -> TrajectoryData {
    let total = outcomes.len();
    if total == 0 {
        return TrajectoryData {
            a_rates: Vec::new(),
            b_rates: Vec::new(),
            starts: Vec::new(),
            ends: Vec::new(),
        };
    }
    let samples = samples.min(total).max(1);
    let mut a_counts = vec![0u32; samples];
    let mut b_counts = vec![0u32; samples];
    let mut bucket_counts = vec![0u32; samples];
    for (idx, &byte) in outcomes.iter().enumerate() {
        let bucket = idx * samples / total;
        bucket_counts[bucket] += 1;
        match byte {
            b'0' => {
                a_counts[bucket] += 1;
                b_counts[bucket] += 1;
            }
            b'1' => a_counts[bucket] += 1,
            b'2' => b_counts[bucket] += 1,
            _ => {}
        }
    }
    let mut a_rates = Vec::with_capacity(samples);
    let mut b_rates = Vec::with_capacity(samples);
    let mut starts = Vec::with_capacity(samples);
    let mut ends = Vec::with_capacity(samples);
    for bucket in 0..samples {
        let start = (bucket * total / samples) as u32 + 1;
        let end = ((bucket + 1) * total / samples) as u32;
        let window = bucket_counts[bucket].max(1) as f64;
        a_rates.push(a_counts[bucket] as f64 / window);
        b_rates.push(b_counts[bucket] as f64 / window);
        starts.push(start);
        ends.push(end);
    }
    TrajectoryData {
        a_rates,
        b_rates,
        starts,
        ends,
    }
}

fn write_match_summary(writer: &mut BufWriter<File>, summary: &MatchSummary) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, summary).map_err(|err| err.to_string())?;
    writer.write_all(b"\n").map_err(|err| err.to_string())?;
    Ok(())
}

fn write_matches_csv_header(writer: &mut BufWriter<File>) -> Result<(), String> {
    let header = [
        "match_id",
        "match_index",
        "total_matches",
        "repetition",
        "rounds",
        "a",
        "b",
        "a_score",
        "b_score",
        "cc",
        "cd",
        "dc",
        "dd",
        "a_coop_rate",
        "b_coop_rate",
        "tail_rounds",
        "tail_cc",
        "tail_cd",
        "tail_dc",
        "tail_dd",
        "a_tail_coop_rate",
        "b_tail_coop_rate",
        "a_initial",
        "b_initial",
    ]
    .join(",");
    writeln!(writer, "{header}").map_err(|err| err.to_string())
}

fn write_match_csv_row(writer: &mut BufWriter<File>, summary: &MatchSummary) -> Result<(), String> {
    let a = csv_escape(&summary.a);
    let b = csv_escape(&summary.b);
    let a_initial = summary
        .a_initial
        .map(|c| c.to_string())
        .unwrap_or_else(|| "".into());
    let b_initial = summary
        .b_initial
        .map(|c| c.to_string())
        .unwrap_or_else(|| "".into());
    writeln!(
        writer,
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{:.6},{:.6},{},{},{},{},{},{:.6},{:.6},{},{}",
        summary.match_id,
        summary.match_index,
        summary.total_matches,
        summary.repetition,
        summary.rounds,
        a,
        b,
        summary.a_score,
        summary.b_score,
        summary.outcomes.cc,
        summary.outcomes.cd,
        summary.outcomes.dc,
        summary.outcomes.dd,
        summary.a_coop_rate,
        summary.b_coop_rate,
        summary.tail_rounds,
        summary.tail_outcomes.cc,
        summary.tail_outcomes.cd,
        summary.tail_outcomes.dc,
        summary.tail_outcomes.dd,
        summary.a_tail_coop_rate,
        summary.b_tail_coop_rate,
        a_initial,
        b_initial
    )
    .map_err(|err| err.to_string())
}

fn write_strategies_csv(path: &str, strategies: &[StrategySummary]) -> Result<(), String> {
    let mut writer = BufWriter::new(File::create(path).map_err(|err| err.to_string())?);
    let header = [
        "id",
        "matches",
        "rounds",
        "coop_rounds",
        "coop_rate",
        "tail_rounds",
        "tail_coop_rounds",
        "tail_coop_rate",
        "total_score",
        "avg_score_per_round",
    ]
    .join(",");
    writeln!(writer, "{header}").map_err(|err| err.to_string())?;
    for strategy in strategies {
        let id = csv_escape(&strategy.id);
        writeln!(
            writer,
            "{},{},{},{},{:.6},{},{},{:.6},{},{:.6}",
            id,
            strategy.matches,
            strategy.rounds,
            strategy.coop_rounds,
            strategy.coop_rate,
            strategy.tail_rounds,
            strategy.tail_coop_rounds,
            strategy.tail_coop_rate,
            strategy.total_score,
            strategy.avg_score_per_round
        )
        .map_err(|err| err.to_string())?;
    }
    writer.flush().map_err(|err| err.to_string())?;
    Ok(())
}

fn write_trajectories_csv_header(writer: &mut BufWriter<File>) -> Result<(), String> {
    let header = [
        "match_id",
        "match_index",
        "a",
        "b",
        "sample_index",
        "round_start",
        "round_end",
        "window_rounds",
        "a_coop_rate",
        "b_coop_rate",
    ]
    .join(",");
    writeln!(writer, "{header}").map_err(|err| err.to_string())
}

fn write_trajectory_samples(
    writer: &mut BufWriter<File>,
    match_id: usize,
    match_index: usize,
    a: &str,
    b: &str,
    rounds: u32,
    data: &TrajectoryData,
) -> Result<(), String> {
    let a_id = csv_escape(a);
    let b_id = csv_escape(b);
    for (idx, (a_rate, b_rate)) in data
        .a_rates
        .iter()
        .copied()
        .zip(data.b_rates.iter().copied())
        .enumerate()
    {
        let start = data.starts.get(idx).copied().unwrap_or(1);
        let end = data.ends.get(idx).copied().unwrap_or(rounds);
        let window_rounds = end.saturating_sub(start).saturating_add(1);
        writeln!(
            writer,
            "{},{},{},{},{},{},{},{},{:.6},{:.6}",
            match_id,
            match_index,
            a_id.as_ref(),
            b_id.as_ref(),
            idx,
            start,
            end,
            window_rounds,
            a_rate,
            b_rate
        )
        .map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn csv_escape(value: &str) -> Cow<'_, str> {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        let mut out = String::with_capacity(value.len() + 2);
        out.push('"');
        for ch in value.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
        Cow::Owned(out)
    } else {
        Cow::Borrowed(value)
    }
}
