#![forbid(unsafe_code)]

mod agents;
mod bootstrap;
mod cli;
mod games;
mod graph;
mod logging;
mod multipane_setup;
mod workspace;

use std::path::Path;
use std::sync::mpsc;

use clap::Parser;
use nit_core::{AppKind, AppState, SubstrateState};
use nit_tui::{run, Theme};

use crate::cli::{AgentsArg, Cli, Command};
use crate::logging::{init_tracing, install_panic_hook, log_path_for_workspace};
use crate::workspace::{find_theme, load_notes, open_target_games, open_target_gol};

fn main() -> anyhow::Result<()> {
    let mut cli = Cli::parse_from(cli::normalize_lab_args(std::env::args()));

    if let Some(result) = try_dispatch_games(&mut cli.command) {
        return result;
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
    let (app_state, theme, log_receiver) =
        prepare_app_state(app_kind, target_path.as_deref(), backend_selection)?;

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

// take()/restore: if the games subcommand isn't present we put the value
// back so the multipane / lab dispatcher downstream still sees it.
fn try_dispatch_games(command: &mut Option<Command>) -> Option<anyhow::Result<()>> {
    match command.take() {
        Some(Command::Games {
            command: Some(games_cmd),
            ..
        }) => Some(games::dispatch_subcommand(games_cmd)),
        other => {
            *command = other;
            None
        }
    }
}

fn prepare_app_state(
    app_kind: AppKind,
    target_path: Option<&Path>,
    agents: Option<AgentsArg>,
) -> anyhow::Result<(AppState, Theme, mpsc::Receiver<String>)> {
    let (workspace_root, editor_buffer) = match app_kind {
        AppKind::Gol => open_target_gol(target_path)?,
        AppKind::Games => open_target_games(target_path)?,
    };

    let theme = Theme::load(find_theme().as_deref());

    // Tracing must be wired before any subprocess spawn or panic so the
    // log channel captures startup diagnostics; install_panic_hook then
    // routes panics through the same writer.
    let (log_sender, log_receiver) = mpsc::channel::<String>();
    init_tracing(log_sender, log_path_for_workspace(&workspace_root))?;
    install_panic_hook();

    let notes_buffer = load_notes(&workspace_root);
    let mut state = AppState::new(workspace_root, editor_buffer, notes_buffer);
    state.substrate = SubstrateState::load(&state.workspace_root);
    bootstrap::configure_app_state(&mut state, agents, app_kind, target_path);

    if app_kind == AppKind::Gol {
        bootstrap::init_gol_rules(&mut state);
    }

    Ok((state, theme, log_receiver))
}

#[cfg(test)]
#[path = "tests/main.rs"]
mod tests;
