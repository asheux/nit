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
    #[serde(default = "super::default_true")]
    pub genome_context_enabled: bool,
    #[serde(default = "super::default_true")]
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
