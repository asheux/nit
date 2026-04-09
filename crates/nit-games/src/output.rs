//! Tournament output types, run layout, and summary serialisation.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{AcceleratorMode, NormalizedConfig, ScoreAggregation, StrategySpecKind};
use nit_utils::fs::write_atomic;
use nit_utils::hashing::stable_hash_bytes;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjusted_total_payoff: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjusted_average_payoff: Option<f64>,
    pub matches: u32,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub crashed: bool,
    pub crash_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tm_metrics: Option<TmDerivedMetrics>,
}

impl StrategyResult {
    pub fn score(&self, aggregation: ScoreAggregation, adjusted: bool) -> f64 {
        match (aggregation, adjusted) {
            (ScoreAggregation::Mean, true) => {
                self.adjusted_average_payoff.unwrap_or(self.average_payoff)
            }
            (ScoreAggregation::Total, true) => self
                .adjusted_total_payoff
                .unwrap_or(self.total_payoff as f64),
            (ScoreAggregation::Mean, false) => self.average_payoff,
            (ScoreAggregation::Total, false) => self.total_payoff as f64,
        }
    }

    pub fn total_payoff_for_scoreboard(
        &self,
        aggregation: ScoreAggregation,
        adjusted: bool,
    ) -> f64 {
        match aggregation {
            ScoreAggregation::Mean => self.score(aggregation, adjusted) * self.matches as f64,
            ScoreAggregation::Total => self.score(aggregation, adjusted),
        }
    }

    pub fn formatted_score(&self, aggregation: ScoreAggregation, adjusted: bool) -> String {
        match (aggregation, adjusted) {
            (ScoreAggregation::Total, false) => self.total_payoff.to_string(),
            _ => format_score_value(self.score(aggregation, adjusted)),
        }
    }

    pub fn formatted_total_payoff(&self, aggregation: ScoreAggregation, adjusted: bool) -> String {
        format_score_value(self.total_payoff_for_scoreboard(aggregation, adjusted))
    }
}

