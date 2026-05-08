//! Top-level Game of Life visualizer config.
//!
//! Concrete budget knobs (search workload, snapshot retention) live in
//! sibling submodules so the top-level `Settings.gol` block stays readable
//! at a glance.

pub mod search;
pub mod snapshots;

pub use search::{GolSearchConfig, GolSearchIntensity};
pub use snapshots::{GolSnapshotsConfig, SnapshotPrunePolicy};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GolConfig {
    pub enabled: bool,
    /// Wall-clock pacing for one generation step. Lower = faster simulation,
    /// higher = friendlier to slow terminals.
    pub tick_ms: u64,
    /// When true, edges of the grid wrap around (toroidal topology); when
    /// false, off-grid neighbors count as dead.
    pub wrap: bool,
    pub seed_source: GolSeedSource,
    /// Characters in the editor buffer that are interpreted as live cells
    /// when seeding from text. Anything outside this set is treated as dead.
    pub seed_live_chars: String,
    /// Probability (0..=100) that a non-live-char glyph is randomly promoted
    /// to a live cell when seeding. Adds entropy on sparse buffers.
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

/// Selected GoL rule + whether the workspace `.nit/config.toml` is allowed to
/// override the user-global default. Kept as its own struct (not merged into
/// `GolRulesConfig`) because `rule_config::load` exposes it standalone in
/// `RuleConfigLoad.rule` for the rule-resolution pipeline.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GolRuleConfig {
    pub default: String,
    pub workspace_override: bool,
}

impl Default for GolRuleConfig {
    fn default() -> Self {
        Self {
            default: "conway".to_string(),
            workspace_override: true,
        }
    }
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
