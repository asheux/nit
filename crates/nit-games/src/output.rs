use crate::config::{NormalizedConfig, StrategySpecKind};
use nit_utils::fs::write_atomic;
use nit_utils::hashing::stable_hash_bytes;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategyDefinition {
    pub id: String,
    pub name: Option<String>,
    #[serde(flatten)]
    pub kind: StrategySpecKind,
    pub rng_seed_a: Option<u64>,
    pub rng_seed_b: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategyResult {
    pub id: String,
    pub name: Option<String>,
    pub total_payoff: i64,
    pub average_payoff: f64,
    pub matches: u32,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub crashed: bool,
    pub crash_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairwiseResult {
    pub a: String,
    pub b: String,
    pub a_total: i64,
    pub b_total: i64,
    pub a_wins: u32,
    pub b_wins: u32,
    pub draws: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DominanceEdge {
    pub winner: String,
    pub loser: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TournamentResults {
    pub ranking: Vec<StrategyResult>,
    pub pairwise: Vec<PairwiseResult>,
    pub dominance: Vec<DominanceEdge>,
}

impl TournamentResults {
    pub fn empty() -> Self {
        Self {
            ranking: Vec::new(),
            pairwise: Vec::new(),
            dominance: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunPaths {
    pub summary: Option<String>,
    pub events: Option<String>,
    pub history: Option<String>,
    #[serde(default)]
    pub definitions: Option<String>,
    #[serde(default)]
    pub results: Option<String>,
    #[serde(default)]
    pub config: Option<String>,
    #[serde(default)]
    pub analysis_dir: Option<String>,
}

pub fn run_id_from_seed_config(seed: u64, config_text: &str) -> String {
    let hash = stable_hash_bytes(format!("{seed}:{config_text}").as_bytes());
    format!("{hash:016x}")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunSummary {
    pub schema_version: u32,
    pub timestamp: String,
    pub run_id: String,
    pub seed: u64,
    pub config_text: String,
    pub config: NormalizedConfig,
    pub paths: RunPaths,
    pub strategies: Vec<StrategyDefinition>,
    pub results: TournamentResults,
    pub event_log: Option<String>,
    pub history_log: Option<String>,
    #[serde(default)]
    pub run_dir: Option<String>,
}

pub const RUN_SUMMARY_SCHEMA_VERSION: u32 = 2;

pub fn write_summary(path: &Path, summary: &RunSummary) -> io::Result<()> {
    write_atomic(path, |writer| {
        serde_json::to_writer_pretty(writer, summary)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    })
}

#[derive(Clone, Debug)]
pub struct RunLayout {
    pub run_dir: PathBuf,
    pub summary_path: PathBuf,
    pub definitions_path: PathBuf,
    pub results_path: PathBuf,
    pub events_path: PathBuf,
    pub history_path: PathBuf,
    pub config_path: PathBuf,
    pub analysis_dir: PathBuf,
}

impl RunLayout {
    pub fn for_base(base_dir: &Path, timestamp: &str, seed: u64, run_id: &str) -> Self {
        let stamp = timestamp.replace(':', "-");
        let runs_root = base_dir.join("runs").join("games");
        let base_name = format!("{stamp}__seed-{seed}");
        let run_dir = unique_run_dir(&runs_root, &base_name, run_id);
        let summary_path = run_dir.join("run_summary.json");
        let definitions_path = run_dir.join("definitions.json");
        let results_path = run_dir.join("results.json");
        let events_path = run_dir.join("events.ndjson");
        let history_path = run_dir.join("history.ndjson");
        let config_path = run_dir.join("config.toml");
        let analysis_dir = run_dir.join("analysis");
        Self {
            run_dir,
            summary_path,
            definitions_path,
            results_path,
            events_path,
            history_path,
            config_path,
            analysis_dir,
        }
    }
}

fn unique_run_dir(root: &Path, base: &str, run_id: &str) -> PathBuf {
    let mut candidate = root.join(base);
    if !candidate.exists() {
        return candidate;
    }
    let suffix = run_id.get(0..8).unwrap_or(run_id);
    let mut with_suffix = format!("{base}__run-{suffix}");
    candidate = root.join(&with_suffix);
    if !candidate.exists() {
        return candidate;
    }
    for attempt in 1..=9 {
        with_suffix = format!("{base}__run-{suffix}-{attempt}");
        candidate = root.join(&with_suffix);
        if !candidate.exists() {
            return candidate;
        }
    }
    root.join(format!("{base}__run-{suffix}-overflow"))
}
