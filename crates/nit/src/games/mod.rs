//! Games subcommand dispatch: routes CLI commands to tournament execution, parameter sweeps,
//! inspection, graphing, and strategy enumeration handlers.

mod artifacts;
mod config_load;
mod enumerate;
mod inspect;
mod run;
mod sweep;
mod tournament;

use crate::cli::{EnumerateCommand, GamesCommand};

// Re-imports flatten the games-level helpers so descendant submodules
// can reach them through `super::`.
use artifacts::write_run_artifacts;
use config_load::{create_parent_dirs, finalize_config, load_games_config, resolve_output_dir};
use tournament::{execute_tournament, TournamentRun};

/// Route a games subcommand to the appropriate handler.
pub(crate) fn dispatch_subcommand(cmd: GamesCommand) -> anyhow::Result<()> {
    match cmd {
        GamesCommand::Run(args) => run::run_games_headless(args),
        GamesCommand::Sweep(args) => sweep::run_games_sweep(args),
        GamesCommand::Inspect(args) => inspect::run_games_inspect(args),
        GamesCommand::Graph(args) => inspect::run_games_graph(args),
        GamesCommand::Enumerate { kind } => match kind {
            EnumerateCommand::Fsm {
                states,
                out,
                canonical,
                limit,
                input_mode,
            } => enumerate::run_games_enumerate_fsm(&states, &out, canonical, limit, input_mode),
        },
    }
}

/// Default TOML template for new games workspaces.
pub(crate) fn games_template() -> &'static str {
    r#"schema_version = 1
game = "ipd"
rounds = 200
repetitions = 1
self_play = true
save_data = true
seed = 12345
noise = 0.0

[payoff]
R = -1
S = -3
T = 0
P = -2

[history]
enabled = true
include_cycle_metadata = false

[engine]
mode = "interactive"
parallelism = "auto"
progress_interval_ms = 80
fast_eval = true
score_aggregation = "mean" # Code-02 semantics: per-round average score; TotalPayoff sums matchup means

[engine.complexity_cost]
enabled = false
tm_step_cost = 0.0
fsm_state_cost = 0.0

[[strategy]]
id = "fsm_tft"
type = "auto"
states = 2
start_state = 1
input_index_base = 1
outputs = ["C", "D"]
transitions = [
  [1, 2],
  [1, 2],
]

[[strategy]]
id = "fsm_index_allc"
type = "fsm"
index = 1
num_states = 1
k = 2

[[strategy]]
id = "ca_rule30"
type = "ca"
n = 30
k = 2
r = 1
t = 3

[[strategy]]
id = "tm_rule_3111"
type = "auto"
states = 2
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 128
rule_code = 3111
"#
}