pub fn format_score_value(value: f64) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    if value.abs() < 1e-9 {
        return "0".to_string();
    }
    let rounded = value.round();
    if (value - rounded).abs() < 1e-9 {
        return (rounded as i64).to_string();
    }
    let mut formatted = format!("{value:.3}");
    while formatted.contains('.') && formatted.ends_with('0') {
        formatted.pop();
    }
    if formatted.ends_with('.') {
        formatted.pop();
    }
    if formatted == "-0" {
        "0".to_string()
    } else {
        formatted
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TmDerivedMetrics {
    pub rounds: u64,
    pub avg_steps_per_move: f64,
    #[serde(default)]
    pub min_steps_per_move: u32,
    #[serde(default)]
    pub max_steps_per_move: u32,
    pub max_steps_hit_count: u64,
    pub output_event_hit_rate: f64,
    pub fallback_rate: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairwiseResult {
    pub a: String,
    pub b: String,
    pub a_total: i64,
    pub b_total: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a_adjusted_total: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b_adjusted_total: Option<f64>,
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

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAcceleratorBackend {
    #[default]
    None,
    Cpu,
    Metal,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RuntimeAcceleratorStats {
    #[serde(default)]
    pub requested: AcceleratorMode,
    #[serde(default)]
    pub backend: RuntimeAcceleratorBackend,
    #[serde(default)]
    pub metal_batches: u64,
    #[serde(default)]
    pub metal_matches: u64,
    #[serde(default)]
    pub cpu_matches: u64,
    #[serde(default)]
    pub metal_fallbacks: u64,
    #[serde(default)]
    pub metal_fallback_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metal_matches_per_batch: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metal_inflight_batches: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metal_policy_source: Option<nit_metal::BatchPolicySource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metal_policy_cache_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metal_policy_cache_path: Option<String>,
}

impl RuntimeAcceleratorStats {
    pub fn new(requested: AcceleratorMode) -> Self {
        Self {
            requested,
            ..Self::default()
        }
    }

    pub fn note_cpu_matches(&mut self, matches: usize) {
        self.cpu_matches = self.cpu_matches.saturating_add(matches as u64);
        if matches > 0 && matches!(self.backend, RuntimeAcceleratorBackend::None) {
            self.backend = RuntimeAcceleratorBackend::Cpu;
        }
    }

    pub fn note_cpu_activity(&mut self) {
        if matches!(self.backend, RuntimeAcceleratorBackend::None) {
            self.backend = RuntimeAcceleratorBackend::Cpu;
        }
    }

    pub fn note_metal_batch(&mut self, matches: usize) {
        self.metal_batches = self.metal_batches.saturating_add(1);
        self.metal_matches = self.metal_matches.saturating_add(matches as u64);
        self.backend = RuntimeAcceleratorBackend::Metal;
    }

    pub fn note_metal_batches(&mut self, batches: usize, matches: usize) {
        self.metal_batches = self.metal_batches.saturating_add(batches as u64);
        self.metal_matches = self.metal_matches.saturating_add(matches as u64);
        if batches > 0 || matches > 0 {
            self.backend = RuntimeAcceleratorBackend::Metal;
        }
    }

    pub fn note_metal_policy(
        &mut self,
        matches_per_batch: usize,
        inflight_batches: usize,
        source: nit_metal::BatchPolicySource,
        cache_key: Option<String>,
        cache_path: Option<String>,
    ) {
        self.metal_matches_per_batch = Some(matches_per_batch.min(u32::MAX as usize) as u32);
        self.metal_inflight_batches = Some(inflight_batches.min(u32::MAX as usize) as u32);
        self.metal_policy_source = Some(source);
        self.metal_policy_cache_key = cache_key;
        self.metal_policy_cache_path = cache_path;
    }

    pub fn note_metal_fallback(&mut self) {
        self.metal_fallbacks = self.metal_fallbacks.saturating_add(1);
    }

    pub fn note_metal_fallback_reason(&mut self, reason: impl Into<String>) {
        self.note_metal_fallback();
        self.metal_fallback_reason = Some(reason.into());
    }

    pub fn metal_policy_source_label(&self) -> Option<&'static str> {
        match self.metal_policy_source? {
            nit_metal::BatchPolicySource::Heuristic => Some("default"),
            nit_metal::BatchPolicySource::Cached => Some("cached"),
            nit_metal::BatchPolicySource::Benchmarked => Some("tuned"),
        }
    }
}

/// Each path is `Option<String>` so older schema versions still deserialise.
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
    pub runtime: RuntimeAcceleratorStats,
    #[serde(default)]
    pub run_dir: Option<String>,
}

pub const RUN_SUMMARY_SCHEMA_VERSION: u32 = 2;

pub fn write_summary(path: &Path, summary: &RunSummary) -> io::Result<()> {
    write_atomic(path, |writer| {
        serde_json::to_writer_pretty(writer, summary).map_err(io::Error::other)
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

/// Find a unique directory name under `root` for this run.
///
/// Tries, in order: `<base>`, `<base>__run-<suffix>`, then
/// `<base>__run-<suffix>-1` through `-9`, and finally falls back to
/// `<base>__run-<suffix>-overflow`.
fn unique_run_dir(root: &Path, base: &str, run_id: &str) -> PathBuf {
    let candidate = root.join(base);
    if !candidate.exists() {
        return candidate;
    }

    let suffix = run_id.get(0..8).unwrap_or(run_id);

    // Try the plain suffixed name, then numbered variants 1..=9.
    let names = std::iter::once(format!("{base}__run-{suffix}"))
        .chain((1..=9).map(|n| format!("{base}__run-{suffix}-{n}")));

    for name in names {
        let candidate = root.join(&name);
        if !candidate.exists() {
            return candidate;
        }
    }

    root.join(format!("{base}__run-{suffix}-overflow"))
}
