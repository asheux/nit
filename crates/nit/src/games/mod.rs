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

// Re-exports let descendant submodules reach the games-level helpers via `super::`.
use artifacts::write_run_artifacts;
use config_load::{create_parent_dirs, finalize_config, load_games_config, resolve_output_dir};
use tournament::{execute_tournament, TournamentRun};

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

pub(crate) fn games_template() -> &'static str {
    include_str!("templates/games.toml")
}
