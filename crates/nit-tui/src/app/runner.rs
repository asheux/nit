#![allow(unused_imports)]
#![allow(clippy::too_many_arguments)]
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::{
    mpsc::{self, Receiver, Sender},
    Arc, Mutex, Weak,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::swarm::{
    chat_clone_base_id, normalize_role_label, GateReport, GateReportGate, SwarmArtifactFocus,
    SwarmRuntime,
};
use crate::{
    claude_runner::{ClaudeRunner, ClaudeRunnerConfig},
    codex_runner::{CodexCommand, CodexRunner, CodexRunnerConfig, CodexRuntimeMode},
    file_tree,
    file_tree_runner::{FileTreeCommand, FileTreeEvent, FileTreeRunner},
    file_watcher::FileWatcher,
    fuzzy_preview_runner::{PreviewEvent, PreviewModel, PreviewRunner},
    fuzzy_search_runner::{
        ContentEvent, ContentSearchRunner, FileIndexRunner, FuzzyCommand, FuzzyEvent,
        FuzzyMatcherRunner, IndexEvent,
    },
    games_petri_dish::GamesPetriDishRuntime,
    layout,
    petri_dish::PetriDishRuntime,
    seed_runtime::SeedRuntime,
    syntax::SyntaxRuntime,
    system_stats::SystemStats,
    theme::Theme,
    vitals::{AgentVitalsState, DiagSeverity, LabVitalsSnapshot, VitalsState},
    widgets::{
        agent_console_view, agent_ops_view, artifacts_history_popup, artifacts_popup, bottom_bar,
        editor_view, file_tree_view, fuzzy_search_popup, games_analysis_popup, games_ca_sim_popup,
        games_match_history_popup, games_replay_popup, games_run_browser_popup,
        games_strategy_popup, games_tm_sim_popup, games_visualizer_view, gate_monitor_view,
        help_overlay, protocol_picker, rule_picker, substrate_overlay, top_bar, visualizer_view,
    },
};
use arboard::Clipboard;
use crossterm::{
    cursor::{SetCursorStyle, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseEvent,
        MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ctrlc::Error as CtrlcError;
use nit_core::{
    actions::Action, apply_action, io as core_io, AgentAlert, AgentAlertSeverity, AgentBusEvent,
    AgentChannel, AgentDiagnosticEvent, AgentMessage, AgentOpsTab, AgentStatus, AppKind, AppState,
    McpConnectionState, MissionPhase, MissionRecord, Mode, PaneId, PatchProposal, PatchStatus,
    Prompt, SavedRunHistoryFilter, SearchMode, UiSelection, UiSelectionPane, YankKind,
    CONSOLE_SCROLL_BOTTOM,
};
use nit_games::config::GamesConfig;
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::Style,
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Terminal,
};

use super::*;

pub(super) const TICK_RATE: Duration = Duration::from_millis(50);

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
    let mut last_tick = Instant::now();
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
    // Parse .gitignore for directory exclusions (shared with file watcher and FILESCORES view).
    state.gitignored_dirs = crate::file_watcher::parse_gitignore_dirs(&state.workspace_root);
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
    let mut fuzzy_runtime = FuzzySearchRuntime::new(theme, state.settings.highlight.clone());
    let mut seed_runtime = if state.app_kind == AppKind::Gol {
        Some(SeedRuntime::new(state))
    } else {
        None
    };
    let mut gol_petri = if state.app_kind == AppKind::Gol {
        Some(PetriDishRuntime::new(state))
    } else {
        None
    };
    let mut games_petri = if state.app_kind == AppKind::Games {
        Some(GamesPetriDishRuntime::new(state))
    } else {
        None
    };
    let mut games_config_preview = if state.app_kind == AppKind::Games {
        Some(GamesConfigPreviewRuntime::spawn())
    } else {
        None
    };
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

        // Poll input with tick fallback (check stashed events from scroll coalescing first)
        let timeout = TICK_RATE;
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
                    let mut mouse_changed = handle_mouse_event_with_swarm(
                        &swarm,
                        mouse,
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

        // codex runner events
        while let Ok(event) = codex_runner.events.try_recv() {
            record_agent_bus_vitals(&mut vitals, &event);
            let finished = matches!(
                event,
                AgentBusEvent::TurnCompleted { .. } | AgentBusEvent::TurnFailed { .. }
            );
            // Snapshot the currently-viewed artifact so we can re-resolve
            // the index after the card list changes.
            let pinned_popup_ref = if state.agents.artifacts_popup_open {
                agent_ops_view::artifacts_popup_ref(state, &swarm, state.agents.ops_viewport_width)
            } else {
                None
            };
            let clear_invalid_thread_context = match &event {
                AgentBusEvent::TurnFailed {
                    agent_id,
                    mission_id,
                    message,
                    ..
                } if codex_thread_context_not_found(message) => {
                    Some((agent_id.clone(), mission_id.clone()))
                }
                _ => None,
            };
            event.apply(state);
            if let Some((agent_id, mission_id)) = clear_invalid_thread_context {
                clear_codex_thread_context_for_agent(
                    state,
                    agent_id.as_str(),
                    mission_id.as_deref(),
                );
            }
            drain_pending_claim_retries(state, &mut vitals, &codex_runner, &claude_runner);
            drain_pending_interventions(state, &mut vitals, &codex_runner, &claude_runner);
            let swarm_outcome = swarm.handle_event_outcome(state, &event);
            maybe_follow_swarm_artifact_in_popup(
                state,
                &swarm,
                swarm_outcome.artifact_focus.as_ref(),
            );
            for mut dispatch in swarm_outcome.dispatches {
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
            let shadow_outcome = shadow.handle_event_outcome(state, &event);
            for dispatch in shadow_outcome.dispatches {
                dispatch_shadow_outcome(
                    state,
                    &mut vitals,
                    &codex_runner,
                    &claude_runner,
                    dispatch,
                );
            }
            if finished {
                maybe_dispatch_next_queued_codex_turn(state, &mut vitals, Some(&codex_runner));
                maybe_dispatch_next_queued_claude_turn(state, &mut vitals, Some(&claude_runner));
                // Clean up chat clones that are done.
                if let AgentBusEvent::TurnCompleted { agent_id, .. }
                | AgentBusEvent::TurnFailed { agent_id, .. } = &event
                {
                    crate::swarm::cleanup_idle_chat_clone(state, agent_id);
                }
                // Dispatch genome evaluations to background threads.
                // Also fires on TurnFailed when the agent wrote files
                // before crashing: integrators often hit max-turns or exit
                // non-zero during cosmetic cleanup after the real work is
                // already on disk. Without this, failed-but-wrote runs
                // silently skip the entire genome retry pipeline.
                match &event {
                    AgentBusEvent::TurnCompleted {
                        agent_id,
                        mission_id,
                        ..
                    } => {
                        dispatch_turn_genome_evals(state, &genome_worker, agent_id, mission_id);
                    }
                    AgentBusEvent::TurnFailed {
                        agent_id,
                        mission_id,
                        ..
                    } if state
                        .genome_turn_modified
                        .get(agent_id)
                        .is_some_and(|s| !s.is_empty()) =>
                    {
                        dispatch_turn_genome_evals(state, &genome_worker, agent_id, mission_id);
                    }
                    _ => {}
                }
            }
            // Re-resolve the pinned artifact so the popup stays on the
            // same card even when new cards shift the indices.
            if let Some(ref pinned) = pinned_popup_ref {
                if let Some(idx) = agent_ops_view::artifacts_card_index_for_popup_ref(
                    state,
                    Some(&swarm),
                    state.agents.ops_viewport_width,
                    pinned,
                ) {
                    state.agents.artifacts_selected = idx;
                }
            }
            needs_redraw = true;
        }

        // claude runner events
        while let Ok(event) = claude_runner.events.try_recv() {
            record_agent_bus_vitals(&mut vitals, &event);
            let finished = matches!(
                event,
                AgentBusEvent::TurnCompleted { .. } | AgentBusEvent::TurnFailed { .. }
            );
            let pinned_popup_ref = if state.agents.artifacts_popup_open {
                agent_ops_view::artifacts_popup_ref(state, &swarm, state.agents.ops_viewport_width)
            } else {
                None
            };
            let clear_invalid_session_context = match &event {
                AgentBusEvent::TurnFailed {
                    agent_id,
                    mission_id,
                    message,
                    ..
                } if claude_session_context_not_found(message) => {
                    Some((agent_id.clone(), mission_id.clone()))
                }
                _ => None,
            };
            apply_claude_event(state, &event);
            if let Some((agent_id, mission_id)) = clear_invalid_session_context {
                clear_claude_session_context_for_agent(
                    state,
                    agent_id.as_str(),
                    mission_id.as_deref(),
                );
            }
            drain_pending_claim_retries(state, &mut vitals, &codex_runner, &claude_runner);
            drain_pending_interventions(state, &mut vitals, &codex_runner, &claude_runner);
            let swarm_outcome = swarm.handle_event_outcome(state, &event);
            maybe_follow_swarm_artifact_in_popup(
                state,
                &swarm,
                swarm_outcome.artifact_focus.as_ref(),
            );
            for mut dispatch in swarm_outcome.dispatches {
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
            let shadow_outcome = shadow.handle_event_outcome(state, &event);
            for dispatch in shadow_outcome.dispatches {
                dispatch_shadow_outcome(
                    state,
                    &mut vitals,
                    &codex_runner,
                    &claude_runner,
                    dispatch,
                );
            }
            if finished {
                maybe_dispatch_next_queued_codex_turn(state, &mut vitals, Some(&codex_runner));
                maybe_dispatch_next_queued_claude_turn(state, &mut vitals, Some(&claude_runner));
                if let AgentBusEvent::TurnCompleted { agent_id, .. }
                | AgentBusEvent::TurnFailed { agent_id, .. } = &event
                {
                    crate::swarm::cleanup_idle_chat_clone(state, agent_id);
                }
                // Dispatch genome evaluations to background threads.
                // Also fires on TurnFailed when the agent wrote files
                // before crashing: integrators often hit max-turns or exit
                // non-zero during cosmetic cleanup after the real work is
                // already on disk. Without this, failed-but-wrote runs
                // silently skip the entire genome retry pipeline.
                match &event {
                    AgentBusEvent::TurnCompleted {
                        agent_id,
                        mission_id,
                        ..
                    } => {
                        dispatch_turn_genome_evals(state, &genome_worker, agent_id, mission_id);
                    }
                    AgentBusEvent::TurnFailed {
                        agent_id,
                        mission_id,
                        ..
                    } if state
                        .genome_turn_modified
                        .get(agent_id)
                        .is_some_and(|s| !s.is_empty()) =>
                    {
                        dispatch_turn_genome_evals(state, &genome_worker, agent_id, mission_id);
                    }
                    _ => {}
                }
            }
            if let Some(ref pinned) = pinned_popup_ref {
                if let Some(idx) = agent_ops_view::artifacts_card_index_for_popup_ref(
                    state,
                    Some(&swarm),
                    state.agents.ops_viewport_width,
                    pinned,
                ) {
                    state.agents.artifacts_selected = idx;
                }
            }
            needs_redraw = true;
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

        // redraw
        if needs_redraw || last_tick.elapsed() >= TICK_RATE {
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

            // Drain file watcher notifications — reload any open buffers that changed on disk.
            drain_file_watcher(state, syntax, &file_watcher, &genome_worker);
            let prescan_completed = drain_genome_results(
                state,
                &genome_worker,
                &mut vitals,
                &codex_runner,
                &claude_runner,
            );
            // Proposer pre-scan bookkeeping.
            // (a) Let the swarm AND shadow runtime know which scope files just
            //     got reports so they can release blocked propose tasks.
            for path in &prescan_completed {
                let dispatches = swarm.note_prescan_result(path);
                for mut dispatch in dispatches {
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
                for mut dispatch in shadow.note_prescan_result(path) {
                    augment_shadow_prompt_with_landscape(state, &mut dispatch);
                    dispatch_agent_prompt(
                        state,
                        &mut vitals,
                        Some(&codex_runner),
                        Some(&claude_runner),
                        dispatch.agent_id,
                        dispatch.mission_id,
                        dispatch.prompt,
                    );
                }
            }
            // (b) Kick off any outstanding prescan evals non-blocking. Each
            //     path spawns a short-lived worker thread; main loop never
            //     waits. Only enabled when genome_context is on — otherwise
            //     there's no point populating reports that no one reads.
            if state.settings.genome.genome_context_enabled {
                let workspace_root = state.workspace_root.clone();
                let paths = swarm.take_pending_prescan_paths(state, &workspace_root);
                if !paths.is_empty() {
                    // One-shot status per mission (not per batch), so the
                    // rate-limited dispatch loop doesn't spam the transcript.
                    let announce = swarm.announce_prescan_start();
                    for (mid, total) in announce {
                        crate::swarm::push_system_message_to_mission(
                            state,
                            &mid,
                            format!("Proposer (Genome check): evaluating {total} file(s)"),
                        );
                    }
                    for path in paths {
                        genome_worker.evaluate_from_disk_prescan(path);
                    }
                }
                for path in shadow.take_pending_prescan_paths() {
                    genome_worker.evaluate_from_disk_prescan(path);
                }
            }

            // Dispatch save-triggered genome evaluation to background worker.
            if let Some(path) = state.genome_save_eval_pending.take() {
                let text = state.editor_buffer().content_as_string();
                genome_worker.evaluate_save(path, text);
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
