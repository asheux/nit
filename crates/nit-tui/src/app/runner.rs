#![allow(clippy::too_many_arguments)]

use std::io::{self, Stdout};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use arboard::Clipboard;
use crossterm::event::{self, Event, KeyEventKind, KeyboardEnhancementFlags};
use nit_core::{actions::Action, AgentBusEvent, AppKind, AppState, Mode, PaneId};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::claude_runner::{ClaudeRunner, ClaudeRunnerConfig};
use crate::codex_runner::{CodexRunner, CodexRunnerConfig, CodexRuntimeMode};
use crate::file_tree;
use crate::file_tree_runner::{FileTreeEvent, FileTreeRunner};
use crate::file_watcher::FileWatcher;
use crate::games_petri_dish::GamesPetriDishRuntime;
use crate::petri_dish::PetriDishRuntime;
use crate::seed_runtime::SeedRuntime;
use crate::swarm::SwarmRuntime;
use crate::syntax::SyntaxRuntime;
use crate::system_stats::SystemStats;
use crate::theme::Theme;
use crate::vitals::VitalsState;
use crate::widgets::{protocol_picker, rule_picker};

use super::*;

pub(super) const TICK_RATE: Duration = Duration::from_millis(50);

/// Minimum interval between full-screen redraws (~60 fps default). The
/// runner gates `terminal.draw` on this so a high-volume agent-bus burst
/// can't repaint faster than the terminal's compositor can absorb.
/// Operator override: `NIT_TUI_FPS` (clamped to 15..=120).
pub(crate) const DEFAULT_FRAME_INTERVAL: Duration = Duration::from_millis(16);

pub(super) const JOB_TICK: Duration = Duration::from_millis(120);

pub(super) const BUSY_PULSE_INTERVAL: Duration = Duration::from_millis(550);

pub(super) const CHORD_TIMEOUT: Duration = Duration::from_millis(300);

pub(super) const INSPECTOR_JUMP_TIMEOUT: Duration = Duration::from_millis(1500);

pub(super) const INITIAL_SIZE_SETTLE: Duration = Duration::from_millis(80);

pub fn run(
    mut state: AppState,
    theme: Theme,
    log_rx: Receiver<String>,
    codex_runtime: CodexRuntimeMode,
    codex_config: CodexRunnerConfig,
    claude_config: ClaudeRunnerConfig,
) -> io::Result<()> {
    let (guard, mut stdout) = TerminalGuard::activate()?;
    install_terminal_panic_hook(guard.weak_state());
    if let Err(err) = guard.install_sigint_handler() {
        tracing::warn!("Failed to install Ctrl-C handler: {err}");
    }
    if guard.enable_mouse_capture(&mut stdout).is_err() {
        tracing::warn!("Mouse capture enable failed; continuing without it");
    }
    let keyboard_flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
    guard.push_keyboard_flags(&mut stdout, keyboard_flags);
    if guard.enable_bracketed_paste(&mut stdout).is_err() {
        tracing::warn!("Bracketed paste enable failed; continuing without it");
    }
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;
    guard.mark_cursor_hidden(true);

    if state.multipane.is_some() {
        let result = crate::multipane::run_loop(
            &mut terminal,
            &mut state,
            &theme,
            log_rx,
            codex_runtime,
            codex_config,
            claude_config,
        );
        terminal.show_cursor()?;
        guard.mark_cursor_hidden(false);
        guard.restore();
        return result;
    }

    let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
    let editor_id = state.active_editor_buffer_id;
    let notes_id = state.notes_buffer_id;
    let warmup_editor = state.editor_buffer().bytes_len() <= 200_000;
    syntax.prime_buffer(editor_id, state.editor_buffer(), warmup_editor);
    syntax.prime_buffer(notes_id, state.notes_buffer(), false);

    // Load git HEAD base for diff gutter on initial editor buffer.
    if let Some(path) = state.editor_buffer().path().cloned() {
        if let Some(base) = git_head_content(&path) {
            state.editor_buffer_mut().set_git_base(&base);
        }
    }

    let result = run_loop(
        &mut terminal,
        &mut state,
        &theme,
        &mut syntax,
        log_rx,
        codex_runtime,
        codex_config,
        claude_config,
    );

    terminal.show_cursor()?;
    guard.mark_cursor_hidden(false);
    guard.restore();
    let _ = save_notes_on_exit(&state);
    result
}

