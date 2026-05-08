//! Background rule-search budget — how aggressively the visualizer hunts for
//! interesting Life rules while the seed simulation is idle.

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GolSearchConfig {
    pub enabled: bool,
    pub intensity: GolSearchIntensity,
    /// Hard rate cap, applied across all candidate worker threads. `0` disables
    /// the cap and lets the worker pool spin as fast as the time budget allows.
    pub rules_per_second: u32,
    /// Maximum simulation generations to run for any single candidate before
    /// scoring and discarding it.
    pub max_generations: u32,
    /// Per-tick cooperative deadline: the search loop yields once it has spent
    /// this many milliseconds on the current tick, even if generations remain.
    pub time_budget_ms_per_tick: u32,
    /// In-flight candidate buffer size. Larger pools improve diversity but
    /// raise peak memory per tick.
    pub candidate_pool_size: usize,
    /// How many top-scoring rules the leaderboard retains for the UI panel.
    pub leaderboard_size: usize,
}

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum GolSearchIntensity {
    Low,
    Med,
    High,
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
