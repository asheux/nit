#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HighlightConfig {
    pub enabled: bool,
    pub engine: HighlightEngine,
    pub debounce_ms: u64,
    pub max_file_bytes: usize,
    pub max_spans_per_line: usize,
}

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum HighlightEngine {
    TreeSitter,
    Plain,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EditorConfig {
    pub tab_width: u8,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GolConfig {
    pub enabled: bool,
    pub tick_ms: u64,
    pub wrap: bool,
    pub seed_source: GolSeedSource,
    pub seed_live_chars: String,
    pub seed_other_live_percent: u8,
    pub braille_enabled: bool,
    pub rule: GolRuleConfig,
    pub rules: GolRulesConfig,
    pub search: GolSearchConfig,
    pub snapshots: GolSnapshotsConfig,
}

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum GolSeedSource {
    Editor,
    Notes,
}

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum GolSearchIntensity {
    Low,
    Med,
    High,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GolSearchConfig {
    pub enabled: bool,
    pub intensity: GolSearchIntensity,
    pub rules_per_second: u32,
    pub max_generations: u32,
    pub time_budget_ms_per_tick: u32,
    pub candidate_pool_size: usize,
    pub leaderboard_size: usize,
}

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum SnapshotPrunePolicy {
    Oldest,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GolSnapshotsConfig {
    pub enabled: bool,
    pub max_files: usize,
    pub prune_policy: SnapshotPrunePolicy,
    pub min_period: u32,
    pub min_transient: u32,
    pub min_interval_ms: u64,
    pub snapshot_on_attractor: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GolRuleConfig {
    pub default: String,
    pub workspace_override: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
pub struct GolRulesConfig {
    pub user: Vec<GolUserRule>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GolUserRule {
    pub id: String,
    pub name: String,
    pub rule: String,
    pub description: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GenomeGateConfig {
    pub min_tier: u8,
    pub max_density: f32,
    pub min_components: usize,
    pub min_consistency: f32,
    pub require_no_regression: bool,
}

impl Default for GenomeGateConfig {
    fn default() -> Self {
        Self {
            min_tier: 1, // Oscillator
            max_density: 0.45,
            min_components: 3,
            min_consistency: 0.4,
            require_no_regression: true,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentGenomeConfig {
    #[serde(default = "default_true")]
    pub genome_context_enabled: bool,
    #[serde(default = "default_true")]
    pub genome_gate_enabled: bool,
    #[serde(default)]
    pub genome_gate: GenomeGateConfig,
}

impl Default for AgentGenomeConfig {
    fn default() -> Self {
        Self {
            genome_context_enabled: true,
            genome_gate_enabled: true,
            genome_gate: GenomeGateConfig::default(),
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
pub struct Settings {
    pub highlight: HighlightConfig,
    pub editor: EditorConfig,
    pub gol: GolConfig,
    #[serde(default)]
    pub genome: AgentGenomeConfig,
}

impl Default for HighlightConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            engine: HighlightEngine::TreeSitter,
            debounce_ms: 50,
            max_file_bytes: 2_000_000,
            max_spans_per_line: 256,
        }
    }
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self { tab_width: 4 }
    }
}

impl Default for GolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            tick_ms: 120,
            wrap: false,
            seed_source: GolSeedSource::Editor,
            seed_live_chars: "#@█▓▒░*+xX%&".to_string(),
            seed_other_live_percent: 50,
            braille_enabled: true,
            rule: GolRuleConfig::default(),
            rules: GolRulesConfig::default(),
            search: GolSearchConfig::default(),
            snapshots: GolSnapshotsConfig::default(),
        }
    }
}

impl Default for GolSearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            intensity: GolSearchIntensity::Low,
            rules_per_second: 0,
            max_generations: 300,
            time_budget_ms_per_tick: 8,
            candidate_pool_size: 32,
            leaderboard_size: 10,
        }
    }
}

impl Default for GolSnapshotsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_files: 500,
            prune_policy: SnapshotPrunePolicy::Oldest,
            min_period: 6,
            min_transient: 20,
            min_interval_ms: 1000,
            snapshot_on_attractor: true,
        }
    }
}

impl Default for GolRuleConfig {
    fn default() -> Self {
        Self {
            default: "conway".to_string(),
            workspace_override: true,
        }
    }
}

// Defaults for GolRulesConfig and Settings are derived.
