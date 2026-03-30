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
use nit_tui::{run, Theme};
use nit_utils::hashing::stable_hash_bytes;

use crate::cli::{AgentsArg, Cli, Command};
use crate::logging::{init_tracing, install_panic_hook, log_path_for_workspace};
use crate::workspace::{
    export_legacy_notes_snapshot, find_theme, load_notes, open_target_games, open_target_gol,
};

/// Entry point: parse CLI, open workspace, initialize state, and launch the TUI.
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
    let runtime_mode = nit_tui::codex_runner::CodexRuntimeMode::from(cli.codex_runtime);
    let codex_runner_config = nit_tui::codex_runner::CodexRunnerConfig {
        sandbox: cli
            .codex_sandbox
            .map(|sandbox_arg| sandbox_arg.as_str().to_string()),
        approval_policy: Some(cli.codex_approval_policy.as_str().to_string()),
        max_parallel_turns: cli.codex_max_parallel_turns as usize,
    };
    let claude_runner_config = nit_tui::claude_runner::ClaudeRunnerConfig {
        max_parallel_turns: cli.codex_max_parallel_turns as usize,
        permission_mode: None,
    };
    let agents_selection = cli.agents;

    // Resolve the application mode and filesystem target from CLI arguments.
    let (app_kind, target_path) = match cli.command {
        Some(Command::Gol { path }) => (AppKind::Gol, path),
        Some(Command::Games { path, .. }) => (AppKind::Games, path),
        None => (nit_core::LabId::from(cli.lab), cli.path),
    };

    let (workspace_root, editor_buffer) = match app_kind {
        AppKind::Gol => open_target_gol(target_path.as_deref())?,
        AppKind::Games => open_target_games(target_path.as_deref())?,
    };

    // Theme and logging setup.
    let theme = Theme::load(find_theme().as_deref());

    let (log_sender, log_receiver) = mpsc::channel::<String>();
    init_tracing(log_sender, log_path_for_workspace(&workspace_root))?;
    install_panic_hook();

    // Build application state and apply configuration.
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

/// Populate the agent roster, seed the visualizer, and optionally open the file tree.
fn configure_app_state(
    state: &mut nit_core::AppState,
    agent_selection: Option<AgentsArg>,
    requested_kind: AppKind,
    target_path: Option<&std::path::Path>,
) {
    let agent_backend = agent_selection.unwrap_or(AgentsArg::All);
    state.agents = agents::init_agents(agent_backend);
    state.app_kind = requested_kind;
    state.visualizer.seed = stable_hash_bytes(state.editor_buffer().content_as_string().as_bytes());
    state.mode = Mode::Normal;

    // Preserve legacy notes as a snapshot if any exist.
    if let Some(snapshot_path) =
        export_legacy_notes_snapshot(&state.workspace_root, state.notes_buffer())
    {
        state.agents.pending_legacy_notes_alert = Some(format!(
            "Legacy Notes were preserved in {} and are available in Agent Ops > Scratchpad.",
            snapshot_path.display()
        ));
    }

    // Open the file tree when launching into a directory rather than a file.
    if target_path.is_none_or(|p| p.is_dir()) {
        state.file_tree.root = state.workspace_root.clone();
        state.file_tree.open = true;
        state.focus = PaneId::Editor;
    }
}

/// Load the GoL rule catalog from disk and wire the selected rule into application state.
fn init_gol_rules(app_state: &mut nit_core::AppState) {
    let persisted_config = nit_core::load_rule_config(&app_state.workspace_root);

    // Parse user-defined rules and surface any diagnostics.
    let (catalog, mut diagnostics) = nit_core::load_rule_catalog(&persisted_config.rules.user);
    diagnostics.extend(persisted_config.warnings);
    for message in diagnostics {
        tracing::warn!("{message}");
    }

    // Resolve which rule to activate: workspace-level override takes priority over global default.
    let rule_preferences = &persisted_config.rule;
    let selected_key = if rule_preferences.workspace_override {
        persisted_config
            .workspace_rule
            .clone()
            .unwrap_or_else(|| rule_preferences.default.clone())
    } else {
        rule_preferences.default.clone()
    };

    let resolved_rule = catalog.select(&selected_key).unwrap_or_else(|err| {
        tracing::warn!("Invalid configured GoL rule '{selected_key}': {err}");
        nit_core::SelectedRule::default()
    });

    // Persist settings and hand the catalog + resolved rule to the simulation engine.
    app_state.settings.gol.rule = rule_preferences.clone();
    app_state.settings.gol.rules = persisted_config.rules.clone();
    app_state.init_rules(
        catalog,
        resolved_rule,
        nit_core::RulePersistence {
            global_path: persisted_config.global_path,
            workspace_path: persisted_config.workspace_path,
            workspace_override: rule_preferences.workspace_override,
        },
    );
}

#[cfg(test)]
#[path = "tests/main.rs"]
mod tests;
