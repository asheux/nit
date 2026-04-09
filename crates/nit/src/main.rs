#![forbid(unsafe_code)]

mod agents;
mod cli;
mod games;
mod graph;
mod logging;
mod workspace;

use std::sync::mpsc;

use clap::Parser;
use nit_core::{AppKind, Mode, PaneId};
use nit_tui::claude_runner::ClaudeRunnerConfig;
use nit_tui::codex_runner::{CodexRunnerConfig, CodexRuntimeMode};
use nit_tui::{run, Theme};
use nit_utils::hashing::stable_hash_bytes;

use crate::cli::{AgentsArg, Cli, Command};
use crate::logging::{init_tracing, install_panic_hook, log_path_for_workspace};
use crate::workspace::{
    export_legacy_notes_snapshot, find_theme, load_notes, open_target_games, open_target_gol,
};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse_from(cli::normalize_lab_args(std::env::args()));

    // Headless games subcommands — early return, no TUI.
    if let Some(Command::Games {
        command: Some(games_cmd),
        ..
    }) = cli.command
    {
        return games::dispatch_subcommand(games_cmd);
    }

    // Build runner configs while `cli` is fully intact (before partial moves).
    let runtime_mode = CodexRuntimeMode::from(cli.codex_runtime);
    let parallel_turns = cli.codex_max_parallel_turns as usize;
    let codex_runner_config = CodexRunnerConfig {
        sandbox: cli.codex_sandbox.map(|s| s.as_str().to_string()),
        approval_policy: Some(cli.codex_approval_policy.as_str().to_string()),
        max_parallel_turns: parallel_turns,
    };
    let claude_runner_config = ClaudeRunnerConfig {
        max_parallel_turns: parallel_turns,
        permission_mode: None,
    };
    let agents_selection = cli.agents;

    // Resolve the application mode and filesystem target.
    let (app_kind, target_path) = match cli.command {
        Some(Command::Gol { path }) => (AppKind::Gol, path),
        Some(Command::Games { path, .. }) => (AppKind::Games, path),
        None => (nit_core::LabId::from(cli.lab), cli.path),
    };

    let (workspace_root, editor_buffer) = match app_kind {
        AppKind::Gol => open_target_gol(target_path.as_deref())?,
        AppKind::Games => open_target_games(target_path.as_deref())?,
    };

    let theme = Theme::load(find_theme().as_deref());

    let (log_sender, log_receiver) = mpsc::channel::<String>();
    init_tracing(log_sender, log_path_for_workspace(&workspace_root))?;
    install_panic_hook();

    let notes_buffer = load_notes(&workspace_root);
    let mut app_state = nit_core::AppState::new(workspace_root, editor_buffer, notes_buffer);
    configure_app_state(
        &mut app_state,
        agents_selection,
        app_kind,
        target_path.as_deref(),
    );

    if app_kind == AppKind::Gol {
        init_gol_rules(&mut app_state);
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

fn configure_app_state(
    state: &mut nit_core::AppState,
    agent_selection: Option<AgentsArg>,
    requested_kind: AppKind,
    target_path: Option<&std::path::Path>,
) {
    state.agents = agents::init_agents(agent_selection.unwrap_or(AgentsArg::All));
    state.app_kind = requested_kind;
    state.visualizer.seed = stable_hash_bytes(state.editor_buffer().content_as_string().as_bytes());
    state.mode = Mode::Normal;

    if let Some(snapshot_path) =
        export_legacy_notes_snapshot(&state.workspace_root, state.notes_buffer())
    {
        state.agents.pending_legacy_notes_alert = Some(format!(
            "Legacy Notes were preserved in {} and are available in Agent Ops > Scratchpad.",
            snapshot_path.display()
        ));
    }

    // Open the file tree when launching into a directory rather than a single file.
    if target_path.is_none_or(|p| p.is_dir()) {
        state.file_tree.root = state.workspace_root.clone();
        state.file_tree.open = true;
        state.focus = PaneId::Editor;
    }
}

/// Load the GoL rule catalog and resolve the active rule from persisted config.
fn init_gol_rules(state: &mut nit_core::AppState) {
    let persisted = nit_core::load_rule_config(&state.workspace_root);

    let (catalog, mut diagnostics) = nit_core::load_rule_catalog(&persisted.rules.user);
    diagnostics.extend(persisted.warnings);
    for message in diagnostics {
        tracing::warn!("{message}");
    }

    // Workspace-level override takes priority over the global default.
    let prefs = &persisted.rule;
    let selected_key = if prefs.workspace_override {
        persisted
            .workspace_rule
            .clone()
            .unwrap_or_else(|| prefs.default.clone())
    } else {
        prefs.default.clone()
    };

    let resolved_rule = catalog.select(&selected_key).unwrap_or_else(|err| {
        tracing::warn!("invalid GoL rule '{selected_key}': {err}");
        nit_core::SelectedRule::default()
    });

    state.settings.gol.rule = prefs.clone();
    state.settings.gol.rules = persisted.rules.clone();
    state.init_rules(
        catalog,
        resolved_rule,
        nit_core::RulePersistence {
            global_path: persisted.global_path,
            workspace_path: persisted.workspace_path,
            workspace_override: prefs.workspace_override,
        },
    );
}

#[cfg(test)]
#[path = "tests/main.rs"]
mod tests;
