use std::sync::mpsc;

use nit_core::{AppKind, AppState, Mode, PaneId, SubstrateState};
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
    let known: Vec<String> = sorted_backend_ids(&roster);
    let unknown = args.backend.as_deref().filter(|value| {
        !multipane::setup::is_backend_family(value) && !known.iter().any(|id| id == *value)
    });
    if let Some(value) = unknown {
        report_unknown_backend(value, &known);
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

    multipane::setup::install_filtered(&mut state, args.backend.as_deref(), pane_count, cwd)
        .map_err(|err| anyhow::anyhow!(err))?;

    // Scale runner concurrency by pane count, clamped to the FD ceiling
    // that already protects swarm fan-out. Without this, the runner
    // thread's `while active.len() < max_parallel` gate (default 2)
    // serializes panes despite per-agent state-side queueing being
    // correct.
    let parallel = pane_count
        .max(codex_config.max_parallel_turns)
        .min(effective_max_swarm_size());
    if parallel < pane_count {
        eprintln!(
            "nit multipane: clamped runner concurrency to {parallel} \
             (host FD ceiling), below requested {pane_count} panes — \
             raise with `ulimit -n 4096` and restart to lift it."
        );
    }
    let codex_config = CodexRunnerConfig {
        max_parallel_turns: parallel,
        ..codex_config
    };
    let claude_config = ClaudeRunnerConfig {
        max_parallel_turns: parallel,
        ..claude_config
    };

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
