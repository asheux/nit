use crate::config::{NormalizedConfig, StrategySpecKind};
use nit_utils::fs::write_atomic;
use nit_utils::hashing::stable_hash_bytes;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

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
}

pub fn write_summary(path: &Path, summary: &RunSummary) -> io::Result<()> {
    write_atomic(path, |writer| {
        serde_json::to_writer_pretty(writer, summary)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    })
}
