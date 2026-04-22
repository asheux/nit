use nit_games::{NormalizedConfig, PayoffMatrix};

pub(super) struct GridCell {
    pub rounds: u32,
    /// Bit-flip probability per round.
    pub noise: f32,
    pub repetitions: u32,
    pub payoff_r: i32,
    pub payoff_s: i32,
    pub payoff_t: i32,
    pub payoff_p: i32,
}

pub(super) struct ParameterGrids {
    pub rounds: Vec<u32>,
    pub noise: Vec<f32>,
    pub repetitions: Vec<u32>,
    pub payoff_r: Vec<i32>,
    pub payoff_s: Vec<i32>,
    pub payoff_t: Vec<i32>,
    pub payoff_p: Vec<i32>,
}

/// Known payoff matrix presets from game theory literature.
#[derive(Debug, Clone, Copy)]
enum PayoffPreset {
    /// Classic Prisoner's Dilemma: temptation to defect dominates.
    PrisonersDilemma,
    /// Stag Hunt: coordination game with Pareto-dominant equilibrium.
    StagHunt,
    /// Snowdrift / Hawk-Dove: anti-coordination with mixed equilibrium.
    Snowdrift,
}

impl PayoffPreset {
    fn from_label(preset_label: &str) -> Option<Self> {
        let canonical: String = preset_label
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .map(|ch| ch.to_ascii_lowercase())
            .collect();
        match canonical.as_str() {
            "pd" | "prisonersdilemma" | "prisoner" => Some(Self::PrisonersDilemma),
            "staghunt" | "stag" => Some(Self::StagHunt),
            "snowdrift" | "snow" | "hawkedove" | "hawkdove" | "chicken" => Some(Self::Snowdrift),
            _ => None,
        }
    }

    /// Canonical (R, S, T, P) values for this preset.
    const fn payoff_values(self) -> (i32, i32, i32, i32) {
        match self {
            Self::PrisonersDilemma => (3, 0, 5, 1),
            Self::StagHunt => (4, 1, 3, 2),
            Self::Snowdrift => (3, 1, 5, 0),
        }
    }
}

/// Build a symmetric 2x2 payoff matrix from the four canonical payoff values.
///
/// R = mutual cooperation reward, S = sucker's payoff,
/// T = temptation to defect, P = mutual defection punishment.
pub(super) fn payoff_from_rstp(
    reward: i32,
    sucker: i32,
    temptation: i32,
    punishment: i32,
) -> PayoffMatrix {
    PayoffMatrix::from_matrix([
        [[reward, reward], [sucker, temptation]],
        [[temptation, sucker], [punishment, punishment]],
    ])
}

/// Resolve CLI grid vectors against config defaults, applying payoff presets if specified.
#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_parameter_grids(
    fallback_config: &NormalizedConfig,
    explicit_rounds: Vec<u32>,
    explicit_noise: Vec<f32>,
    explicit_repetitions: Vec<u32>,
    named_preset: Option<&str>,
    explicit_reward: Vec<i32>,
    explicit_sucker: Vec<i32>,
    explicit_temptation: Vec<i32>,
    explicit_punishment: Vec<i32>,
) -> anyhow::Result<ParameterGrids> {
    let (fallback_reward, fallback_sucker, fallback_temptation, fallback_punishment) =
        match named_preset {
            Some(preset_key) => PayoffPreset::from_label(preset_key)
                .map(|p| p.payoff_values())
                .ok_or_else(|| anyhow::anyhow!("unknown payoff preset '{preset_key}'"))?,
            None => (
                fallback_config.payoff.r,
                fallback_config.payoff.s,
                fallback_config.payoff.t,
                fallback_config.payoff.p,
            ),
        };

    Ok(ParameterGrids {
        rounds: grid_or_default(explicit_rounds, fallback_config.rounds),
        noise: grid_or_default(explicit_noise, fallback_config.noise),
        repetitions: grid_or_default(explicit_repetitions, fallback_config.repetitions),
        payoff_r: grid_or_default(explicit_reward, fallback_reward),
        payoff_s: grid_or_default(explicit_sucker, fallback_sucker),
        payoff_t: grid_or_default(explicit_temptation, fallback_temptation),
        payoff_p: grid_or_default(explicit_punishment, fallback_punishment),
    })
}

/// Resolve a parameter dimension: CLI overrides take precedence,
/// falling back to the single value from the parsed config.
fn grid_or_default<T>(explicit_values: Vec<T>, config_fallback: T) -> Vec<T> {
    if explicit_values.is_empty() {
        vec![config_fallback]
    } else {
        explicit_values
    }
}

/// Build the full Cartesian product of parameter dimensions as a flat grid.
pub(super) fn build_cartesian_grid(space: &ParameterGrids) -> Vec<GridCell> {
    let capacity = space.rounds.len()
        * space.noise.len()
        * space.repetitions.len()
        * space.payoff_r.len()
        * space.payoff_s.len()
        * space.payoff_t.len()
        * space.payoff_p.len();

    let mut grid = Vec::with_capacity(capacity);
    for &rounds in &space.rounds {
        for &noise in &space.noise {
            for &repetitions in &space.repetitions {
                expand_payoff_combinations(&mut grid, rounds, noise, repetitions, space);
            }
        }
    }
    grid
}

/// Expand the payoff-matrix dimensions and append GridCells for one execution triple.
fn expand_payoff_combinations(
    output: &mut Vec<GridCell>,
    round_count: u32,
    noise_level: f32,
    rep_count: u32,
    space: &ParameterGrids,
) {
    for &reward_val in &space.payoff_r {
        for &sucker_val in &space.payoff_s {
            for &temptation_val in &space.payoff_t {
                for &punishment_val in &space.payoff_p {
                    output.push(GridCell {
                        rounds: round_count,
                        noise: noise_level,
                        repetitions: rep_count,
                        payoff_r: reward_val,
                        payoff_s: sucker_val,
                        payoff_t: temptation_val,
                        payoff_p: punishment_val,
                    });
                }
            }
        }
    }
}
