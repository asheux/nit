#![forbid(unsafe_code)]

mod agents;
mod cli;
mod games;
mod graph;
mod logging;
mod workspace;

use std::path::Path;
use std::sync::mpsc;

use clap::Parser;
use nit_core::{AppKind, AppState, LabId, Mode, PaneId, SubstrateState};
use nit_tui::claude_runner::ClaudeRunnerConfig;
use nit_tui::codex_runner::{CodexRunnerConfig, CodexRuntimeMode};
use nit_tui::multipane;
use nit_tui::{run, Theme};
use nit_utils::hashing::stable_hash_bytes;

use crate::cli::{AgentsArg, Cli, Command, MultipaneArgs};
use crate::logging::{init_tracing, install_panic_hook, log_path_for_workspace};
use crate::workspace::{
    export_legacy_notes_snapshot, find_theme, load_notes, open_target_games, open_target_gol,
};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse_from(cli::normalize_lab_args(std::env::args()));

    if let Some(Command::Games {
        command: Some(games_cmd),
        ..
    }) = cli.command
    {
        return games::dispatch_subcommand(games_cmd);
    }

    let (runtime_mode, codex_runner_config, claude_runner_config) = build_runner_configs(&cli);
    let backend_selection = cli.agents;
    let resolved = match cli.command {
        Some(Command::Multipane(args)) => {
            return run_multipane(
                args,
                runtime_mode,
                codex_runner_config,
                claude_runner_config,
            );
        }
        other => other,
    };
    let (app_kind, target_path) = resolve_app_target(resolved, cli.lab, cli.path);

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
    configure_app_state(
        &mut app_state,
        backend_selection,
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

fn build_runner_configs(cli: &Cli) -> (CodexRuntimeMode, CodexRunnerConfig, ClaudeRunnerConfig) {
    let runtime_mode = CodexRuntimeMode::from(cli.codex_runtime);
    let max_turns = cli.codex_max_parallel_turns as usize;
    let codex = CodexRunnerConfig {
        sandbox: cli.codex_sandbox.map(|s| s.as_str().to_string()),
        approval_policy: Some(cli.codex_approval_policy.as_str().to_string()),
        max_parallel_turns: max_turns,
        // mcp_backchannel_socket is filled in by nit-tui's app::run once the
        // back-channel listener has bound its UDS path.
        mcp_backchannel_socket: None,
    };
    let claude = ClaudeRunnerConfig {
        max_parallel_turns: max_turns,
        permission_mode: None,
    };
    (runtime_mode, codex, claude)
}

fn resolve_app_target(
    command: Option<Command>,
    lab: cli::LabArg,
    fallback_path: Option<std::path::PathBuf>,
) -> (AppKind, Option<std::path::PathBuf>) {
    match command {
        Some(Command::Gol { path }) => (AppKind::Gol, path),
        Some(Command::Games { path, .. }) => (AppKind::Games, path),
        Some(Command::Multipane(_)) => {
            // Unreachable: the Multipane arm short-circuits in `main` before
            // resolve_app_target is called. Kept exhaustive for future
            // refactors that pipe everything through this resolver.
            (LabId::from(lab), fallback_path)
        }
        None => (LabId::from(lab), fallback_path),
    }
}

fn configure_app_state(
    state: &mut AppState,
    agent_selection: Option<AgentsArg>,
    requested_kind: AppKind,
    target_path: Option<&Path>,
) {
    state.agents = agents::init_agents(agent_selection.unwrap_or(AgentsArg::All));
    state.app_kind = requested_kind;
    state.visualizer.seed = stable_hash_bytes(state.editor_buffer().content_as_string().as_bytes());
    state.mode = Mode::Normal;

    migrate_legacy_notes(state);

    // Open the file tree when launching into a directory rather than a single file.
    if target_path.is_none_or(|p| p.is_dir()) {
        state.file_tree.root = state.workspace_root.clone();
        state.file_tree.open = true;
        state.focus = PaneId::Editor;
    }
}

fn migrate_legacy_notes(state: &mut AppState) {
    if let Some(snapshot_path) =
        export_legacy_notes_snapshot(&state.workspace_root, state.notes_buffer())
    {
        state.agents.pending_legacy_notes_alert = Some(format!(
            "Legacy Notes were preserved in {} and are available in Agent Ops > Scratchpad.",
            snapshot_path.display()
        ));
    }
}

fn init_gol_rules(state: &mut AppState) {
    let nit_core::RuleConfigLoad {
        rule: prefs,
        rules,
        workspace_rule,
        global_path,
        workspace_path,
        warnings,
    } = nit_core::load_rule_config(&state.workspace_root);

    let (catalog, mut diagnostics) = nit_core::load_rule_catalog(&rules.user);
    diagnostics.extend(warnings);
    for message in diagnostics {
        tracing::warn!("{message}");
    }

    // Workspace-level override takes priority over the global default.
    let chosen_key = prefs
        .workspace_override
        .then_some(workspace_rule)
        .flatten()
        .unwrap_or_else(|| prefs.default.clone());

    let resolved_rule = catalog.select(&chosen_key).unwrap_or_else(|err| {
        tracing::warn!("invalid GoL rule '{chosen_key}': {err}");
        nit_core::SelectedRule::default()
    });

    state.settings.gol.rule = prefs.clone();
    state.settings.gol.rules = rules;
    state.init_rules(
        catalog,
        resolved_rule,
        nit_core::RulePersistence {
            global_path,
            workspace_path,
            workspace_override: prefs.workspace_override,
        },
    );
}

fn run_multipane(
    args: MultipaneArgs,
    runtime_mode: CodexRuntimeMode,
    codex_config: CodexRunnerConfig,
    claude_config: ClaudeRunnerConfig,
) -> anyhow::Result<()> {
    let roster = agents::init_agents(AgentsArg::All);
    let known: Vec<String> = sorted_backend_ids(&roster);
    if !known.iter().any(|id| id == &args.backend) {
        report_unknown_backend(&args.backend, &known);
        std::process::exit(2);
    }

    let pane_count = usize::from(args.panes);
    let cwd = args
        .cwd
        .clone()
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)?;

    let (workspace_root, editor_buffer) = open_target_gol(Some(&cwd))?;
    let theme = Theme::load(find_theme().as_deref());
    let (log_sender, log_receiver) = mpsc::channel::<String>();
    init_tracing(log_sender, log_path_for_workspace(&workspace_root))?;
    install_panic_hook();
    let notes_buffer = load_notes(&workspace_root);

    let mut state = AppState::new(workspace_root, editor_buffer, notes_buffer);
    state.substrate = SubstrateState::load(&state.workspace_root);
    state.agents = roster;
    state.app_kind = AppKind::Gol;
    state.mode = Mode::Normal;
    state.focus = PaneId::Editor;

    multipane::setup::install(&mut state, &args.backend, pane_count, cwd)
        .map_err(|err| anyhow::anyhow!(err))?;

    run(
        state,
        theme,
        log_receiver,
        runtime_mode,
        codex_config,
        claude_config,
    )?;
    Ok(())
}

fn sorted_backend_ids(roster: &nit_core::AgentsState) -> Vec<String> {
    let mut ids: Vec<String> = roster.agents.iter().map(|lane| lane.id.clone()).collect();
    ids.sort();
    ids
}

fn report_unknown_backend(backend: &str, known: &[String]) {
    eprintln!("error: unknown --backend '{backend}'");
    if known.is_empty() {
        eprintln!("no backends available — install codex / claude / gemini and retry");
        return;
    }
    eprintln!("available backends:");
    for id in known {
        eprintln!("  {id}");
    }
}

#[cfg(test)]
#[path = "tests/main.rs"]
mod tests;
