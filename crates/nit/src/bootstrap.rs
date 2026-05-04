use std::path::Path;

use nit_core::{AppKind, AppState, LabId, Mode, PaneId};
use nit_tui::claude_runner::ClaudeRunnerConfig;
use nit_tui::codex_runner::{CodexRunnerConfig, CodexRuntimeMode};
use nit_utils::hashing::stable_hash_bytes;

use crate::agents;
use crate::cli::{self, AgentsArg, Cli, Command};
use crate::workspace::export_legacy_notes_snapshot;

pub(crate) fn build_runner_configs(
    cli: &Cli,
) -> (CodexRuntimeMode, CodexRunnerConfig, ClaudeRunnerConfig) {
    let runtime_mode = CodexRuntimeMode::from(cli.codex_runtime);
    let max_turns = cli.codex_max_parallel_turns as usize;
    let codex = CodexRunnerConfig {
        sandbox: cli.codex_sandbox.map(|s| s.as_str().to_string()),
        approval_policy: Some(cli.codex_approval_policy.as_str().to_string()),
        max_parallel_turns: max_turns,
        mcp_backchannel_socket: None,
    };
    let claude = ClaudeRunnerConfig {
        max_parallel_turns: max_turns,
        permission_mode: None,
    };
    (runtime_mode, codex, claude)
}

pub(crate) fn resolve_app_target(
    command: Option<Command>,
    lab: cli::LabArg,
    fallback_path: Option<std::path::PathBuf>,
) -> (AppKind, Option<std::path::PathBuf>) {
    match command {
        Some(Command::Gol { path }) => (AppKind::Gol, path),
        Some(Command::Games { path, .. }) => (AppKind::Games, path),
        // Multipane short-circuits before this resolver runs.
        Some(Command::Multipane(_)) => (LabId::from(lab), fallback_path),
        None => (LabId::from(lab), fallback_path),
    }
}

pub(crate) fn configure_app_state(
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

pub(crate) fn init_gol_rules(state: &mut AppState) {
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
