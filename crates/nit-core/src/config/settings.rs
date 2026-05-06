use super::editor::EditorConfig;
use super::genome::AgentGenomeConfig;
use super::gol::GolConfig;
use super::highlight::HighlightConfig;
use super::swarm::SwarmConfig;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub highlight: HighlightConfig,
    pub editor: EditorConfig,
    pub gol: GolConfig,
    #[serde(default)]
    pub genome: AgentGenomeConfig,
    #[serde(default)]
    pub swarm: SwarmConfig,
    /// LLM-based "intake" agent runs before each chat dispatch to classify
    /// the prompt and decide whether to append a FILE CHECKLIST. Defaults
    /// on; set `NIT_INTAKE_DISABLED=1` for the runtime kill switch, or
    /// `intake_enabled = false` in config to opt out persistently.
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