/// Resolve the redraw cap. Out-of-range values (or anything unparseable)
/// fall back to `DEFAULT_FRAME_INTERVAL` silently — there's no panic
/// path for a typo'd env var. Called once at run start so the env read
/// stays out of the hot loop.
pub(crate) fn frame_interval() -> Duration {
    match std::env::var("NIT_TUI_FPS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    {
        Some(fps) if (15..=120).contains(&fps) => Duration::from_millis(1000 / fps),
        _ => DEFAULT_FRAME_INTERVAL,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut AppState,
    theme: &Theme,
    syntax: &mut SyntaxRuntime,
    log_rx: Receiver<String>,
    codex_runtime: CodexRuntimeMode,
    codex_config: CodexRunnerConfig,
    claude_config: ClaudeRunnerConfig,
) -> io::Result<()> {
    let frame_interval = frame_interval();
    let mut last_tick = Instant::now();
    // Underflow-safe init that also lets the very first redraw fire
    // immediately (no startup blank): if the host clock is too young to
    // subtract `frame_interval`, fall back to `app_start`-equivalent
    // (which is "now", same effect — the gate sees `elapsed() == 0`
    // only on the very first iteration and the existing `needs_redraw =
    // true` initialisation forces the draw anyway).
    let mut last_render = Instant::now()
        .checked_sub(frame_interval)
        .unwrap_or_else(Instant::now);
    let mut last_job = Instant::now();
    let mut last_metabolism = Instant::now();
    let mut last_vitals_sample = Instant::now();
    let mut last_busy_pulse = Instant::now();
    let app_start = Instant::now();
    let mut last_resize_event: Option<(Duration, u16, u16)> = None;
    let mut needs_redraw = true;
    let mut input_state = InputState::new();
    let mut stashed_event: Option<Event> = None;
    let mut system_stats = SystemStats::new();
    let mut clipboard = Clipboard::new().ok();
    let mut file_tree_runner = FileTreeRunner::spawn();
    let mut file_watcher = FileWatcher::spawn();
    let genome_worker = crate::genome_worker::GenomeWorker::new();
    let mut workspace_scan = crate::workspace_scan::WorkspaceScanRuntime::new();
    // Parse .gitignore for directory exclusions (shared with file watcher and FILESCORES view).
    state.gitignored_dirs = crate::file_watcher::parse_gitignore_dirs(&state.workspace_root);
    // Load any previously-cached genome reports from disk so FILESCORES has
    // something to show on launch. The expensive workspace walk + per-file
    // eval is deferred — large projects (~2500 files) would otherwise pin
    // the CPU at boot and risk crashing low-power laptops. The operator
    // triggers the walk via the EVALUATE button in the gate monitor's
    // FILESCORES sub-view (`Action::WorkspaceScanStart`).
    workspace_scan.load_cache(state);
    // Dry walk: count code files + stale reports so the EVAL button can
    // show "stale/total" before the operator clicks. This walk is cheap
    // (filesystem traversal + mtime lookups, no tree-sitter or GoL) — the
    // CPU cost we were avoiding lives in the per-file genome eval, not in
    // the walk itself.
    let (stale, total) = crate::workspace_scan::WorkspaceScanRuntime::count_workspace(state);
    state.agents.workspace_scan_stale_files = stale;
    state.agents.workspace_scan_total_files = total;
    // Pre-seed `workspace_scan_clean` based on the dry walk so the button
    // renders correctly on first frame: if total > 0 and stale == 0, the
    // cache is already complete and the button should read "✓ ALL
    // EVALUATED" without the operator having to click first.
    state.agents.workspace_scan_clean = total > 0 && stale == 0;
    // Watch the entire workspace for agent-created/modified files.
    file_watcher.watch_workspace(state.workspace_root.clone());
    let mut last_watched_path = state.editor_buffer().path().cloned();
    if let Some(ref path) = last_watched_path {
        file_watcher.watch(path.clone());
    }
    let has_codex = state.agents.agents.iter().any(|lane| lane.is_codex());
    let codex_runtime = if has_codex {
        codex_runtime
    } else {
        CodexRuntimeMode::Exec
    };
    state.agents.codex_max_parallel_turns = codex_config.max_parallel_turns;
    // nit-mcp back-channel: spawn the UDS listener before Codex so the
    // socket is bound by the time Codex's first tool call arrives.
    // Unix-only in v1; Windows builds skip this and lose deliberate-emit MCP.
    #[cfg(unix)]
    let (mcp_backchannel, mcp_event_rx): (
        Option<crate::mcp_backchannel::McpBackchannel>,
        Option<Receiver<AgentBusEvent>>,
    ) = {
        let (tx, rx) = mpsc::channel();
        match crate::mcp_backchannel::McpBackchannel::spawn(tx) {
            Ok(bc) => {
                tracing::info!("nit-mcp back-channel listening at {}", bc.socket_path);
                (Some(bc), Some(rx))
            }
            Err(err) => {
                tracing::warn!("nit-mcp back-channel disabled: {err}");
                (None, None)
            }
        }
    };
    #[cfg(not(unix))]
    let mcp_event_rx: Option<Receiver<AgentBusEvent>> = None;
    let mcp_backchannel_socket: Option<String> = {
        #[cfg(unix)]
        {
            mcp_backchannel.as_ref().map(|bc| bc.socket_path.clone())
        }
        #[cfg(not(unix))]
        {
            None
        }
    };
    let mut codex_runner =
        CodexRunner::spawn(codex_runtime, codex_config, mcp_backchannel_socket.clone());
    state.agents.claude_max_parallel_turns = claude_config.max_parallel_turns;
    let mut claude_runner = ClaudeRunner::spawn(claude_config);
    // Keep the MCP listener alive for the lifetime of the run loop.
    #[cfg(unix)]
    let _mcp_backchannel_keepalive = mcp_backchannel;
    let mut swarm = SwarmRuntime::default();
    let mut shadow = crate::shadow::ShadowRuntime::default();
    // Idle-sleep guard: held while at least one agent turn is in flight so
    // macOS doesn't hibernate mid-swarm and SIGSTOP the runner subprocesses.
    // Released on Drop, so a panic / hard-exit can't leave caffeinate behind.
    let mut idle_sleep_guard = crate::power::IdleSleepGuard::default();
    let mut fuzzy_runtime = FuzzySearchRuntime::new(theme, state.settings.highlight.clone());
    let (mut seed_runtime, mut gol_petri, mut games_petri, mut games_config_preview) =
        spawn_app_runtimes(state);
    let mut vitals = VitalsState::default();
    let mut last_gol_generation = state.visualizer.generation;
    let mut last_gol_running = state.visualizer.running;
    let mut last_gol_paused = state.visualizer.paused;
    let mut last_games_status = state.games.status;
    let mut last_games_running = state.games.running;
    let mut last_games_paused = state.games.paused;
    let mut last_status_text = state.status.clone();
    let mut last_games_activity_epoch = games_petri
        .as_ref()
        .map(|petri| petri.activity_epoch())
        .unwrap_or(0);
    let mut last_agent_event_epoch = state.agents.event_epoch;
    let initial_size = settle_initial_terminal_size(terminal, INITIAL_SIZE_SETTLE)?;
    let mut last_screen_size = Some((initial_size.width, initial_size.height));
    tracing::info!(
        "init_size size=({},{}) settle_ms={}",
        initial_size.width,
        initial_size.height,
        INITIAL_SIZE_SETTLE.as_millis()
    );
    tracing::info!(
        "SECURITY: no plugins; nit makes no network calls; external commands only run via explicit agent integrations (e.g. codex)"
    );
    loop {
        fuzzy_runtime.tick_open(state);
        if let Some(runtime) = games_config_preview.as_mut() {
            runtime.request_for_editor(state);
            runtime.poll(state);
        }
        if let Some(deferred) = input_state.take_deferred() {
            if let Some(action) = map_key_to_action(deferred, state, &mut input_state) {
                prepare_clipboard_paste(state, &mut clipboard, &action);
                let action_copy = action.clone();
                let outcome = apply_action_with_syntax(state, syntax, action);
                if matches!(action_copy, Action::ToggleSyntax) {
                    fuzzy_runtime.update_syntax_config(state.settings.highlight.clone());
                }
                handle_clipboard_copy(state, &mut clipboard, &action_copy);
                handle_selection_autocopy(state, &mut clipboard, &mut input_state);
                if outcome.should_exit {
                    break;
                }
                needs_redraw = needs_redraw || outcome.state_changed;
            }
            continue;
        }

        // Poll input with tick fallback (check stashed events from scroll coalescing first).
        // When a redraw is already pending but the frame cap deferred it, shrink the
        // wait to the remaining frame budget so the loop wakes at the next frame
        // boundary instead of busy-spinning. The 1 ms floor avoids degenerate
        // sub-millisecond timeouts that some terminals reduce to a busy poll.
        let timeout = if needs_redraw {
            frame_interval
                .saturating_sub(last_render.elapsed())
                .max(Duration::from_millis(1))
        } else {
            TICK_RATE
        };
        let mut handled_input = false;
        let next_event = match stashed_event.take() {
            Some(e) => Some(e),
            None => {
                if event::poll(timeout)? {
                    Some(event::read()?)
                } else {
                    None
                }
            }
        };
        if let Some(next_event) = next_event {
            match next_event {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }
                    handled_input = true;
                    if is_job_pause_key(&key) {
                        if state.app_kind == AppKind::Games
                            && state.command_line.is_none()
                            && state.prompt.is_none()
                        {
                            if let Some(petri) = games_petri.as_mut() {
                                if petri.is_visible() && petri.handle_key(&key, state) {
                                    needs_redraw = true;
                                    continue;
                                }
                            }
                        }
                        let outcome =
                            apply_action_with_syntax(state, syntax, Action::ToggleJobPause);
                        if outcome.should_exit {
                            break;
                        }
                        needs_redraw = needs_redraw || outcome.state_changed;
                        continue;
                    }
                    if state.app_kind == AppKind::Games
                        && state.command_line.is_none()
                        && state.prompt.is_none()
                        && !state.show_help
                        && !games_modal_popup_open(state)
                        && is_games_petri_control_key(&key)
                    {
                        if let Some(petri) = games_petri.as_mut() {
                            if petri.is_visible() && petri.handle_key(&key, state) {
                                needs_redraw = true;
                                continue;
                            }
                        }
                    }
                    if !state.fuzzy_search.open
                        && state.command_line.is_none()
                        && state.prompt.is_none()
                        && matches!(
                            key,
                            KeyEvent {
                                code: KeyCode::Char('p')
                                    | KeyCode::Char('P')
                                    | KeyCode::Char('f')
                                    | KeyCode::Char('F'),
                                modifiers,
                                ..
                            } if modifiers.contains(KeyModifiers::CONTROL)
                        )
                    {
                        if let Some(action) = map_key_to_action(key, state, &mut input_state) {
                            prepare_clipboard_paste(state, &mut clipboard, &action);
                            let action_copy = action.clone();
                            let outcome = apply_action_with_syntax(state, syntax, action);
                            if matches!(action_copy, Action::ToggleSyntax) {
                                fuzzy_runtime
                                    .update_syntax_config(state.settings.highlight.clone());
                            }
                            handle_clipboard_copy(state, &mut clipboard, &action_copy);
                            handle_selection_autocopy(state, &mut clipboard, &mut input_state);
                            if outcome.should_exit {
                                break;
                            }
                            needs_redraw = needs_redraw || outcome.state_changed;
                        }
                        continue;
                    }
                    if state.fuzzy_search.open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_fuzzy_search_key(&key, state, syntax, &mut fuzzy_runtime, screen)
                        {
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.command_line.is_some() || state.prompt.is_some() {
                        if let Some(action) = map_key_to_action(key, state, &mut input_state) {
                            prepare_clipboard_paste(state, &mut clipboard, &action);
                            let action_copy = action.clone();
                            let outcome = apply_action_with_syntax(state, syntax, action);
                            if matches!(action_copy, Action::ToggleSyntax) {
                                fuzzy_runtime
                                    .update_syntax_config(state.settings.highlight.clone());
                            }
                            handle_clipboard_copy(state, &mut clipboard, &action_copy);
                            handle_selection_autocopy(state, &mut clipboard, &mut input_state);
                            if outcome.should_exit {
                                break;
                            }
                            needs_redraw = needs_redraw || outcome.state_changed;
                        }
                        continue;
                    }
                    if state.rule_picker.open && rule_picker::handle_key(&key, state) {
                        needs_redraw = true;
                        continue;
                    }
                    if state.protocol_picker.open && protocol_picker::handle_key(&key, state) {
                        needs_redraw = true;
                        continue;
                    }
                    if state.agents.artifacts_popup_open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_artifacts_popup_key(
                            &key,
                            state,
                            &mut swarm,
                            &mut shadow,
                            &mut vitals,
                            Some(&codex_runner),
                            Some(&claude_runner),
                            &mut clipboard,
                            screen,
                            theme,
                        ) {
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.agents.global_archive_open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_global_archive_key(&key, state, screen, theme) {
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.show_help {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_help_popup_key(&key, state, screen, theme) {
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.show_substrate_overlay && handle_substrate_overlay_key(&key, state) {
                        needs_redraw = true;
                        continue;
                    }
                    if state.games.analysis.open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_analysis_popup_key(&key, state, screen, theme) {
                            clamp_modal_scroll_offsets(state, screen, theme);
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.app_kind == AppKind::Games && state.games.run_browser.open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_run_browser_key(&key, state, screen) {
                            clamp_modal_scroll_offsets(state, screen, theme);
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.app_kind == AppKind::Games && state.games.replay.open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_replay_popup_key(&key, state, screen, theme) {
                            clamp_modal_scroll_offsets(state, screen, theme);
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.app_kind == AppKind::Games && state.games.strategy_inspect.open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_strategy_popup_key(&key, state, screen) {
                            clamp_modal_scroll_offsets(state, screen, theme);
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.app_kind == AppKind::Games && state.games.tm_sim.open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_tm_sim_popup_key(&key, state, screen, theme) {
                            clamp_modal_scroll_offsets(state, screen, theme);
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.app_kind == AppKind::Games && state.games.ca_sim.open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_ca_sim_popup_key(&key, state, screen, theme) {
                            clamp_modal_scroll_offsets(state, screen, theme);
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.app_kind == AppKind::Games && state.games.match_history.open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_match_history_popup_key(&key, state, screen) {
                            clamp_modal_scroll_offsets(state, screen, theme);
                            needs_redraw = true;
                            continue;
                        }
                    }
                    let petri_visible = match state.app_kind {
                        AppKind::Gol => gol_petri.as_ref().map(|p| p.is_visible()).unwrap_or(false),
                        AppKind::Games => games_petri
                            .as_ref()
                            .map(|p| p.is_visible())
                            .unwrap_or(false),
                    };
                    if petri_visible && state.command_line.is_none() && state.prompt.is_none() {
                        let screen = terminal.size().unwrap_or_default();
                        let handled = match state.app_kind {
                            AppKind::Gol => {
                                if let (Some(petri), Some(seed_runtime)) =
                                    (gol_petri.as_mut(), seed_runtime.as_mut())
                                {
                                    petri.handle_key(&key, state, seed_runtime, screen)
                                } else {
                                    false
                                }
                            }
                            AppKind::Games => games_petri
                                .as_mut()
                                .map(|petri| petri.handle_key(&key, state))
                                .unwrap_or(false),
                        };
                        if handled {
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.file_tree.open && state.focus == PaneId::Editor {
                        let screen = terminal.size().unwrap_or_default();
                        let layout = layout::split(screen);
                        if handle_file_tree_key(&key, state, syntax, layout.editor) {
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if handle_agent_station_key_with_clipboard(
                        key,
                        state,
                        &mut vitals,
                        Some(&codex_runner),
                        Some(&claude_runner),
                        &mut swarm,
                        &mut shadow,
                        &mut clipboard,
                    ) {
                        needs_redraw = true;
                        continue;
                    }
                    if handle_editor_buffer_shortcuts(key, state, syntax, &mut clipboard) {
                        needs_redraw = true;
                        continue;
                    }
                    if let Some(action) = map_key_to_action(key, state, &mut input_state) {
                        prepare_clipboard_paste(state, &mut clipboard, &action);
                        let action_copy = action.clone();
                        let outcome = apply_action_with_syntax(state, syntax, action);
                        if matches!(action_copy, Action::ToggleSyntax) {
                            fuzzy_runtime.update_syntax_config(state.settings.highlight.clone());
                        }
                        handle_clipboard_copy(state, &mut clipboard, &action_copy);
                        handle_selection_autocopy(state, &mut clipboard, &mut input_state);
                        if outcome.should_exit {
                            break;
                        }
                        needs_redraw = needs_redraw || outcome.state_changed;
                    }
                }
                Event::Paste(text) => {
                    handled_input = true;
                    if handle_paste_event(&text, state, syntax, &mut fuzzy_runtime, &mut vitals) {
                        needs_redraw = true;
                    }
                }
                Event::Mouse(mouse) => {
                    handled_input = true;
                    let screen = terminal.size().unwrap_or_default();
                    let is_scroll = matches!(
                        mouse.kind,
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                    );
                    // Coalesce drag events the same way scroll events get
                    // coalesced below: each drag fires the popup-body /
                    // chat-thread mapper which calls `build_lines`
                    // (markdown rendering + syntax highlighting), so a
                    // burst of 30+ drag events per gesture compounds into
                    // visible scroll lag. Replace the to-handle event
                    // with the LAST drag in the burst — auto-scroll
                    // overshoot already scales with mouse-distance-past-
                    // edge so dropping intermediate drags doesn't hurt
                    // scroll throughput.
                    let mut to_handle = mouse;
                    if matches!(to_handle.kind, MouseEventKind::Drag(_)) {
                        while event::poll(Duration::ZERO)? {
                            match event::read()? {
                                Event::Mouse(m) if matches!(m.kind, MouseEventKind::Drag(_)) => {
                                    to_handle = m;
                                }
                                other => {
                                    stashed_event = Some(other);
                                    break;
                                }
                            }
                        }
                    }
                    let mut mouse_changed = handle_mouse_event_with_swarm(
                        &swarm,
                        to_handle,
                        screen,
                        state,
                        &mut fuzzy_runtime,
                        &mut input_state,
                        &mut clipboard,
                        theme,
                    );
                    // Coalesce pending scroll events to prevent phantom-offset
                    // lag from trackpad momentum scrolling. Without this, each
                    // queued scroll event triggers a full render cycle, so
                    // reversing direction feels delayed.
                    if is_scroll {
                        while event::poll(Duration::ZERO)? {
                            match event::read()? {
                                Event::Mouse(m)
                                    if matches!(
                                        m.kind,
                                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                                    ) =>
                                {
                                    mouse_changed |= handle_mouse_event_with_swarm(
                                        &swarm,
                                        m,
                                        screen,
                                        state,
                                        &mut fuzzy_runtime,
                                        &mut input_state,
                                        &mut clipboard,
                                        theme,
                                    );
                                }
                                other => {
                                    stashed_event = Some(other);
                                    break;
                                }
                            }
                        }
                    }
                    if mouse_changed {
                        clamp_modal_scroll_offsets(state, screen, theme);
                        needs_redraw = true;
                    }
                }
                Event::Resize(w, h) => {
                    let ts = app_start.elapsed();
                    tracing::info!(
                        "resize_event ts_ms={} w={} h={} prev_size={:?}",
                        ts.as_millis(),
                        w,
                        h,
                        last_screen_size
                    );
                    last_resize_event = Some((ts, w, h));
                    needs_redraw = true;
                }
                _ => {}
            }
        }

        if !handled_input && matches!(state.focus, PaneId::Editor) && state.mode == Mode::Insert {
            if let Some(action) = input_state.flush_insert_timeout() {
                prepare_clipboard_paste(state, &mut clipboard, &action);
                let action_copy = action.clone();
                let outcome = apply_action_with_syntax(state, syntax, action);
                handle_clipboard_copy(state, &mut clipboard, &action_copy);
                handle_selection_autocopy(state, &mut clipboard, &mut input_state);
                if outcome.should_exit {
                    break;
                }
                needs_redraw = needs_redraw || outcome.state_changed;
            }
        }

        // job tick
        if last_job.elapsed() >= JOB_TICK {
            state.tick_job(0.03);
            tick_agent_turn_liveness(state);
            // Sync the idle-sleep guard with the current in-flight count.
            // 120ms cadence is plenty fast — the worst case is "turn started
            // 119ms ago and the system has already begun the idle-sleep
            // countdown", which the assertion still cancels in time. Costs
            // nothing when the count and setting haven't changed.
            idle_sleep_guard.sync(
                state.settings.power.prevent_idle_sleep_during_turns,
                state.agents.active_turns.len(),
            );
            last_job = Instant::now();
            needs_redraw = true;
        }

        // metabolic tick — wall-clock substrate sweep, no gen advance.
        // Phase 9: interval breathes with mood (Exploration 10s, Consolidation
        // 5s, Defensive 3s).
        if last_metabolism.elapsed()
            >= nit_core::metabolism::tick_interval_for(state.substrate.mood)
        {
            let outcome = nit_core::metabolism::tick(state);
            if !outcome.is_noop() {
                needs_redraw = true;
            }
            last_metabolism = Instant::now();
        }

        // drain logs
        while let Ok(line) = log_rx.try_recv() {
            let now = Instant::now();
            record_log_line_vitals(&mut vitals, now, &line);
            state.receive_log(line.clone());
            append_log_to_agent_diagnostics(state, &line);
            needs_redraw = true;
        }

        // file tree runner events
        while let Ok(event) = file_tree_runner.events.try_recv() {
            let preserve = file_tree::selected_path(state);
            match event {
                FileTreeEvent::DirListed { dir, entries } => {
                    state.file_tree.cache.insert(dir.clone(), entries);
                    state.file_tree.loading_dirs.remove(&dir);
                    file_tree::rebuild_view(state, Some(preserve));
                    needs_redraw = true;
                }
                FileTreeEvent::Error { dir, message } => {
                    state.file_tree.loading_dirs.remove(&dir);
                    vitals.record_diag_event(Instant::now(), DiagSeverity::Error);
                    state.status = Some(format!("NITTree: {message}"));
                    file_tree::rebuild_view(state, Some(preserve));
                    needs_redraw = true;
                }
            }
        }

        // fuzzy search runner events
        while let Ok(event) = fuzzy_runtime.indexer.events.try_recv() {
            handle_fuzzy_index_event(state, &mut fuzzy_runtime, event);
            needs_redraw = needs_redraw || state.fuzzy_search.open;
        }
        while let Ok(event) = fuzzy_runtime.fuzzy.events.try_recv() {
            handle_fuzzy_file_event(state, &mut fuzzy_runtime, event);
            needs_redraw = needs_redraw || state.fuzzy_search.open;
        }
        while let Ok(event) = fuzzy_runtime.content.events.try_recv() {
            handle_fuzzy_content_event(state, &mut fuzzy_runtime, event);
            needs_redraw = needs_redraw || state.fuzzy_search.open;
        }
        while let Ok(event) = fuzzy_runtime.preview.events.try_recv() {
            handle_fuzzy_preview_event(state, &mut fuzzy_runtime, event);
            needs_redraw = needs_redraw || state.fuzzy_search.open;
        }

        // nit-mcp back-channel events (*Request variants from subprocess
        // Codex agents).  Mint-on-apply ids live on the main thread, so the
        // listener is just a translator from socket bytes to AgentBusEvent.
        if let Some(rx) = mcp_event_rx.as_ref() {
            while let Ok(event) = rx.try_recv() {
                record_agent_bus_vitals(&mut vitals, &event);
                event.apply(state);
                needs_redraw = true;
            }
        }

        // codex runner events: collect this tick's batch first so the
        // heartbeat coalescer can drop dominated heartbeats before the
        // per-event side-effect pipeline runs. Other variants are
        // preserved verbatim (see crate::app::event_coalesce docs).
        let mut codex_batch: Vec<AgentBusEvent> = codex_runner.events.try_iter().collect();
        if !codex_batch.is_empty() {
            super::event_coalesce::coalesce_heartbeats(&mut codex_batch);
            for event in codex_batch {
                let outcome = super::event_drain::drain_codex_event(
                    state,
                    &mut vitals,
                    &codex_runner,
                    &claude_runner,
                    &mut swarm,
                    &mut shadow,
                    Some(&genome_worker),
                    event,
                );
                if outcome.redraw {
                    needs_redraw = true;
                }
            }
        }

        // claude runner events
        let mut claude_batch: Vec<AgentBusEvent> = claude_runner.events.try_iter().collect();
        if !claude_batch.is_empty() {
            super::event_coalesce::coalesce_heartbeats(&mut claude_batch);
            for event in claude_batch {
                let outcome = super::event_drain::drain_claude_event(
                    state,
                    &mut vitals,
                    &codex_runner,
                    &claude_runner,
                    &mut swarm,
                    &mut shadow,
                    Some(&genome_worker),
                    event,
                );
                if outcome.redraw {
                    needs_redraw = true;
                }
            }
        }

        if file_tree_tick(state, &file_tree_runner) {
            needs_redraw = true;
        }

        // syntax ticks
        let editor_id = state.active_editor_buffer_id;
        let notes_id = state.notes_buffer_id;
        syntax.tick(editor_id, state.editor_buffer());
        syntax.tick(notes_id, state.notes_buffer());
        syntax.poll_results(editor_id, state.editor_buffer().version());
        syntax.poll_results(notes_id, state.notes_buffer().version());

        let tick_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Some(seed_runtime) = seed_runtime.as_mut() {
                seed_runtime.tick(state);
            }
            match state.app_kind {
                AppKind::Gol => {
                    if let Some(petri) = gol_petri.as_mut() {
                        petri.tick(state);
                    }
                }
                AppKind::Games => {
                    if let Some(petri) = games_petri.as_mut() {
                        petri.tick(state);
                    }
                }
            }
        }));
        if let Err(err) = tick_result {
            tracing::error!("Runtime panic: {:?}", err);
            let now = Instant::now();
            vitals.record_diag_event(now, DiagSeverity::Error);
            vitals.mark_fatal(now);
            match state.app_kind {
                AppKind::Gol => {
                    state.visualizer.paused = true;
                    state.visualizer.paused_by_attractor = false;
                    state.status = Some("Petri dish paused (error)".into());
                }
                AppKind::Games => {
                    state.games.paused = true;
                    state.games.status = nit_core::GamesStatus::Error;
                    state.status = Some("Games tournament paused (error)".into());
                }
            }
        }
        let now = Instant::now();
        if state.status != last_status_text {
            if let Some(status) = state.status.as_deref() {
                record_log_line_vitals(&mut vitals, now, status);
                if status_looks_busy(status) {
                    vitals.record_job_event(now);
                }
            }
            last_status_text = state.status.clone();
        }
        match state.app_kind {
            AppKind::Gol => {
                if state.visualizer.generation != last_gol_generation {
                    last_gol_generation = state.visualizer.generation;
                    vitals.record_job_event(now);
                }
                if state.visualizer.running != last_gol_running
                    || state.visualizer.paused != last_gol_paused
                {
                    last_gol_running = state.visualizer.running;
                    last_gol_paused = state.visualizer.paused;
                    vitals.record_job_event(now);
                }
            }
            AppKind::Games => {
                if let Some(petri) = games_petri.as_ref() {
                    let epoch = petri.activity_epoch();
                    if epoch != last_games_activity_epoch {
                        let pulses = epoch.saturating_sub(last_games_activity_epoch).min(8);
                        for _ in 0..pulses {
                            vitals.record_job_event(now);
                        }
                        last_games_activity_epoch = epoch;
                    }
                }
                if state.games.status != last_games_status
                    || state.games.running != last_games_running
                    || state.games.paused != last_games_paused
                {
                    last_games_status = state.games.status;
                    last_games_running = state.games.running;
                    last_games_paused = state.games.paused;
                    vitals.record_job_event(now);
                }
            }
        }
        let busy = is_background_work_active(state);
        if busy {
            if !is_lab_job_running(state)
                && now.saturating_duration_since(last_busy_pulse) >= BUSY_PULSE_INTERVAL
            {
                vitals.record_job_event(now);
                last_busy_pulse = now;
            }
        } else {
            last_busy_pulse = now;
        }
        if let Some(message) = state.agents.pending_legacy_notes_alert.take() {
            state.agents.alerts.push(AgentAlert {
                severity: AgentAlertSeverity::Warn,
                source: "migration".into(),
                message: message.clone(),
                at: timestamp_label(state),
            });
            state.agents.diag_events.push(AgentDiagnosticEvent {
                severity: AgentAlertSeverity::Warn,
                source: "migration".into(),
                message,
                at: timestamp_label(state),
            });
            state.agents.note_event();
            needs_redraw = true;
        }
        if state.agents.event_epoch != last_agent_event_epoch {
            let now = Instant::now();
            let pulses = state
                .agents
                .event_epoch
                .wrapping_sub(last_agent_event_epoch)
                .min(8);
            for _ in 0..pulses {
                vitals.record_agent_event(now);
            }
            last_agent_event_epoch = state.agents.event_epoch;
        }
        if flush_agent_run_provenance(state, &swarm).is_err() {
            let now = Instant::now();
            vitals.record_diag_event(now, DiagSeverity::Warn);
        }

        // redraw — gated on the per-frame minimum interval so high-volume
        // bus bursts can't repaint faster than the terminal compositor.
        // `needs_redraw` is left true on a deferred redraw so the next
        // iteration wakes at the frame boundary (see input-poll timeout).
        if (needs_redraw || last_tick.elapsed() >= TICK_RATE)
            && last_render.elapsed() >= frame_interval
        {
            if let Ok(screen) = terminal.size() {
                let size = (screen.width, screen.height);
                if Some(size) != last_screen_size {
                    let ts = app_start.elapsed();
                    let last_resize = last_resize_event
                        .as_ref()
                        .map(|(t, w, h)| (t.as_millis(), *w, *h));
                    tracing::info!(
                        "draw_size ts_ms={} size={:?} prev={:?} last_resize_event={:?} trigger_handled_input={}",
                        ts.as_millis(),
                        size,
                        last_screen_size,
                        last_resize,
                        handled_input
                    );
                    last_screen_size = Some(size);
                }
            }
            system_stats.refresh_if_needed();
            let now = Instant::now();
            let dt = now.saturating_duration_since(last_vitals_sample);
            last_vitals_sample = now;
            let vitals_snapshot = vitals.tick(
                now,
                dt,
                is_lab_job_running(state),
                current_agent_state(state),
            );
            // Update file watcher when the active editor file changes.
            let current_path = state.editor_buffer().path().cloned();
            if current_path != last_watched_path {
                if let Some(ref old) = last_watched_path {
                    file_watcher.send(crate::file_watcher::FileWatchCommand::Unwatch(old.clone()));
                }
                if let Some(ref new) = current_path {
                    file_watcher.watch(new.clone());
                }
                last_watched_path = current_path;
            }

            // Drain file watcher notifications — reload any open buffers
            // that changed on disk and route every event through the
            // workspace-scan runtime for genome cache invalidation.
            drain_file_watcher(state, syntax, &file_watcher, &mut workspace_scan);
            drain_genome_results(
                state,
                &genome_worker,
                &mut workspace_scan,
                &mut vitals,
                &codex_runner,
                &claude_runner,
            );
            // EVALUATE click → walk + queue stale files. `rescan` no-ops
            // while a scan is in flight, so a stuck flag can't double-queue.
            if state.agents.workspace_scan_requested {
                state.agents.workspace_scan_requested = false;
                workspace_scan.rescan(state);
                let (_done, queued) = workspace_scan.progress();
                // Refresh dry-walk counts (total may have drifted from external
                // edits; stale = whatever rescan just queued).
                let (_stale, total) =
                    crate::workspace_scan::WorkspaceScanRuntime::count_workspace(state);
                state.agents.workspace_scan_total_files = total;
                state.agents.workspace_scan_stale_files = queued;
                if queued == 0 {
                    // Cache fully fresh — surface "up to date" instead of
                    // leaving a misleading "evaluating…" status on screen.
                    state.status =
                        Some("Genome cache up to date — no files need re-evaluation".into());
                    state.agents.workspace_scan_clean = true;
                } else {
                    state.status = Some(format!("Evaluating genome for {queued} files…"));
                    state.agents.workspace_scan_clean = false;
                }
            }
            // Non-blocking background scan; cap adapts to available_parallelism().
            workspace_scan.drive(&genome_worker);
            // Surface progress to the agent-console breather; None when idle so
            // the indicator auto-hides. Detect scanning→idle to flip clean.
            let was_scanning = state.agents.workspace_scan_progress.is_some();
            let is_scanning_now = workspace_scan.is_scanning();
            state.agents.workspace_scan_progress = if is_scanning_now {
                Some(workspace_scan.progress())
            } else {
                None
            };
            if was_scanning && !is_scanning_now {
                // Scan just drained — every queued file has a fresh report
                // now, so stale drops to 0 and the cache is clean.
                state.agents.workspace_scan_clean = true;
                state.agents.workspace_scan_stale_files = 0;
            }

            // Dispatch save-triggered genome evaluation to background worker.
            // Non-code files (markdown, config, plaintext) skip evaluation —
            // the metric is only meaningful for code, and computing it for
            // a README just to throw the report away is waste.
            if let Some(path) = state.genome_save_eval_pending.take() {
                if crate::workspace_scan::is_code_file(&path) {
                    let text = state.editor_buffer().content_as_string();
                    genome_worker.evaluate_save(path, text);
                }
            }

            // Auto-compute genome report for the active editor buffer if missing.
            maybe_compute_genome_report(state, &genome_worker);

            // Poll background genome gate evaluations — dispatches the
            // verifier agent once results arrive (never blocks).
            for mut dispatch in swarm.poll_genome_gates(state) {
                augment_dispatch_prompt_with_landscape(state, &swarm, &mut dispatch);
                apply_swarm_task_role(state, &dispatch);
                dispatch_agent_prompt(
                    state,
                    &mut vitals,
                    Some(&codex_runner),
                    Some(&claude_runner),
                    dispatch.agent_id,
                    Some(dispatch.mission_id),
                    dispatch.prompt,
                );
            }

            // Poll background genome review prompt builds — dispatches the
            // genome reviewer agent once the per-file genome reports finish
            // computing (never blocks the main loop on the GoL sim).
            for mut dispatch in swarm.poll_genome_reviews(state) {
                augment_dispatch_prompt_with_landscape(state, &swarm, &mut dispatch);
                apply_swarm_task_role(state, &dispatch);
                dispatch_agent_prompt(
                    state,
                    &mut vitals,
                    Some(&codex_runner),
                    Some(&claude_runner),
                    dispatch.agent_id,
                    Some(dispatch.mission_id),
                    dispatch.prompt,
                );
            }

            draw(
                terminal,
                state,
                &swarm,
                &workspace_scan,
                theme,
                syntax,
                &system_stats,
                &mut seed_runtime,
                &mut gol_petri,
                &mut games_petri,
                fuzzy_runtime.preview_model.as_ref(),
                &mut fuzzy_runtime.preview_scroll_delta,
                &vitals_snapshot,
            )?;
            needs_redraw = false;
            last_tick = Instant::now();
            last_render = Instant::now();
        }
    }
    file_tree_runner.shutdown();
    file_watcher.shutdown();
    codex_runner.shutdown();
    claude_runner.shutdown();
    fuzzy_runtime.shutdown();
    if let Some(runtime) = games_config_preview.as_mut() {
        runtime.shutdown();
    }
    Ok(())
}

pub(super) fn settle_initial_terminal_size(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    max_wait: Duration,
) -> io::Result<Rect> {
    let mut size = terminal.size()?;
    let start = Instant::now();
    let mut probes = 0;
    while start.elapsed() < max_wait {
        thread::sleep(Duration::from_millis(10));
        let current = terminal.size()?;
        if current == size {
            break;
        }
        probes += 1;
        tracing::info!(
            "init_size_probe ts_ms={} size=({},{}) prev=({},{})",
            start.elapsed().as_millis(),
            current.width,
            current.height,
            size.width,
            size.height
        );
        size = current;
    }
    if probes > 0 {
        tracing::info!(
            "init_size_settle ts_ms={} size=({},{}) probes={}",
            start.elapsed().as_millis(),
            size.width,
            size.height,
            probes
        );
    }
    terminal.resize(size)?;
    Ok(size)
}

fn spawn_app_runtimes(
    state: &AppState,
) -> (
    Option<SeedRuntime>,
    Option<PetriDishRuntime>,
    Option<GamesPetriDishRuntime>,
    Option<GamesConfigPreviewRuntime>,
) {
    match state.app_kind {
        AppKind::Gol => (
            Some(SeedRuntime::new(state)),
            Some(PetriDishRuntime::new(state)),
            None,
            None,
        ),
        AppKind::Games => (
            None,
            None,
            Some(GamesPetriDishRuntime::new(state)),
            Some(GamesConfigPreviewRuntime::spawn()),
        ),
    }
}
