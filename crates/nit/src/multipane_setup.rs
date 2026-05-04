use std::path::PathBuf;
use std::sync::mpsc;

use nit_core::{AgentsState, AppKind, AppState, Mode, PaneId, SubstrateState};
use nit_tui::claude_runner::ClaudeRunnerConfig;
use nit_tui::codex_runner::{CodexRunnerConfig, CodexRuntimeMode};
use nit_tui::multipane;
use nit_tui::swarm::effective_max_swarm_size;
use nit_tui::{run, Theme};

use crate::agents;
use crate::cli::{AgentsArg, MultipaneArgs};
use crate::logging::{init_tracing, install_panic_hook, log_path_for_workspace};
use crate::workspace::{find_theme, load_notes, open_target_gol};

pub(crate) fn run_multipane(
    args: MultipaneArgs,
    runtime_mode: CodexRuntimeMode,
    codex_config: CodexRunnerConfig,
    claude_config: ClaudeRunnerConfig,
) -> anyhow::Result<()> {
    let roster = agents::init_agents(AgentsArg::All);
    validate_backend(&roster, args.backend.as_deref());

    let pane_count = usize::from(args.panes);
    let cwd = args
        .cwd
        .clone()
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)?;

    let (state, theme, log_receiver) =
        bootstrap_state(args.backend.as_deref(), cwd, roster, pane_count)?;

    let (codex_config, claude_config) =
        scale_runner_concurrency(pane_count, codex_config, claude_config);

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

// Unknown-backend rejection must fire before any state construction so
// the operator gets a fast actionable error and the process doesn't half-
// initialize the workspace.
fn validate_backend(roster: &AgentsState, backend: Option<&str>) {
    let known: Vec<String> = sorted_backend_ids(roster);
    let unknown = backend.filter(|value| {
        !multipane::setup::is_backend_family(value) && !known.iter().any(|id| id == *value)
    });
    if let Some(value) = unknown {
        report_unknown_backend(value, &known);
        std::process::exit(2);
    }
}

fn bootstrap_state(
    backend: Option<&str>,
    cwd: PathBuf,
    roster: AgentsState,
    pane_count: usize,
) -> anyhow::Result<(AppState, Theme, mpsc::Receiver<String>)> {
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

    multipane::setup::install_filtered(&mut state, backend, pane_count, cwd)
        .map_err(|err| anyhow::anyhow!(err))?;

    Ok((state, theme, log_receiver))
}

// Without scaling, the runner thread's `while active.len() < max_parallel`
// gate (default 2) serializes panes despite per-agent state-side queueing
// being correct. We clamp to the FD ceiling already protecting swarm fan-out.
fn scale_runner_concurrency(
    pane_count: usize,
    codex_config: CodexRunnerConfig,
    claude_config: ClaudeRunnerConfig,
) -> (CodexRunnerConfig, ClaudeRunnerConfig) {
    let host_fd_ceiling = effective_max_swarm_size();
    let requested = pane_count.max(codex_config.max_parallel_turns);
    let scaled = requested.min(host_fd_ceiling);
    if scaled < pane_count {
        eprintln!(
            "nit multipane: clamped runner concurrency to {scaled} \
             (host FD ceiling), below requested {pane_count} panes — \
             raise with `ulimit -n 4096` and restart to lift it."
        );
    }
    let codex_scaled = CodexRunnerConfig {
        max_parallel_turns: scaled,
        ..codex_config
    };
    let claude_scaled = ClaudeRunnerConfig {
        max_parallel_turns: scaled,
        ..claude_config
    };
    (codex_scaled, claude_scaled)
}

fn sorted_backend_ids(roster: &AgentsState) -> Vec<String> {
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
