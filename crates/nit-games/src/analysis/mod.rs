//! Post-tournament history analysis.

mod csv;
mod summarize;
mod trajectory;

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::events::EventWriter;
use nit_utils::fs::write_atomic;

use csv::{
    write_csv_header, write_match_csv_row, write_match_summary, write_strategies_csv,
    write_trajectory_samples, MATCHES_CSV_COLUMNS, TRAJECTORIES_CSV_COLUMNS,
};
use summarize::{
    agg_to_summary, analysis_base_name, coop_counts, is_random_match, summarize_record,
    update_strategy, MatchHistoryLite, StrategyAgg,
};
use trajectory::build_trajectory;

const ANALYSIS_SCHEMA_VERSION: u32 = 1;
const DEFAULT_TAIL_ROUNDS: usize = 10_000;
const DEFAULT_TRAJECTORY_SAMPLES: usize = 50;
const DEFAULT_PREVIEW_LIMIT: usize = 3;

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

fn finalise_config(mut config: AnalysisConfig) -> AnalysisConfig {
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
    config
}

fn build_paths(out_dir: &Path, base: &str) -> AnalysisPaths {
    let join = |stem: &str| out_dir.join(stem).display().to_string();
    AnalysisPaths {
        summary: join(&format!("analysis__{base}.json")),
        matches_csv: join(&format!("analysis_matches__{base}.csv")),
        matches_ndjson: join(&format!("analysis_matches__{base}.ndjson")),
        strategies_csv: join(&format!("analysis_strategies__{base}.csv")),
        trajectories_csv: join(&format!("analysis_trajectories__{base}.csv")),
    }
}

pub fn analyze_history(
    history_path: &Path,
    out_dir: &Path,
    config: AnalysisConfig,
) -> Result<HistoryAnalysis, String> {
    let config = finalise_config(config);

    fs::create_dir_all(out_dir)
        .map_err(|err| format!("Failed to create analysis dir {}: {err}", out_dir.display()))?;

    let base = analysis_base_name(history_path);
    let paths = build_paths(out_dir, &base);

    let history_file =
        File::open(history_path).map_err(|err| format!("Failed to open history log: {err}"))?;
    let reader = BufReader::new(history_file);

    let mut matches_csv =
        BufWriter::new(File::create(&paths.matches_csv).map_err(|e| e.to_string())?);
    let mut matches_ndjson =
        BufWriter::new(File::create(&paths.matches_ndjson).map_err(|e| e.to_string())?);
    let mut trajectories_csv =
        BufWriter::new(File::create(&paths.trajectories_csv).map_err(|e| e.to_string())?);

    write_csv_header(&mut matches_csv, MATCHES_CSV_COLUMNS)?;
    write_csv_header(&mut trajectories_csv, TRAJECTORIES_CSV_COLUMNS)?;

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

    matches_csv.flush().map_err(|e| e.to_string())?;
    matches_ndjson.flush().map_err(|e| e.to_string())?;
    trajectories_csv.flush().map_err(|e| e.to_string())?;

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
