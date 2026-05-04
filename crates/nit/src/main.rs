#![forbid(unsafe_code)]

mod agents;
mod bootstrap;
mod cli;
mod games;
mod graph;
mod logging;
mod multipane_setup;
mod workspace;

use std::sync::mpsc;

use clap::Parser;
use nit_core::{AppKind, AppState, SubstrateState};
use nit_tui::{run, Theme};

use crate::cli::{Cli, Command};
use crate::logging::{init_tracing, install_panic_hook, log_path_for_workspace};
use crate::workspace::{find_theme, load_notes, open_target_games, open_target_gol};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse_from(cli::normalize_lab_args(std::env::args()));

    if let Some(Command::Games {
        command: Some(games_cmd),
        ..
    }) = cli.command
    {
        return games::dispatch_subcommand(games_cmd);
    }

    let (runtime_mode, codex_runner_config, claude_runner_config) =
        bootstrap::build_runner_configs(&cli);
    let backend_selection = cli.agents;
    let resolved = match cli.command {
        Some(Command::Multipane(args)) => {
            return multipane_setup::run_multipane(
                args,
                runtime_mode,
                codex_runner_config,
                claude_runner_config,
            );
        }
        other => other,
    };
    let (app_kind, target_path) = bootstrap::resolve_app_target(resolved, cli.lab, cli.path);

    let (workspace_root, editor_buffer) = match app_kind {
        AppKind::Gol => open_target_gol(target_path.as_deref())?,
        AppKind::Games => open_target_games(target_path.as_deref())?,
    };

    let theme = Theme::load(find_theme().as_deref());

    let (log_sender, log_receiver) = mpsc::channel::<String>();
    init_tracing(log_sender, log_path_for_workspace(&workspace_root))?;
    install_panic_hook();

    let notes_buffer = load_notes(&workspace_root);
    let mut app_state = AppState::new(workspace_root, editor_buffer, notes_buffer);
    app_state.substrate = SubstrateState::load(&app_state.workspace_root);
    bootstrap::configure_app_state(
        &mut app_state,
        backend_selection,
        app_kind,
        target_path.as_deref(),
    );

    if app_kind == AppKind::Gol {
        bootstrap::init_gol_rules(&mut app_state);
    }

    run(
        app_state,
        theme,
        log_receiver,
        runtime_mode,
        codex_runner_config,
        claude_runner_config,
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/main.rs"]
mod tests;
