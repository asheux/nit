//! Genome quality gate + per-agent genome context settings.
//!
//! The genome gate runs after each agent turn writes to disk: nit re-encodes
//! the touched files, simulates them as Game of Life seeds, and rejects the
//! turn if the structural metrics regress past the configured floor. Lowering
//! the thresholds makes the gate more forgiving (good for warm-up runs);
//! raising them tightens the bar for "elite" code.

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GenomeGateConfig {
    /// Lowest acceptable tier (1=Still Life … 5=Replicator). Turns whose worst
    /// touched file falls below this floor fail the gate.
    #[serde(default = "default_min_tier")]
    pub min_tier: u8,
    /// Reject the turn if any file's structural density exceeds this fraction
    /// — a high density typically signals dead/duplicated code regions.
    #[serde(default = "default_max_density")]
    pub max_density: f32,
    /// Minimum AST-component count per file. Below this the file is too
    /// monolithic for the encoders to score reliably.
    #[serde(default = "default_min_components")]
    pub min_components: usize,
    /// Cross-encoder agreement floor. Low consistency means one encoder
    /// disagrees sharply with the others — usually parsimony bloat.
    #[serde(default = "default_min_consistency")]
    pub min_consistency: f32,
    /// When true, even a passing absolute score fails if it regresses against
    /// the pre-edit baseline (prevents "make it worse but still acceptable").
    #[serde(default = "super::default_true")]
    pub require_no_regression: bool,
}

impl Default for GenomeGateConfig {
    fn default() -> Self {
        Self {
            min_tier: default_min_tier(),
            max_density: default_max_density(),
            min_components: default_min_components(),
            min_consistency: default_min_consistency(),
            require_no_regression: super::default_true(),
        }
    }
}

/// Per-agent genome integration: whether to surface genome scores to agents
/// in their context, and whether to enforce the gate on their writes.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentGenomeConfig {
    /// Inject the current file's genome report into the agent's prompt context.
    /// Disable for backends that have small context windows or do not benefit
    /// from structural feedback.
    #[serde(default = "super::default_true", rename = "genome_context_enabled")]
    pub genome_context_enabled: bool,
    /// Enforce `genome_gate` rejection on this agent's writes. Disable for
    /// exploratory or planning-only agents that should never be auto-retried
    /// on quality grounds.
    #[serde(default = "super::default_true", rename = "genome_gate_enabled")]
    pub genome_gate_enabled: bool,
    #[serde(default)]
    pub genome_gate: GenomeGateConfig,
}

impl Default for AgentGenomeConfig {
    fn default() -> Self {
        Self {
            genome_context_enabled: super::default_true(),
            genome_gate_enabled: super::default_true(),
            genome_gate: GenomeGateConfig::default(),
        }
    }
}

fn default_min_tier() -> u8 {
    1
}

fn default_max_density() -> f32 {
    0.45
}

fn default_min_components() -> usize {
    3
}

fn default_min_consistency() -> f32 {
    0.4
}
