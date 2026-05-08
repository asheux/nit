use super::editor::EditorConfig;
use super::genome::AgentGenomeConfig;
use super::gol::GolConfig;
use super::highlight::HighlightConfig;
use super::swarm::SwarmConfig;

/// Top-level merged config — global TOML layered with the per-workspace
/// override. `#[serde(default)]` on the optional sub-blocks lets older config
/// files that predate a section keep deserializing.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub highlight: HighlightConfig,
    pub editor: EditorConfig,
    pub gol: GolConfig,
    #[serde(default)]
    pub genome: AgentGenomeConfig,
    #[serde(default)]
    pub swarm: SwarmConfig,
    /// LLM intake classifier — runs before each chat dispatch to decide
    /// whether a FILE CHECKLIST is appended. Override at runtime with
    /// `NIT_INTAKE_DISABLED=1`, or persist `intake_enabled = false`.
    #[serde(default = "super::default_true")]
    pub intake_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            highlight: HighlightConfig::default(),
            editor: EditorConfig::default(),
            gol: GolConfig::default(),
            genome: AgentGenomeConfig::default(),
            swarm: SwarmConfig::default(),
            intake_enabled: super::default_true(),
        }
    }
}
