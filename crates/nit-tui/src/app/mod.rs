use std::collections::BTreeSet;
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
        help_overlay, protocol_picker, rule_picker, top_bar, visualizer_view,
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
    Prompt, SavedRunHistoryFilter, SavedRunHistoryPendingAction, SearchMode, UiSelection,
    UiSelectionPane, YankKind, CONSOLE_SCROLL_BOTTOM,
};
use nit_games::config::GamesConfig;
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::Style,
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Terminal,
};

mod artifacts_popup_handler;
mod chat_input;
mod dispatch;
use artifacts_popup_handler::{
    artifacts_popup_scroll_metrics, copy_popup_chat_input_selection, handle_artifacts_popup_key,
    insert_popup_chat_text,
};
use chat_input::{
    chat_input_byte_index, handle_chat_input_editing_key, slice_by_char,
    submit_chat_input_and_dispatch,
};
use dispatch::*;

const TICK_RATE: Duration = Duration::from_millis(50);
const JOB_TICK: Duration = Duration::from_millis(120);
const BUSY_PULSE_INTERVAL: Duration = Duration::from_millis(550);
const CHORD_TIMEOUT: Duration = Duration::from_millis(300);
const INSPECTOR_JUMP_TIMEOUT: Duration = Duration::from_millis(1500);
const INITIAL_SIZE_SETTLE: Duration = Duration::from_millis(80);
const VERIFY_OUTPUT_MD_MAX_CHARS: usize = 4_000;

struct FuzzySearchRuntime {
    indexer: FileIndexRunner,
    fuzzy: FuzzyMatcherRunner,
    content: ContentSearchRunner,
    preview: PreviewRunner,

    index_gen: u64,
    file_gen: u64,
    content_gen: u64,
    preview_gen: u64,

    index_ready: bool,
    index_filters: Option<(bool, bool)>,

    preview_model: Option<PreviewModel>,
    last_preview_key: Option<PreviewKey>,
    preview_scroll_delta: i32,
    last_open: bool,
}

enum GamesConfigPreviewCommand {
    Parse {
        version: u64,
        config_text: String,
        workspace_root: PathBuf,
    },
    Shutdown,
}

struct GamesConfigPreviewEvent {
    version: u64,
    result: Result<nit_games::config::NormalizedConfig, nit_games::config::ConfigError>,
}

struct GamesConfigPreviewRuntime {
    cmd_tx: Sender<GamesConfigPreviewCommand>,
    events: Receiver<GamesConfigPreviewEvent>,
    handle: Option<JoinHandle<()>>,
    pending_version: Option<u64>,
}

impl GamesConfigPreviewRuntime {
    fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<GamesConfigPreviewCommand>();
        let (event_tx, event_rx) = mpsc::channel::<GamesConfigPreviewEvent>();
        let handle = thread::Builder::new()
            .name("nit-games-config-preview".into())
            .spawn(move || {
                while let Ok(cmd) = cmd_rx.recv() {
                    match cmd {
                        GamesConfigPreviewCommand::Parse {
                            version,
                            config_text,
                            workspace_root,
                        } => {
                            let result = GamesConfig::from_toml_with_root(
                                &config_text,
                                Some(&workspace_root),
                            );
                            let _ = event_tx.send(GamesConfigPreviewEvent { version, result });
                        }
                        GamesConfigPreviewCommand::Shutdown => break,
                    }
                }
            })
            .expect("spawn games config preview");
        Self {
            cmd_tx,
            events: event_rx,
            handle: Some(handle),
            pending_version: None,
        }
    }

    fn request_for_editor(&mut self, state: &mut AppState) {
        if state.app_kind != AppKind::Games {
            return;
        }
        let version = state.editor_buffer().version();
        if state
            .games
            .config_preview
            .as_ref()
            .is_some_and(|preview| preview.version == version)
            || self.pending_version == Some(version)
        {
            return;
        }
        let cmd = GamesConfigPreviewCommand::Parse {
            version,
            config_text: state.editor_buffer().content_as_string(),
            workspace_root: state.workspace_root.clone(),
        };
        if self.cmd_tx.send(cmd).is_ok() {
            self.pending_version = Some(version);
            state.games.config_preview_pending = true;
        } else {
            state.games.config_preview_pending = false;
        }
    }

    fn poll(&mut self, state: &mut AppState) {
        while let Ok(event) = self.events.try_recv() {
            state.games.config_preview = Some(nit_core::GamesConfigPreview {
                version: event.version,
                result: event.result,
            });
            if self.pending_version == Some(event.version) {
                self.pending_version = None;
            }
            if state.editor_buffer().version() == event.version {
                state.games.config_preview_pending = false;
            }
        }
    }

    fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(GamesConfigPreviewCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreviewKey {
    mode: SearchMode,
    path: PathBuf,
    line_hint: usize,
    query: String,
}

impl FuzzySearchRuntime {
    fn new(theme: &Theme, highlight: nit_core::HighlightConfig) -> Self {
        Self {
            indexer: FileIndexRunner::spawn(),
            fuzzy: FuzzyMatcherRunner::spawn(),
            content: ContentSearchRunner::spawn(),
            preview: PreviewRunner::spawn(theme.clone(), highlight),
            index_gen: 0,
            file_gen: 0,
            content_gen: 0,
            preview_gen: 0,
            index_ready: false,
            index_filters: None,
            preview_model: None,
            last_preview_key: None,
            preview_scroll_delta: 0,
            last_open: false,
        }
    }

    fn shutdown(&mut self) {
        self.indexer.shutdown();
        self.fuzzy.shutdown();
        self.content.shutdown();
        self.preview.shutdown();
    }

    fn update_syntax_config(&self, highlight: nit_core::HighlightConfig) {
        self.preview.update_config(highlight);
    }

    fn tick_open(&mut self, state: &mut AppState) {
        if state.fuzzy_search.open {
            if !self.last_open {
                self.last_open = true;
                self.preview_model = None;
                self.last_preview_key = None;
                self.preview_scroll_delta = 0;
                self.ensure_index(state);
                self.run_search_for_mode(state);
                self.request_preview_for_selection(state);
            }
            return;
        }
        if self.last_open {
            self.last_open = false;
            self.preview_model = None;
            self.last_preview_key = None;
            self.preview_scroll_delta = 0;
            self.file_gen = self.file_gen.wrapping_add(1);
            self.preview_gen = self.preview_gen.wrapping_add(1);
            self.fuzzy.send(FuzzyCommand::Query {
                generation: 0,
                query: String::new(),
            });
            state.fuzzy_search.status_msg.clear();
            state.fuzzy_search.indexing = false;
            state.fuzzy_search.searching = false;
            // Cancel any in-flight content search quickly.
            self.content_gen = self.content_gen.wrapping_add(1);
            self.content.search(
                self.content_gen,
                state.workspace_root.clone(),
                String::new(),
                state.fuzzy_search.show_hidden,
                state.fuzzy_search.show_ignored,
            );
        }
    }

    fn ensure_index(&mut self, state: &mut AppState) {
        let filters = (
            state.fuzzy_search.show_hidden,
            state.fuzzy_search.show_ignored,
        );
        let needs = !self.index_ready || self.index_filters != Some(filters);
        if !needs {
            return;
        }
        self.index_filters = Some(filters);
        self.index_ready = false;
        self.index_gen = self.index_gen.wrapping_add(1);
        state.fuzzy_search.indexing = true;
        state.fuzzy_search.status_msg = "Indexing…".into();
        self.preview_model = None;
        self.preview_scroll_delta = 0;

        self.fuzzy.send(FuzzyCommand::ResetIndex {
            generation: self.index_gen,
            root: state.workspace_root.clone(),
        });
        self.indexer.build(
            self.index_gen,
            state.workspace_root.clone(),
            state.fuzzy_search.show_hidden,
            state.fuzzy_search.show_ignored,
        );
    }

    fn rebuild_index(&mut self, state: &mut AppState) {
        self.index_ready = false;
        self.index_filters = None;
        self.ensure_index(state);
    }

    fn run_search_for_mode(&mut self, state: &mut AppState) {
        match state.fuzzy_search.mode {
            SearchMode::Files => self.run_file_query(state),
            SearchMode::Content => self.run_content_query(state),
        }
    }

    fn run_file_query(&mut self, state: &mut AppState) {
        self.file_gen = self.file_gen.wrapping_add(1);
        state.fuzzy_search.searching = false;
        state.fuzzy_search.file_results.clear();
        state.fuzzy_search.selected = 0;
        state.fuzzy_search.scroll_offset = 0;
        self.fuzzy.send(FuzzyCommand::Query {
            generation: self.file_gen,
            query: state.fuzzy_search.query.clone(),
        });
    }

    fn run_content_query(&mut self, state: &mut AppState) {
        self.content_gen = self.content_gen.wrapping_add(1);
        state.fuzzy_search.match_results.clear();
        state.fuzzy_search.selected = 0;
        state.fuzzy_search.scroll_offset = 0;
        let query = state.fuzzy_search.query.trim().to_string();
        if query.is_empty() {
            state.fuzzy_search.searching = false;
            state.fuzzy_search.status_msg = "Type to search".into();
            self.content.search(
                self.content_gen,
                state.workspace_root.clone(),
                String::new(),
                state.fuzzy_search.show_hidden,
                state.fuzzy_search.show_ignored,
            );
            return;
        }
        state.fuzzy_search.searching = true;
        state.fuzzy_search.status_msg = "Searching…".into();
        self.content.search(
            self.content_gen,
            state.workspace_root.clone(),
            query,
            state.fuzzy_search.show_hidden,
            state.fuzzy_search.show_ignored,
        );
    }

    fn request_preview_for_selection(&mut self, state: &AppState) {
        if !state.fuzzy_search.open {
            return;
        }
        let (path, line_hint, query) = match state.fuzzy_search.mode {
            SearchMode::Files => {
                let Some(item) = state
                    .fuzzy_search
                    .file_results
                    .get(state.fuzzy_search.selected)
                else {
                    return;
                };
                (item.abs_path.clone(), None, String::new())
            }
            SearchMode::Content => {
                let Some(item) = state
                    .fuzzy_search
                    .match_results
                    .get(state.fuzzy_search.selected)
                else {
                    return;
                };
                (
                    item.abs_path.clone(),
                    Some(item.line),
                    state.fuzzy_search.query.trim().to_string(),
                )
            }
        };
        let key = PreviewKey {
            mode: state.fuzzy_search.mode,
            path: path.clone(),
            line_hint: line_hint.unwrap_or(0),
            query: if matches!(state.fuzzy_search.mode, SearchMode::Content) {
                query.clone()
            } else {
                String::new()
            },
        };
        if self.last_preview_key.as_ref() == Some(&key) {
            return;
        }
        self.preview_scroll_delta = 0;
        self.last_preview_key = Some(key);
        self.preview_gen = self.preview_gen.wrapping_add(1);
        self.preview.request(
            self.preview_gen,
            state.fuzzy_search.mode,
            path,
            line_hint,
            query,
        );
    }
}

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
fn run_loop(
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
    let mut last_vitals_sample = Instant::now();
    let mut last_busy_pulse = Instant::now();
    let app_start = Instant::now();
    let mut last_resize_event: Option<(Duration, u16, u16)> = None;
    let mut needs_redraw = true;
    let mut input_state = InputState::new();
    let mut system_stats = SystemStats::new();
    let mut clipboard = Clipboard::new().ok();
    let mut file_tree_runner = FileTreeRunner::spawn();
    let has_codex = state.agents.agents.iter().any(|lane| lane.is_codex());
    let codex_runtime = if has_codex {
        codex_runtime
    } else {
        CodexRuntimeMode::Exec
    };
    state.agents.codex_max_parallel_turns = codex_config.max_parallel_turns;
    let mut codex_runner = CodexRunner::spawn(codex_runtime, codex_config);
    state.agents.claude_max_parallel_turns = claude_config.max_parallel_turns;
    let mut claude_runner = ClaudeRunner::spawn(claude_config);
    let mut swarm = SwarmRuntime::default();
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

        // Poll input with tick fallback
        let timeout = TICK_RATE;
        let mut handled_input = false;
        if event::poll(timeout)? {
            match event::read()? {
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
                    if state.agents.artifacts_history_popup_open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_artifacts_history_popup_key(&key, state, screen, theme) {
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.agents.artifacts_popup_open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_artifacts_popup_key(
                            &key,
                            state,
                            &mut swarm,
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
                    if state.show_help {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_help_popup_key(&key, state, screen, theme) {
                            needs_redraw = true;
                            continue;
                        }
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
                    if handle_mouse_event_with_swarm(
                        &swarm,
                        mouse,
                        screen,
                        state,
                        &mut fuzzy_runtime,
                        &mut input_state,
                        &mut clipboard,
                        theme,
                    ) {
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
            let swarm_outcome = swarm.handle_event_outcome(state, &event);
            maybe_follow_swarm_artifact_in_popup(
                state,
                &swarm,
                swarm_outcome.artifact_focus.as_ref(),
            );
            for dispatch in swarm_outcome.dispatches {
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
            if finished {
                maybe_dispatch_next_queued_codex_turn(state, &mut vitals, Some(&codex_runner));
                maybe_dispatch_next_queued_claude_turn(state, &mut vitals, Some(&claude_runner));
                // Clean up chat clones that are done.
                if let AgentBusEvent::TurnCompleted { agent_id, .. }
                | AgentBusEvent::TurnFailed { agent_id, .. } = &event
                {
                    crate::swarm::cleanup_idle_chat_clone(state, agent_id);
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
            let swarm_outcome = swarm.handle_event_outcome(state, &event);
            maybe_follow_swarm_artifact_in_popup(
                state,
                &swarm,
                swarm_outcome.artifact_focus.as_ref(),
            );
            for dispatch in swarm_outcome.dispatches {
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
            if finished {
                maybe_dispatch_next_queued_claude_turn(state, &mut vitals, Some(&claude_runner));
                if let AgentBusEvent::TurnCompleted { agent_id, .. }
                | AgentBusEvent::TurnFailed { agent_id, .. } = &event
                {
                    crate::swarm::cleanup_idle_chat_clone(state, agent_id);
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
                fuzzy_runtime.preview_scroll_delta,
                &vitals_snapshot,
            )?;
            needs_redraw = false;
            last_tick = Instant::now();
        }
    }
    file_tree_runner.shutdown();
    codex_runner.shutdown();
    claude_runner.shutdown();
    fuzzy_runtime.shutdown();
    if let Some(runtime) = games_config_preview.as_mut() {
        runtime.shutdown();
    }
    Ok(())
}

fn settle_initial_terminal_size(
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

fn is_lab_job_running(state: &AppState) -> bool {
    match state.app_kind {
        AppKind::Gol => state.visualizer.running && !state.visualizer.paused,
        AppKind::Games => state.games.running && !state.games.paused,
    }
}

fn record_log_line_vitals(vitals: &mut VitalsState, now: Instant, line: &str) {
    if let Some(severity) = log_diag_severity(line) {
        vitals.record_diag_event(now, severity);
    }
    if line_looks_fatal(line) {
        vitals.mark_fatal(now);
    }
}

fn append_log_to_agent_diagnostics(state: &mut AppState, line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    let severity = match log_diag_severity(trimmed) {
        Some(DiagSeverity::Error) => AgentAlertSeverity::Error,
        Some(DiagSeverity::Warn) => AgentAlertSeverity::Warn,
        None => AgentAlertSeverity::Info,
    };
    state.agents.diag_events.push(AgentDiagnosticEvent {
        severity,
        source: "runtime".into(),
        message: trimmed.to_string(),
        at: timestamp_label(state),
    });
    if state.agents.diag_events.len() > 512 {
        let drop = state.agents.diag_events.len().saturating_sub(512);
        if drop > 0 {
            state.agents.diag_events.drain(0..drop);
        }
    }
}

fn record_agent_bus_vitals(vitals: &mut VitalsState, event: &AgentBusEvent) {
    let now = Instant::now();
    match event {
        AgentBusEvent::TurnFailed { .. } => vitals.record_diag_event(now, DiagSeverity::Error),
        AgentBusEvent::TurnLog { message, .. } => {
            let lowered = message.to_ascii_lowercase();
            if lowered.contains("error") || lowered.contains("failed") {
                vitals.record_diag_event(now, DiagSeverity::Warn);
            }
        }
        AgentBusEvent::AlertAppend { alert } => match alert.severity {
            AgentAlertSeverity::Error => vitals.record_diag_event(now, DiagSeverity::Error),
            AgentAlertSeverity::Warn => vitals.record_diag_event(now, DiagSeverity::Warn),
            AgentAlertSeverity::Info => {}
        },
        AgentBusEvent::DiagnosticAppend { event: diag } => match diag.severity {
            AgentAlertSeverity::Error => vitals.record_diag_event(now, DiagSeverity::Error),
            AgentAlertSeverity::Warn => vitals.record_diag_event(now, DiagSeverity::Warn),
            AgentAlertSeverity::Info => {}
        },
        _ => {}
    }
}

fn tick_agent_turn_liveness(state: &mut AppState) {
    // Backends can emit periodic `TurnHeartbeat` events. Use those to keep the roster "HB"
    // column honest and to surface stalls when heartbeats stop.
    let now = Instant::now();
    let (agents, active_turns) = {
        let agents_state = &mut state.agents;
        (&mut agents_state.agents, &agents_state.active_turns)
    };
    for agent in agents.iter_mut() {
        if !matches!(agent.status, AgentStatus::Running) {
            continue;
        }
        let Some(turn) = active_turns.get(&agent.id) else {
            continue;
        };
        let age = now
            .checked_duration_since(turn.last_heartbeat_at)
            .or_else(|| now.checked_duration_since(turn.started_at))
            .map(|d| d.as_secs())
            .unwrap_or(0);
        agent.heartbeat_age_secs = age;
    }
}

fn is_background_work_active(state: &AppState) -> bool {
    match state.app_kind {
        AppKind::Gol => false,
        AppKind::Games => {
            state.games.running
                || state.games.pending_run
                || state.games.family_building
                || state.games.analysis.running
                || state.games.run_browser.loading
                || state.games.replay.loading
                || state.games.config_preview_pending
                || state.games.pending_analyze.is_some()
                || state.games.pending_run_load.is_some()
                || state.games.pending_replay.is_some()
                || state.status.as_deref().is_some_and(status_looks_busy)
        }
    }
}

fn status_looks_busy(status_text: &str) -> bool {
    let lower = status_text.to_ascii_lowercase();
    lower.contains("queued")
        || lower.contains("running")
        || lower.contains("loading")
        || lower.contains("pending")
        || lower.contains("preparing")
        || lower.contains("started")
        || lower.contains("busy")
}

fn current_agent_state(state: &AppState) -> AgentVitalsState {
    let enabled = !state.agents.agents.is_empty();
    let connected = matches!(state.agents.mcp.state, McpConnectionState::Connected);
    let active_tasks = state
        .agents
        .agents
        .iter()
        .any(|agent| matches!(agent.status, AgentStatus::Running) || agent.queue_len > 0);
    AgentVitalsState {
        enabled,
        connected,
        active_tasks,
    }
}

fn log_diag_severity(line: &str) -> Option<DiagSeverity> {
    let upper = line.to_ascii_uppercase();
    if upper.contains("PANIC") || upper.contains("ERROR") || upper.contains("FAILED") {
        Some(DiagSeverity::Error)
    } else if upper.contains("WARN") {
        Some(DiagSeverity::Warn)
    } else {
        None
    }
}

fn line_looks_fatal(line: &str) -> bool {
    let upper = line.to_ascii_uppercase();
    upper.contains("PANIC") || upper.contains("BACKTRACE")
}

#[allow(clippy::too_many_arguments)]
fn draw(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
    syntax: &mut SyntaxRuntime,
    system_stats: &SystemStats,
    seed_runtime: &mut Option<SeedRuntime>,
    gol_petri: &mut Option<PetriDishRuntime>,
    games_petri: &mut Option<GamesPetriDishRuntime>,
    fuzzy_preview: Option<&PreviewModel>,
    fuzzy_preview_scroll_delta: i32,
    vitals: &LabVitalsSnapshot,
) -> io::Result<()> {
    let start = Instant::now();
    terminal.draw(|f| {
        let screen = f.size();
        let layout = layout::split(screen);

        // Update viewports (account for gutters)
        let editor_total = state.editor_buffer().lines_len().max(1);
        let editor_line_width = editor_total.to_string().len().max(3) as u16;
        let editor_gutter = editor_line_width + 4;
        let editor_text_width = layout
            .editor
            .width
            .saturating_sub(2)
            .saturating_sub(editor_gutter);
        let editor_height = layout.editor.height.saturating_sub(2) as usize;
        let editor_width = editor_text_width as usize;
        {
            let buf = state.editor_buffer_mut();
            let resized =
                buf.viewport.height != editor_height || buf.viewport.width != editor_width;
            buf.set_viewport_size(editor_height, editor_width);
            if resized {
                buf.ensure_visible();
            }
        }
        let editor_id = state.active_editor_buffer_id;
        let notes_id = state.notes_buffer_id;
        top_bar::render(f, layout.top, state, theme, vitals);
        let editor_cursor = if state.file_tree.open {
            adjust_file_tree_scroll(state, layout.editor);
            file_tree_view::render(f, layout.editor, state, theme);
            None
        } else {
            let editor_render = syntax.render_snapshot_for(editor_id, state.editor_buffer());
            editor_view::render_editor(
                f,
                layout.editor,
                state.editor_buffer(),
                editor_render.snapshot,
                editor_render.line_map,
                state.focus,
                state.mode,
                theme,
                state.settings.editor.tab_width as usize,
            )
        };
        let notes_cursor = agent_console_view::render(f, layout.notes, state, swarm, theme);
        {
            let text_area = job_output_text_area(layout.job);
            state.agents.ops_viewport_width = text_area.width.max(1) as usize;
            state.agents.ops_viewport_height = text_area.height.max(1) as usize;
        }
        let job_cursor = if state.agents.dock_tab == AgentOpsTab::Scratchpad {
            let focused = state.focus == PaneId::JobOutput;
            let border_style = if focused {
                Style::default().fg(theme.border_focused)
            } else {
                Style::default().fg(theme.border)
            };
            let border_type = if focused {
                BorderType::Thick
            } else {
                BorderType::Plain
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .title("AGENT OPS")
                .border_style(border_style)
                .border_type(border_type)
                .style(Style::default().bg(theme.background));
            f.render_widget(block.clone(), layout.job);
            let outer_inner = block.inner(layout.job);
            if outer_inner.width >= 4 && outer_inner.height >= 3 {
                let outer_chunks = ratatui::layout::Layout::default()
                    .direction(ratatui::layout::Direction::Vertical)
                    .constraints([
                        ratatui::layout::Constraint::Length(1),
                        ratatui::layout::Constraint::Min(1),
                    ])
                    .split(outer_inner);
                agent_ops_view::render_tab_bar(f, outer_chunks[0], state, theme);
                let scratchpad_area = outer_chunks[1];

                let notes_total = state.notes_buffer().lines_len().max(1);
                let notes_line_width = notes_total.to_string().len().max(3) as u16;
                let notes_gutter = notes_line_width + 4;
                let notes_text_width = scratchpad_area
                    .width
                    .saturating_sub(2)
                    .saturating_sub(notes_gutter);
                let notes_height = scratchpad_area.height.saturating_sub(2) as usize;
                let notes_width = notes_text_width as usize;
                {
                    let buf = state.notes_buffer_mut();
                    let resized =
                        buf.viewport.height != notes_height || buf.viewport.width != notes_width;
                    buf.set_viewport_size(notes_height, notes_width);
                    if resized {
                        buf.ensure_visible();
                    }
                }
                let notes_render = syntax.render_snapshot_for(notes_id, state.notes_buffer());
                editor_view::render_buffer(
                    f,
                    scratchpad_area,
                    state.notes_buffer(),
                    notes_render.snapshot,
                    notes_render.line_map,
                    PaneId::JobOutput,
                    state.focus,
                    "SCRATCHPAD",
                    theme,
                    state.settings.editor.tab_width as usize,
                    true,
                    state.mode,
                )
            } else {
                None
            }
        } else {
            agent_ops_view::render(f, layout.job, state, swarm, theme);
            None
        };
        match state.app_kind {
            AppKind::Gol => {
                let viz_inner_width = layout.visualizer.width.saturating_sub(2) as usize;
                let viz_inner_height = layout.visualizer.height.saturating_sub(2) as usize;
                let viz_grid_rows = viz_inner_height.saturating_sub(1);
                let (grid_w, grid_h) = crate::seed_render::grid_size_for_mode(
                    viz_inner_width,
                    viz_grid_rows,
                    state.visualizer.seed_plate_mode,
                );
                if let Some(seed_runtime) = seed_runtime.as_mut() {
                    seed_runtime.ensure_size(grid_w, grid_h, state);
                    visualizer_view::render(f, layout.visualizer, state, theme, seed_runtime);
                }
            }
            AppKind::Games => {
                games_visualizer_view::render(
                    f,
                    layout.visualizer,
                    state,
                    theme,
                    current_games_config_result(state),
                    state.games.config_preview_pending,
                );
            }
        }
        let syntax_status = syntax.status_label_for(editor_id, state.editor_buffer().version());
        let syntax_debug = {
            let latest = syntax.latest_snapshot_for(editor_id);
            nit_core::SyntaxDebugInfo {
                buffer_version: state.editor_buffer().version(),
                snapshot_version: latest.map(|s| s.version),
                engine_state: syntax.engine_state_label(editor_id),
                last_job_ms: latest.map(|s| s.duration_ms),
            }
        };
        state.syntax_status = syntax_status.clone();
        state.syntax_debug = Some(syntax_debug.clone());
        gate_monitor_view::render(f, layout.gate, state, theme);
        bottom_bar::render(f, layout.bottom, state, theme, system_stats);

        match state.app_kind {
            AppKind::Gol => {
                if let (Some(petri), Some(seed_runtime)) =
                    (gol_petri.as_mut(), seed_runtime.as_mut())
                {
                    petri.handle_pending_requests(state, seed_runtime, screen);
                    petri.render(f, screen, state, theme);
                }
            }
            AppKind::Games => {
                if let Some(petri) = games_petri.as_mut() {
                    petri.handle_pending_requests(state);
                    petri.render(f, screen, state, theme);
                }
            }
        }
        if state.app_kind == AppKind::Games && state.games.analysis.open {
            let area = dynamic_popup_rect(screen, games_analysis_popup::preferred_size(screen));
            games_analysis_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.run_browser.open {
            let area = dynamic_popup_rect(screen, games_run_browser_popup::preferred_size(screen));
            games_run_browser_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.replay.open {
            let area = dynamic_popup_rect(screen, games_replay_popup::preferred_size(screen));
            games_replay_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.strategy_inspect.open {
            let area = dynamic_popup_rect(screen, games_strategy_popup::preferred_size(screen));
            games_strategy_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.tm_sim.open {
            let area = dynamic_popup_rect(screen, games_tm_sim_popup::preferred_size(screen));
            games_tm_sim_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.ca_sim.open {
            let area = dynamic_popup_rect(screen, games_ca_sim_popup::preferred_size(screen));
            games_ca_sim_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.match_history.open {
            let area =
                dynamic_popup_rect(screen, games_match_history_popup::preferred_size(screen));
            games_match_history_popup::render(f, area, state, theme);
        }
        if state.rule_picker.open {
            rule_picker::render(f, screen, state, theme);
        }
        if state.agents.artifacts_history_popup_open {
            let area = dynamic_popup_rect(screen, artifacts_history_popup::preferred_size(screen));
            artifacts_history_popup::render(f, area, state, theme);
        }
        let artifacts_popup_cursor = if state.agents.artifacts_popup_open {
            let area = dynamic_popup_rect(screen, artifacts_popup::preferred_size(screen));
            artifacts_popup::render(f, area, state, swarm, theme)
        } else {
            None
        };
        if state.show_help {
            let area = dynamic_popup_rect(screen, help_overlay::preferred_size(screen));
            help_overlay::render(f, area, state, theme);
        }
        if state.fuzzy_search.open {
            let area = dynamic_popup_rect(screen, fuzzy_popup_size(screen, state));
            fuzzy_search_popup::render(
                f,
                area,
                state,
                theme,
                fuzzy_preview,
                fuzzy_preview_scroll_delta,
            );
        }
        if let Some(Prompt::ConfirmQuit) = state.prompt {
            let message = "Quit without saving? (Y/N)";
            let area = dynamic_popup_rect(screen, prompt_size(message));
            render_prompt(f, area, theme, message);
        }
        let mut command_cursor = None;
        if let Some(cmd) = state.command_line.as_ref() {
            let message = format!(":{}", cmd.input);
            let area = dynamic_popup_rect(screen, prompt_size(&message));
            render_command_prompt(f, area, theme, &message);
            command_cursor = command_prompt_cursor(area, &cmd.input, cmd.cursor);
        }
        let fuzzy_cursor = if state.fuzzy_search.open {
            let area = dynamic_popup_rect(screen, fuzzy_popup_size(screen, state));
            fuzzy_search_cursor(area, state)
        } else {
            None
        };

        // Cursor: only set it when we actually want a visible caret. If we don't set a cursor,
        // ratatui will hide it; calling `set_cursor(0, 0)` makes it look like the cursor is
        // jumping to the top-left corner.
        let petri_visible = match state.app_kind {
            AppKind::Gol => gol_petri.as_ref().map(|p| p.is_visible()).unwrap_or(false),
            AppKind::Games => games_petri
                .as_ref()
                .map(|p| p.is_visible())
                .unwrap_or(false),
        };
        let cursor = if let Some((x, y)) = command_cursor {
            Some((x, y))
        } else if let Some((x, y)) = fuzzy_cursor {
            Some((x, y))
        } else if let Some((x, y)) = artifacts_popup_cursor {
            Some((x, y))
        } else if petri_visible || (state.file_tree.open && state.focus == PaneId::Editor) {
            None
        } else if state.focus == PaneId::Editor {
            editor_cursor.map(|pos| (pos.x, pos.y))
        } else if state.focus == PaneId::JobOutput
            && state.agents.dock_tab == AgentOpsTab::Scratchpad
        {
            job_cursor.map(|pos| (pos.x, pos.y))
        } else {
            notes_cursor.map(|pos| (pos.x, pos.y))
        };
        if let Some((x, y)) = cursor {
            f.set_cursor(x, y);
        }
    })?;
    let cursor_style = if state.agents.artifacts_popup_open || state.focus == PaneId::Notes {
        SetCursorStyle::SteadyBar
    } else {
        match state.mode {
            Mode::Insert => SetCursorStyle::SteadyBar,
            Mode::Normal | Mode::Visual => SetCursorStyle::SteadyBlock,
        }
    };
    execute!(terminal.backend_mut(), cursor_style)?;
    state.metrics.last_render_ms = start.elapsed().as_millis();
    state.metrics.frame_count += 1;
    Ok(())
}

fn handle_editor_buffer_shortcuts(
    key: KeyEvent,
    state: &mut AppState,
    syntax: &mut SyntaxRuntime,
    clipboard: &mut Option<Clipboard>,
) -> bool {
    if state.command_line.is_some()
        || state.prompt.is_some()
        || state.rule_picker.open
        || state.protocol_picker.open
        || state.show_help
        || games_modal_popup_open(state)
    {
        return false;
    }
    if !pane_accepts_text_input(state, state.focus) {
        return false;
    }
    if state.file_tree.open && state.focus == PaneId::Editor {
        return false;
    }
    let (buffer_id, is_editor) = match state.focus {
        PaneId::Editor => (state.active_editor_buffer_id, true),
        PaneId::JobOutput if state.agents.dock_tab == AgentOpsTab::Scratchpad => {
            (state.notes_buffer_id, false)
        }
        _ => return false,
    };

    let select_all = matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('a') | KeyCode::Char('A'),
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('\u{1}'),
            modifiers: KeyModifiers::NONE,
            ..
        }
    );
    if select_all {
        state.mode = Mode::Insert;
        if is_editor {
            let buf = state.editor_buffer_mut();
            buf.go_to_top();
            buf.move_home();
            buf.set_selection_anchor();
            buf.go_to_bottom();
            buf.move_end();
            buf.ensure_visible();
        } else {
            let buf = state.notes_buffer_mut();
            buf.go_to_top();
            buf.move_home();
            buf.set_selection_anchor();
            buf.go_to_bottom();
            buf.move_end();
            buf.ensure_visible();
        }
        return true;
    }

    let copy = matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('c') | KeyCode::Char('C'),
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('\u{3}'),
            modifiers: KeyModifiers::NONE,
            ..
        }
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Insert,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    );
    if copy {
        let text = if is_editor {
            state.editor_buffer().yank_selection()
        } else {
            state.notes_buffer().yank_selection()
        };
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            state.yank_kind = if text.contains('\n') {
                YankKind::Line
            } else {
                YankKind::Char
            };
            state.yank = Some(text.clone());
            if let Some(cb) = clipboard.as_mut() {
                let _ = cb.set_text(text);
            }
        }
        return true;
    }

    let cut = matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('x') | KeyCode::Char('X'),
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('\u{18}'),
            modifiers: KeyModifiers::NONE,
            ..
        }
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Delete,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SHIFT)
    );
    if cut {
        if matches!(key.code, KeyCode::Delete) {
            let has_selection = if is_editor {
                state.editor_buffer().selection_range().is_some()
            } else {
                state.notes_buffer().selection_range().is_some()
            };
            if !has_selection {
                return false;
            }
        }

        let (selection_text, changed) = if is_editor {
            let buf = state.editor_buffer_mut();
            let before_version = buf.version();
            let selection_text = buf.yank_selection().filter(|t| !t.is_empty());
            let mut changed = false;
            if selection_text.is_some() {
                changed = buf.delete_selection();
                if changed {
                    buf.ensure_visible();
                }
            }
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
            (selection_text, changed)
        } else {
            let buf = state.notes_buffer_mut();
            let before_version = buf.version();
            let selection_text = buf.yank_selection().filter(|t| !t.is_empty());
            let mut changed = false;
            if selection_text.is_some() {
                changed = buf.delete_selection();
                if changed {
                    buf.ensure_visible();
                }
            }
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
            (selection_text, changed)
        };

        if let Some(text) = selection_text {
            state.yank_kind = if text.contains('\n') {
                YankKind::Line
            } else {
                YankKind::Char
            };
            state.yank = Some(text.clone());
            if let Some(cb) = clipboard.as_mut() {
                let _ = cb.set_text(text);
            }
            if changed {
                state.mode = Mode::Insert;
            }
        }
        return true;
    }

    let paste = matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('v') | KeyCode::Char('V'),
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('\u{16}'),
            modifiers: KeyModifiers::NONE,
            ..
        }
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Insert,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SHIFT)
    );
    if paste {
        state.mode = Mode::Insert;
        let Some(cb) = clipboard.as_mut() else {
            return true;
        };
        let Ok(text) = cb.get_text() else {
            return true;
        };
        let normalized = normalize_buffer_input_text(&text);
        if normalized.is_empty() {
            return true;
        }
        if is_editor {
            let buf = state.editor_buffer_mut();
            let before_version = buf.version();
            buf.break_undo_group();
            buf.insert_str(normalized.as_ref());
            buf.ensure_visible();
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
        } else {
            let buf = state.notes_buffer_mut();
            let before_version = buf.version();
            buf.break_undo_group();
            buf.insert_str(normalized.as_ref());
            buf.ensure_visible();
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
        }
        return true;
    }

    if matches!(key.code, KeyCode::Backspace | KeyCode::Delete) {
        let has_selection = if is_editor {
            state.editor_buffer().selection_range().is_some()
        } else {
            state.notes_buffer().selection_range().is_some()
        };
        if has_selection {
            state.mode = Mode::Insert;
            if is_editor {
                let buf = state.editor_buffer_mut();
                let before_version = buf.version();
                let _ = buf.delete_selection();
                buf.ensure_visible();
                if buf.version() != before_version {
                    syntax.note_buffer_change(buffer_id, buf);
                }
            } else {
                let buf = state.notes_buffer_mut();
                let before_version = buf.version();
                let _ = buf.delete_selection();
                buf.ensure_visible();
                if buf.version() != before_version {
                    syntax.note_buffer_change(buffer_id, buf);
                }
            }
            return true;
        }
    }

    let word_left = matches!(
        key,
        KeyEvent {
            code: KeyCode::Left,
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    );
    if word_left {
        if is_editor {
            let buf = state.editor_buffer_mut();
            buf.move_word_back();
            buf.ensure_visible();
        } else {
            let buf = state.notes_buffer_mut();
            buf.move_word_back();
            buf.ensure_visible();
        }
        return true;
    }

    let word_right = matches!(
        key,
        KeyEvent {
            code: KeyCode::Right,
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    );
    if word_right {
        if is_editor {
            let buf = state.editor_buffer_mut();
            buf.move_word_end();
            buf.ensure_visible();
        } else {
            let buf = state.notes_buffer_mut();
            buf.move_word_end();
            buf.ensure_visible();
        }
        return true;
    }

    let word_backspace = matches!(
        key,
        KeyEvent {
            code: KeyCode::Backspace,
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    );
    if word_backspace {
        state.mode = Mode::Insert;
        if is_editor {
            let buf = state.editor_buffer_mut();
            let before_version = buf.version();
            buf.delete_word_back();
            buf.ensure_visible();
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
        } else {
            let buf = state.notes_buffer_mut();
            let before_version = buf.version();
            buf.delete_word_back();
            buf.ensure_visible();
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
        }
        return true;
    }

    let word_delete = matches!(
        key,
        KeyEvent {
            code: KeyCode::Delete,
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    );
    if word_delete {
        state.mode = Mode::Insert;
        if is_editor {
            let buf = state.editor_buffer_mut();
            let before_version = buf.version();
            buf.delete_word_forward();
            buf.ensure_visible();
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
        } else {
            let buf = state.notes_buffer_mut();
            let before_version = buf.version();
            buf.delete_word_forward();
            buf.ensure_visible();
            if buf.version() != before_version {
                syntax.note_buffer_change(buffer_id, buf);
            }
        }
        return true;
    }

    false
}

fn handle_agent_station_key_with_clipboard(
    key: KeyEvent,
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    swarm: &mut SwarmRuntime,
    clipboard: &mut Option<Clipboard>,
) -> bool {
    if let Some(target) = map_focus_hotkey(&key) {
        state.focus = target;
        if target == PaneId::JobOutput && state.agents.dock_tab == AgentOpsTab::Scratchpad {
            state.mode = Mode::Insert;
        } else if target != PaneId::Editor {
            state.mode = Mode::Normal;
        }
        return true;
    }
    if state.command_line.is_some()
        || state.prompt.is_some()
        || state.show_help
        || state.rule_picker.open
        || state.protocol_picker.open
        || state.fuzzy_search.open
        || games_modal_popup_open(state)
    {
        return false;
    }

    match state.focus {
        PaneId::JobOutput => handle_agent_ops_key(key, state, vitals, codex, claude, swarm),
        PaneId::Notes => handle_agent_console_key(key, state, vitals, codex, claude, swarm, clipboard),
        _ => false,
    }
}

fn map_focus_hotkey(key: &KeyEvent) -> Option<PaneId> {
    match key {
        KeyEvent {
            code: KeyCode::Char('1'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => Some(PaneId::Editor),
        KeyEvent {
            code: KeyCode::Char('2'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => Some(PaneId::JobOutput),
        KeyEvent {
            code: KeyCode::Char('3'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => Some(PaneId::Notes),
        _ => None,
    }
}

fn handle_agent_ops_key(
    key: KeyEvent,
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    _claude: Option<&ClaudeRunner>,
    swarm: &SwarmRuntime,
) -> bool {
    if state.agents.dock_tab == AgentOpsTab::Scratchpad {
        match key {
            KeyEvent {
                code: KeyCode::Tab,
                modifiers: KeyModifiers::SHIFT,
                ..
            } => {
                state.agents.dock_tab = state.agents.dock_tab.prev();
                state.agents.roster_tree_selected = None;
                state.agents.ops_scroll = 0;
                state.mode = if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                    Mode::Insert
                } else {
                    Mode::Normal
                };
                state.agents.note_event();
                vitals.record_agent_event(Instant::now());
                return true;
            }
            KeyEvent {
                code: KeyCode::Tab, ..
            } => {
                state.agents.dock_tab = state.agents.dock_tab.next();
                state.agents.roster_tree_selected = None;
                state.agents.ops_scroll = 0;
                state.mode = if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                    Mode::Insert
                } else {
                    Mode::Normal
                };
                state.agents.note_event();
                vitals.record_agent_event(Instant::now());
                return true;
            }
            KeyEvent {
                code: KeyCode::Left,
                ..
            } if state.mode != Mode::Insert => {
                state.agents.dock_tab = state.agents.dock_tab.prev();
                state.agents.roster_tree_selected = None;
                state.agents.ops_scroll = 0;
                state.mode = if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                    Mode::Insert
                } else {
                    Mode::Normal
                };
                state.agents.note_event();
                vitals.record_agent_event(Instant::now());
                return true;
            }
            KeyEvent {
                code: KeyCode::Right,
                ..
            } if state.mode != Mode::Insert => {
                state.agents.dock_tab = state.agents.dock_tab.next();
                state.agents.roster_tree_selected = None;
                state.agents.ops_scroll = 0;
                state.mode = if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                    Mode::Insert
                } else {
                    Mode::Normal
                };
                state.agents.note_event();
                vitals.record_agent_event(Instant::now());
                return true;
            }
            KeyEvent {
                code: KeyCode::Char(_),
                modifiers,
                ..
            } if state.mode != Mode::Insert
                && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
            {
                // In Scratchpad, treat first printable key as intent to type.
                state.mode = Mode::Insert;
                return false;
            }
            KeyEvent {
                code: KeyCode::Enter | KeyCode::Backspace | KeyCode::Delete,
                modifiers,
                ..
            } if state.mode != Mode::Insert && modifiers.is_empty() => {
                state.mode = Mode::Insert;
                return false;
            }
            KeyEvent {
                code: KeyCode::Esc, ..
            } if state.mode == Mode::Insert => {
                state.mode = Mode::Normal;
                return true;
            }
            _ => return false,
        }
    }

    let mut changed = false;
    match key {
        KeyEvent {
            code: KeyCode::Char('l'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if let Some(agent_ops_view::RosterSelectableRow::Backend { backend }) =
                agent_ops_view::roster_selected_row(state)
            {
                select_roster_backend(state, backend);
                changed = if !state
                    .agents
                    .roster_expanded_backend_kinds
                    .contains(&backend)
                {
                    toggle_roster_backend_expanded(state, backend)
                } else {
                    false
                };
            } else {
                changed = enter_roster_tree_cursor(state);
            }
        }
        KeyEvent {
            code: KeyCode::Char('h'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if let Some(agent_ops_view::RosterSelectableRow::Backend { backend }) =
                agent_ops_view::roster_selected_row(state)
            {
                select_roster_backend(state, backend);
                changed = if state
                    .agents
                    .roster_expanded_backend_kinds
                    .contains(&backend)
                {
                    toggle_roster_backend_expanded(state, backend)
                } else {
                    false
                };
            } else {
                changed = exit_roster_tree_cursor(state);
            }
        }
        KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            changed = reset_roster_context(state, swarm);
        }
        KeyEvent {
            code: KeyCode::Char('1'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if !state
                .agents
                .swarm_default_template
                .eq_ignore_ascii_case("lab")
            {
                state.agents.swarm_default_template = "lab".into();
                state.agents.roster_tree_selected = None;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('2'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if !state
                .agents
                .swarm_default_template
                .eq_ignore_ascii_case("parallel")
            {
                state.agents.swarm_default_template = "parallel".into();
                state.agents.roster_tree_selected = None;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('3'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if !state
                .agents
                .swarm_default_template
                .eq_ignore_ascii_case("bulk")
            {
                state.agents.swarm_default_template = "bulk".into();
                state.agents.roster_tree_selected = None;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('4'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if !state
                .agents
                .swarm_default_mission
                .eq_ignore_ascii_case("auto")
            {
                state.agents.swarm_default_mission = "auto".into();
                state.agents.roster_tree_selected = None;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('5'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if !state
                .agents
                .swarm_default_mission
                .eq_ignore_ascii_case("general")
            {
                state.agents.swarm_default_mission = "general".into();
                state.agents.roster_tree_selected = None;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('6'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if !state
                .agents
                .swarm_default_mission
                .eq_ignore_ascii_case("research")
            {
                state.agents.swarm_default_mission = "research".into();
                state.agents.roster_tree_selected = None;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('7'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if !state
                .agents
                .swarm_default_mission
                .eq_ignore_ascii_case("computational-research")
            {
                state.agents.swarm_default_mission = "computational-research".into();
                state.agents.roster_tree_selected = None;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::SHIFT,
            ..
        } => {
            state.agents.dock_tab = state.agents.dock_tab.prev();
            state.agents.roster_tree_selected = None;
            state.agents.ops_scroll = 0;
            if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                state.mode = Mode::Insert;
            }
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Tab, ..
        } => {
            state.agents.dock_tab = state.agents.dock_tab.next();
            state.agents.roster_tree_selected = None;
            state.agents.ops_scroll = 0;
            if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                state.mode = Mode::Insert;
            }
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Left,
            ..
        } => {
            state.agents.dock_tab = state.agents.dock_tab.prev();
            state.agents.roster_tree_selected = None;
            state.agents.ops_scroll = 0;
            if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                state.mode = Mode::Insert;
            }
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Right,
            ..
        } => {
            state.agents.dock_tab = state.agents.dock_tab.next();
            state.agents.roster_tree_selected = None;
            state.agents.ops_scroll = 0;
            if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                state.mode = Mode::Insert;
            }
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Up, ..
        }
        | KeyEvent {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            changed = move_agent_ops_selection(state, swarm, -1);
        }
        KeyEvent {
            code: KeyCode::Down,
            ..
        }
        | KeyEvent {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            changed = move_agent_ops_selection(state, swarm, 1);
        }
        KeyEvent {
            code: KeyCode::Char(' '),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster
            && state.agents.roster_tree_selected.is_none() =>
        {
            changed = toggle_roster_priority(state);
        }
        KeyEvent {
            code: KeyCode::Char(' '),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster
            && state.agents.roster_tree_selected.is_some() =>
        {
            changed = select_roster_tree_leaf(state);
        }
        KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster
            && state.agents.roster_tree_selected.is_some() =>
        {
            changed = select_roster_tree_leaf(state);
        }
        KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if let Some(agent_ops_view::RosterSelectableRow::Backend { backend }) =
                agent_ops_view::roster_selected_row(state)
            {
                select_roster_backend(state, backend);
                changed = toggle_roster_backend_expanded(state, backend);
            } else {
                state.focus = PaneId::Notes;
                state.mode = Mode::Normal;
                state.agents.console_scroll = CONSOLE_SCROLL_BOTTOM;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Evidence => {
            // Check if the selected card is a PROMPT — toggle collapse.
            let selected_card_kind = {
                let text_width = state.agents.ops_viewport_width.max(32);
                let widths = agent_ops_view::artifact_list_widths(text_width);
                let preview_chars = widths
                    .get(3)
                    .copied()
                    .unwrap_or(120)
                    .saturating_sub(1)
                    .max(10);
                let cards =
                    agent_ops_view::artifact_cards_for_context(state, Some(swarm), preview_chars);
                let sel = state
                    .agents
                    .artifacts_selected
                    .min(cards.len().saturating_sub(1));
                cards.get(sel).map(|c| c.kind.to_string())
            };
            if selected_card_kind.as_deref() == Some("PROMPT") {
                let idx = state.agents.artifacts_selected;
                if !state.agents.artifacts_collapsed_prompts.remove(&idx) {
                    state.agents.artifacts_collapsed_prompts.insert(idx);
                }
            } else {
                state.agents.artifacts_popup_open = true;
                state.agents.artifacts_popup_scroll = 0;
            }
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Char('r') | KeyCode::Char('R'),
            modifiers,
            ..
        } if modifiers.is_empty() && state.agents.dock_tab == AgentOpsTab::Evidence => {
            state.agents.artifacts_history_popup_open = true;
            state.agents.artifacts_history_selected =
                agent_ops_view::artifacts_selected_visible_history_entry(state);
            state.agents.artifacts_history_popup_scroll = 0;
            state.agents.artifacts_history_pending_action = None;
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Enter,
            ..
        } => {
            state.focus = PaneId::Notes;
            state.mode = Mode::Normal;
            state.agents.console_scroll = CONSOLE_SCROLL_BOTTOM;
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Char('n'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            spawn_mock_mission(state);
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Char('r'),
            modifiers,
            ..
        } if modifiers.is_empty() && state.agents.dock_tab == AgentOpsTab::Mcp => {
            if let Some(codex) = codex {
                state.status =
                    Some("MCP reconnect: cancelling in-flight turns (context preserved)".into());
                codex.send(CodexCommand::McpReconnect);
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('s'),
            modifiers,
            ..
        } if modifiers.is_empty() && state.agents.dock_tab == AgentOpsTab::Mcp => {
            if let Some(codex) = codex {
                codex.send(CodexCommand::McpStart);
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('x'),
            modifiers,
            ..
        } if modifiers.is_empty() && state.agents.dock_tab == AgentOpsTab::Mcp => {
            if let Some(codex) = codex {
                reset_codex_mcp_sessions(state, "MCP stop clears Codex thread context");
                codex.send(CodexCommand::McpStop);
                changed = true;
            }
        }
        _ => {}
    }
    if changed {
        state.agents.note_event();
        vitals.record_agent_event(Instant::now());
    }
    changed
}

fn reset_codex_mcp_sessions(state: &mut AppState, status: &str) {
    state.agents.codex_thread_ids.clear();
    state.agents.codex_mission_thread_ids.clear();
    state.agents.codex_used_tokens.clear();
    state.agents.codex_mission_used_tokens.clear();
    state.agents.codex_context_remaining_pct.clear();
    state.agents.codex_mission_context_remaining_pct.clear();
    state.agents.codex_estimated_tokens_used_by_mission.clear();
    state.status = Some(status.to_string());
}

fn sync_roster_selected_agent(state: &mut AppState, agent_idx: usize) {
    state.agents.roster_selected = agent_idx;
    state.agents.roster_selected_backend = None;
    state.agents.roster_tree_selected = None;
    if let Some(agent) = state.agents.agents.get(agent_idx) {
        state.agents.selected_agent = Some(agent.id.clone());
        if let Some(mission_id) = agent.current_mission.as_deref() {
            state.agents.selected_mission = Some(mission_id.to_string());
            if let Some(idx) = state
                .agents
                .missions
                .iter()
                .position(|mission| mission.id == mission_id)
            {
                state.agents.mission_selected = idx;
            }
        }
    }
}

fn select_roster_backend(state: &mut AppState, backend: nit_core::AgentLaneKind) {
    state.agents.roster_selected_backend = Some(backend);
    state.agents.roster_tree_selected = None;
}

fn toggle_roster_backend_expanded(state: &mut AppState, backend: nit_core::AgentLaneKind) -> bool {
    let keep_backend_selected = state.agents.roster_selected_backend == Some(backend);
    if state.agents.roster_expanded_backend_kinds.remove(&backend) {
        if keep_backend_selected {
            state.agents.roster_tree_selected = None;
            return true;
        }
        let visible = agent_ops_view::roster_agent_display_order(state);
        if !visible.contains(&state.agents.roster_selected) {
            state.agents.roster_tree_selected = None;
            if let Some(&first_visible) = visible.first() {
                sync_roster_selected_agent(state, first_visible);
            }
        }
        return true;
    }

    if !state.agents.roster_expanded_backend_kinds.insert(backend) {
        return false;
    }
    if keep_backend_selected {
        state.agents.roster_tree_selected = None;
        return true;
    }
    if let Some(first_agent_idx) =
        agent_ops_view::roster_first_agent_idx_for_backend(state, backend)
    {
        sync_roster_selected_agent(state, first_agent_idx);
    }
    true
}

fn roster_selected_agent_is_visible(state: &AppState) -> bool {
    agent_ops_view::roster_agent_display_order(state).contains(&state.agents.roster_selected)
}

fn move_roster_primary_selection(state: &mut AppState, delta: i32) -> bool {
    let order = agent_ops_view::roster_selection_rows(state);
    if order.is_empty() {
        return false;
    }

    let current = agent_ops_view::roster_selected_row(state).unwrap_or(order[0]);
    let cur_pos = order.iter().position(|row| *row == current).unwrap_or(0);
    let next_pos = (cur_pos as i32 + delta).clamp(0, order.len().saturating_sub(1) as i32) as usize;
    if next_pos == cur_pos {
        return false;
    }

    match order[next_pos] {
        agent_ops_view::RosterSelectableRow::Backend { backend } => {
            select_roster_backend(state, backend);
        }
        agent_ops_view::RosterSelectableRow::Agent { agent_idx } => {
            sync_roster_selected_agent(state, agent_idx);
        }
    }
    true
}

fn move_agent_ops_selection(state: &mut AppState, swarm: &SwarmRuntime, delta: i32) -> bool {
    match state.agents.dock_tab {
        AgentOpsTab::Roster => {
            if state.agents.agents.is_empty() {
                return false;
            }
            if let Some(sel) = state.agents.roster_tree_selected {
                let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
                    state.agents.roster_tree_selected = None;
                    return true;
                };
                let show_roles = state
                    .agents
                    .swarm_default_template
                    .eq_ignore_ascii_case("bulk")
                    || state
                        .agents
                        .swarm_default_template
                        .eq_ignore_ascii_case("parallel");
                let efforts = state
                    .agents
                    .codex_supported_reasoning_efforts
                    .get(&agent.id)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let size_len = efforts.len();
                let has_roles = show_roles && agent.is_codex();
                let roles_len = if has_roles { 8usize } else { 0usize };

                match sel.branch {
                    nit_core::RosterTreeBranch::Size => {
                        if size_len == 0 {
                            state.agents.roster_tree_selected = None;
                            return true;
                        }
                        let max = size_len.saturating_sub(1);
                        if delta.is_negative() {
                            if sel.leaf_idx > 0 {
                                state.agents.roster_tree_selected =
                                    Some(nit_core::RosterTreeSelection {
                                        branch: nit_core::RosterTreeBranch::Size,
                                        leaf_idx: sel.leaf_idx.saturating_sub(1),
                                    });
                                return true;
                            }
                            state.agents.roster_tree_selected = None;
                            return true;
                        }
                        if delta > 0 {
                            if sel.leaf_idx < max {
                                state.agents.roster_tree_selected =
                                    Some(nit_core::RosterTreeSelection {
                                        branch: nit_core::RosterTreeBranch::Size,
                                        leaf_idx: (sel.leaf_idx + 1).min(max),
                                    });
                                return true;
                            }

                            if roles_len > 0 {
                                state.agents.roster_tree_selected =
                                    Some(nit_core::RosterTreeSelection {
                                        branch: nit_core::RosterTreeBranch::Role,
                                        leaf_idx: 0,
                                    });
                                return true;
                            }
                        }
                    }
                    nit_core::RosterTreeBranch::Role => {
                        if roles_len == 0 {
                            state.agents.roster_tree_selected = None;
                            return true;
                        }
                        let max = roles_len.saturating_sub(1);
                        if delta.is_negative() {
                            if sel.leaf_idx > 0 {
                                state.agents.roster_tree_selected =
                                    Some(nit_core::RosterTreeSelection {
                                        branch: nit_core::RosterTreeBranch::Role,
                                        leaf_idx: sel.leaf_idx.saturating_sub(1),
                                    });
                                return true;
                            }
                            if size_len > 0 {
                                state.agents.roster_tree_selected =
                                    Some(nit_core::RosterTreeSelection {
                                        branch: nit_core::RosterTreeBranch::Size,
                                        leaf_idx: size_len.saturating_sub(1),
                                    });
                                return true;
                            }
                            state.agents.roster_tree_selected = None;
                            return true;
                        }
                        if delta > 0 && sel.leaf_idx < max {
                            state.agents.roster_tree_selected =
                                Some(nit_core::RosterTreeSelection {
                                    branch: nit_core::RosterTreeBranch::Role,
                                    leaf_idx: (sel.leaf_idx + 1).min(max),
                                });
                            return true;
                        }
                    }
                }

                // Walk out of the tree when we hit the end and press Down.
                if delta > 0 {
                    state.agents.roster_tree_selected = None;
                    return move_roster_primary_selection(state, 1);
                }
                return false;
            }

            move_roster_primary_selection(state, delta)
        }
        AgentOpsTab::Missions => {
            if state.agents.missions.is_empty() {
                return false;
            }
            let max = state.agents.missions.len().saturating_sub(1) as i32;
            let next = (state.agents.mission_selected as i32 + delta).clamp(0, max) as usize;
            if next == state.agents.mission_selected {
                return false;
            }
            state.agents.mission_selected = next;
            if let Some(mission) = state.agents.missions.get(next) {
                state.agents.selected_mission = Some(mission.id.clone());
            }
            true
        }
        AgentOpsTab::Evidence => {
            let text_width = state.agents.ops_viewport_width.max(32).max(1);
            let lines =
                agent_ops_view::current_lines_for_width_with_swarm(state, Some(swarm), text_width);
            let count = agent_ops_view::artifacts_card_count(&lines);
            if count == 0 {
                return false;
            }
            let max = count.saturating_sub(1) as i32;
            let next = (state.agents.artifacts_selected as i32 + delta).clamp(0, max) as usize;
            if next == state.agents.artifacts_selected {
                return false;
            }
            state.agents.artifacts_selected = next;

            if let Some(line_idx) = agent_ops_view::artifacts_card_line_for_index(&lines, next) {
                let height = state.agents.ops_viewport_height.max(1);
                if line_idx < state.agents.ops_scroll {
                    state.agents.ops_scroll = line_idx;
                } else if line_idx >= state.agents.ops_scroll.saturating_add(height) {
                    state.agents.ops_scroll = line_idx.saturating_sub(height.saturating_sub(1));
                }
            }
            true
        }
        AgentOpsTab::Alerts => {
            if state.agents.alerts.is_empty() {
                return false;
            }
            let max = state.agents.alerts.len().saturating_sub(1) as i32;
            let next = (state.agents.alert_selected as i32 + delta).clamp(0, max) as usize;
            if next == state.agents.alert_selected {
                return false;
            }
            state.agents.alert_selected = next;
            true
        }
        AgentOpsTab::Patch
        | AgentOpsTab::Diagnostics
        | AgentOpsTab::Dag
        | AgentOpsTab::Scratchpad => {
            if delta.is_negative() {
                state.agents.ops_scroll = state.agents.ops_scroll.saturating_sub(1);
            } else if delta > 0 {
                state.agents.ops_scroll = state.agents.ops_scroll.saturating_add(1);
            }
            true
        }
        AgentOpsTab::Mcp => false,
    }
}

const SWARM_ROLE_OPTIONS: [&str; 8] = [
    "all",
    "propose",
    "research",
    "computational-research",
    "judge",
    "integrate",
    "review",
    "test",
];

fn normalize_swarm_role_hint_for_roster(raw: &str) -> String {
    let role = raw.trim();
    if role.eq_ignore_ascii_case("all") {
        return "all".into();
    }
    normalize_role_label(role).unwrap_or_else(|| role.to_ascii_lowercase())
}

fn enter_roster_tree_cursor(state: &mut AppState) -> bool {
    if state.agents.roster_tree_selected.is_some() {
        return false;
    }
    if !roster_selected_agent_is_visible(state) {
        return false;
    }
    let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
        state.agents.roster_tree_selected = None;
        return false;
    };

    let show_roles = state
        .agents
        .swarm_default_template
        .eq_ignore_ascii_case("bulk")
        || state
            .agents
            .swarm_default_template
            .eq_ignore_ascii_case("parallel");

    let efforts = state
        .agents
        .codex_supported_reasoning_efforts
        .get(&agent.id)
        .or_else(|| state.agents.claude_supported_efforts.get(&agent.id))
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let has_size = !efforts.is_empty();
    let has_roles = show_roles && (agent.is_codex() || agent.is_claude());
    if !has_size && !has_roles {
        state.agents.roster_tree_selected = None;
        return false;
    }
    state
        .agents
        .roster_tree_collapsed_agent_ids
        .remove(&agent.id);
    if has_size {
        let current = state
            .agents
            .codex_selected_reasoning_effort
            .get(&agent.id)
            .or_else(|| state.agents.codex_default_reasoning_effort.get(&agent.id))
            .or_else(|| state.agents.claude_selected_effort.get(&agent.id))
            .or_else(|| state.agents.claude_default_effort.get(&agent.id))
            .map(|s| s.as_str());
        let idx = current
            .and_then(|effort| efforts.iter().position(|e| e == effort))
            .unwrap_or(0)
            .min(efforts.len().saturating_sub(1));
        state.agents.roster_tree_selected = Some(nit_core::RosterTreeSelection {
            branch: nit_core::RosterTreeBranch::Size,
            leaf_idx: idx,
        });
        return true;
    }

    if has_roles {
        let current = state
            .agents
            .swarm_role_by_agent_id
            .get(&agent.id)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        let idx = current
            .and_then(|role| {
                let current = normalize_swarm_role_hint_for_roster(role);
                SWARM_ROLE_OPTIONS.iter().position(|candidate| {
                    current == normalize_swarm_role_hint_for_roster(candidate)
                })
            })
            .unwrap_or(0)
            .min(SWARM_ROLE_OPTIONS.len().saturating_sub(1));
        state.agents.roster_tree_selected = Some(nit_core::RosterTreeSelection {
            branch: nit_core::RosterTreeBranch::Role,
            leaf_idx: idx,
        });
        return true;
    }

    state.agents.roster_tree_selected = None;
    false
}

fn exit_roster_tree_cursor(state: &mut AppState) -> bool {
    if state.agents.roster_tree_selected.is_some() {
        state.agents.roster_tree_selected = None;
        return true;
    }
    if !roster_selected_agent_is_visible(state) {
        return false;
    }
    let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
        return false;
    };
    if state
        .agents
        .roster_tree_collapsed_agent_ids
        .insert(agent.id.clone())
    {
        return true;
    }
    false
}

fn select_roster_tree_leaf(state: &mut AppState) -> bool {
    let Some(sel) = state.agents.roster_tree_selected else {
        return false;
    };
    let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
        return false;
    };

    match sel.branch {
        nit_core::RosterTreeBranch::Size => {
            let efforts = state
                .agents
                .codex_supported_reasoning_efforts
                .get(&agent.id)
                .or_else(|| state.agents.claude_supported_efforts.get(&agent.id));
            let Some(efforts) = efforts else {
                return false;
            };
            let Some(effort) = efforts.get(sel.leaf_idx) else {
                return false;
            };

            let effort = effort.trim();
            if effort.is_empty() {
                return false;
            }

            let is_claude = agent.is_claude();
            if is_claude {
                let current = state
                    .agents
                    .claude_selected_effort
                    .get(&agent.id)
                    .map(|s| s.as_str());
                if current == Some(effort) {
                    return false;
                }
                state
                    .agents
                    .claude_selected_effort
                    .insert(agent.id.clone(), effort.to_string());
            } else {
                let current = state
                    .agents
                    .codex_selected_reasoning_effort
                    .get(&agent.id)
                    .map(|s| s.as_str());
                if current == Some(effort) {
                    return false;
                }
                state
                    .agents
                    .codex_selected_reasoning_effort
                    .insert(agent.id.clone(), effort.to_string());
            }
            true
        }
        nit_core::RosterTreeBranch::Role => {
            let Some(role) = SWARM_ROLE_OPTIONS.get(sel.leaf_idx).copied() else {
                return false;
            };

            if role.eq_ignore_ascii_case("all") {
                let current = state
                    .agents
                    .swarm_role_by_agent_id
                    .get(&agent.id)
                    .map(|s| s.as_str());
                if current.is_some_and(|cur| cur.trim().eq_ignore_ascii_case("all")) {
                    return false;
                }
                state
                    .agents
                    .swarm_role_by_agent_id
                    .insert(agent.id.clone(), "all".to_string());
                return true;
            }

            let current = state
                .agents
                .swarm_role_by_agent_id
                .get(&agent.id)
                .map(|s| s.as_str());
            if current.is_some_and(|cur| {
                normalize_swarm_role_hint_for_roster(cur)
                    == normalize_swarm_role_hint_for_roster(role)
            }) {
                return false;
            }
            state
                .agents
                .swarm_role_by_agent_id
                .insert(agent.id.clone(), role.to_string());
            true
        }
    }
}

fn toggle_roster_priority(state: &mut AppState) -> bool {
    if !roster_selected_agent_is_visible(state) {
        return false;
    }
    let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
        return false;
    };
    if !agent.supports_swarm_priority() {
        return false;
    }
    if agent.id.contains("#swarm-") {
        return false;
    }
    if state.agents.swarm_priority_agent_ids.remove(&agent.id) {
        return true;
    }
    state
        .agents
        .swarm_priority_agent_ids
        .insert(agent.id.clone())
}

fn reset_roster_context(state: &mut AppState, swarm: &SwarmRuntime) -> bool {
    let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
        return false;
    };
    let agent_id = agent.id.clone();
    let agent_label = agent.role.trim().to_string();
    let is_codex = agent.is_codex();
    let mission_ctx = state
        .agents
        .selected_context_mission()
        .map(ToString::to_string);

    state.agents.roster_tree_selected = None;
    // Clear any in-flight liveness tracking for this agent context.
    state.agents.active_turns.remove(&agent_id);
    if is_codex {
        // Reset back to "full context" for display purposes.
        state
            .agents
            .codex_context_remaining_pct
            .insert(agent_id.clone(), 100);
    } else {
        state.agents.codex_context_remaining_pct.remove(&agent_id);
    }

    let before = state.agents.messages.len();
    if let Some(mission_id) = mission_ctx.as_deref() {
        if let Err(err) = write_agent_run_provenance(state, swarm, mission_id) {
            state.agents.diag_events.push(AgentDiagnosticEvent {
                severity: AgentAlertSeverity::Warn,
                source: "artifacts".into(),
                message: format!(
                    "failed to persist mission artifacts before reset for {mission_id}: {err}"
                ),
                at: timestamp_label(state),
            });
        } else {
            let run_dir = state
                .workspace_root
                .join(".nit")
                .join("agents")
                .join("runs")
                .join(mission_id);
            if let Err(err) = archive_saved_run_snapshot(&run_dir) {
                state.agents.diag_events.push(AgentDiagnosticEvent {
                    severity: AgentAlertSeverity::Warn,
                    source: "artifacts".into(),
                    message: format!("failed to archive saved mission run for {mission_id}: {err}"),
                    at: timestamp_label(state),
                });
            }
            state
                .agents
                .pending_provenance_mission_ids
                .retain(|id| id != mission_id);
        }
        // In mission context, the Codex session is shared by mission id. Resetting context should
        // clear the mission transcript and forget the session id so the next prompt starts fresh.
        state.agents.codex_mission_thread_ids.remove(mission_id);
        state
            .agents
            .codex_mission_context_remaining_pct
            .remove(mission_id);
        state.agents.codex_mission_used_tokens.remove(mission_id);
        state
            .agents
            .codex_estimated_tokens_used_by_mission
            .remove(mission_id);
        state
            .agents
            .messages
            .retain(|msg| msg.mission_id.as_deref() != Some(mission_id));
    } else {
        if let Err(err) = write_ad_hoc_run_provenance(state, &agent_id) {
            state.agents.diag_events.push(AgentDiagnosticEvent {
                severity: AgentAlertSeverity::Warn,
                source: "artifacts".into(),
                message: format!(
                    "failed to persist ad-hoc artifacts before reset for {agent_id}: {err}"
                ),
                at: timestamp_label(state),
            });
        } else {
            let run_dir = state
                .workspace_root
                .join(".nit")
                .join("agents")
                .join("ad-hoc")
                .join(sanitize_for_filename(&agent_id));
            if let Err(err) = archive_saved_run_snapshot(&run_dir) {
                state.agents.diag_events.push(AgentDiagnosticEvent {
                    severity: AgentAlertSeverity::Warn,
                    source: "artifacts".into(),
                    message: format!("failed to archive saved ad-hoc run for {agent_id}: {err}"),
                    at: timestamp_label(state),
                });
            }
            state
                .agents
                .pending_provenance_agent_ids
                .retain(|id| id != &agent_id);
        }
        // In non-mission chat, the thread isn't partitioned by agent; reset the whole local thread.
        state.agents.codex_thread_ids.clear();
        state.agents.codex_used_tokens.clear();
        state.agents.messages.retain(|msg| msg.mission_id.is_some());
    }

    // If there are queued Codex turns for the context we're resetting, drop them (they would run
    // against a now-forgotten thread id). Keep each agent's `queue_len` consistent with removals.
    let mut removed_by_agent: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    if let Some(mission_id) = mission_ctx.as_deref() {
        state.agents.queued_codex_turns.retain(|turn| {
            if turn.mission_id.as_deref() == Some(mission_id) {
                *removed_by_agent.entry(turn.agent_id.clone()).or_insert(0) += 1;
                false
            } else {
                true
            }
        });
    } else {
        state.agents.queued_codex_turns.retain(|turn| {
            if turn.mission_id.is_none() {
                *removed_by_agent.entry(turn.agent_id.clone()).or_insert(0) += 1;
                false
            } else {
                true
            }
        });
    }
    if !removed_by_agent.is_empty() {
        for agent in state.agents.agents.iter_mut() {
            let Some(removed) = removed_by_agent.get(&agent.id).copied() else {
                continue;
            };
            agent.queue_len = agent.queue_len.saturating_sub(removed);
            if agent.queue_len == 0 && matches!(agent.status, AgentStatus::Waiting) {
                agent.status = AgentStatus::Idle;
            }
        }
    }
    let removed = before.saturating_sub(state.agents.messages.len());
    state.agents.console_scroll = CONSOLE_SCROLL_BOTTOM;
    state.agents.artifacts_selected = 0;
    state.agents.artifacts_selected_saved_run_path = None;
    state.agents.artifacts_history_selected = 0;
    state.agents.artifacts_history_popup_scroll = 0;
    state.agents.artifacts_history_popup_open = false;
    state.agents.artifacts_history_pending_action = None;

    state.agents.diag_events.push(AgentDiagnosticEvent {
        severity: AgentAlertSeverity::Info,
        source: "ops".into(),
        message: format!(
            "{}context reset{} (cleared {removed} msgs)",
            mission_ctx
                .as_deref()
                .map(|id| format!("mission {id} "))
                .unwrap_or_default(),
            if agent_label.is_empty() {
                format!(" for {agent_id}")
            } else {
                format!(" for {agent_id} ({agent_label})")
            }
        ),
        at: timestamp_label(state),
    });
    state.status = Some(format!(
        "{}Context reset: {}",
        mission_ctx
            .as_deref()
            .map(|id| format!("{id} "))
            .unwrap_or_default(),
        if agent_label.is_empty() {
            agent_id
        } else {
            agent_label.to_string()
        }
    ));
    true
}

fn handle_agent_console_key(
    key: KeyEvent,
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    swarm: &mut SwarmRuntime,
    clipboard: &mut Option<Clipboard>,
) -> bool {
    let mut changed = false;
    let mut handled = false;
    let mut follow_chat_cursor = false;

    // Try reusable text-editing handler first.
    let edit = handle_chat_input_editing_key(&key, state, clipboard);
    if edit.handled {
        handled = true;
        changed = edit.changed;
        follow_chat_cursor = edit.follow_cursor;
    }

    // Keys specific to the Agent Console context (not handled by the shared editor).
    if !handled {
        match key {
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                handled = true;
                changed = submit_chat_input_and_dispatch(state, vitals, codex, claude, swarm);
                follow_chat_cursor = changed;
            }
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                if matches!(
                    state.ui_selection,
                    Some(nit_core::UiSelection {
                        pane: UiSelectionPane::AgentConsole,
                        ..
                    })
                ) {
                    handled = true;
                    state.ui_selection = None;
                }
                if state.agents.chat_input_selection_anchor.is_some() {
                    handled = true;
                    state.agents.chat_input_selection_anchor = None;
                }
            }
            KeyEvent {
                modifiers,
                code: KeyCode::Up,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                handled = true;
                state.agents.console_scroll = state.agents.console_scroll.saturating_sub(1);
                changed = true;
            }
            KeyEvent {
                code: KeyCode::Down,
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                handled = true;
                state.agents.console_scroll = state.agents.console_scroll.saturating_add(1);
                changed = true;
            }
            KeyEvent {
                code: KeyCode::Up,
                modifiers,
                ..
            } => {
                handled = true;
                let selecting = modifiers.contains(KeyModifiers::SHIFT);
                let cursor = state.agents.chat_input_cursor;
                let moved = chat_cursor_move_vertical(&state.agents.chat_input, cursor, -1);
                if moved != cursor {
                    if selecting {
                        if state.agents.chat_input_selection_anchor.is_none() {
                            state.agents.chat_input_selection_anchor = Some(cursor);
                        }
                    } else {
                        state.agents.chat_input_selection_anchor = None;
                    }
                    state.agents.chat_input_cursor = moved;
                    changed = true;
                    follow_chat_cursor = true;
                    if selecting {
                        copy_chat_input_selection(state, clipboard);
                    }
                } else if !selecting && chat_history_prev(state) {
                    state.agents.chat_input_selection_anchor = None;
                    changed = true;
                    follow_chat_cursor = true;
                }
            }
            KeyEvent {
                code: KeyCode::Down,
                modifiers,
                ..
            } => {
                handled = true;
                let selecting = modifiers.contains(KeyModifiers::SHIFT);
                let cursor = state.agents.chat_input_cursor;
                let moved = chat_cursor_move_vertical(&state.agents.chat_input, cursor, 1);
                if moved != cursor {
                    if selecting {
                        if state.agents.chat_input_selection_anchor.is_none() {
                            state.agents.chat_input_selection_anchor = Some(cursor);
                        }
                    } else {
                        state.agents.chat_input_selection_anchor = None;
                    }
                    state.agents.chat_input_cursor = moved;
                    changed = true;
                    follow_chat_cursor = true;
                    if selecting {
                        copy_chat_input_selection(state, clipboard);
                    }
                } else if !selecting && chat_history_next(state) {
                    state.agents.chat_input_selection_anchor = None;
                    changed = true;
                    follow_chat_cursor = true;
                }
            }
            _ => {}
        }
    }

    if changed {
        if follow_chat_cursor {
            state.agents.chat_input_scroll = usize::MAX;
        }
        state.agents.note_event();
        vitals.record_agent_event(Instant::now());
    }
    handled
}

fn handle_paste_event(
    text: &str,
    state: &mut AppState,
    syntax: &mut SyntaxRuntime,
    fuzzy_runtime: &mut FuzzySearchRuntime,
    vitals: &mut VitalsState,
) -> bool {
    if text.is_empty() {
        return false;
    }

    if state.fuzzy_search.open {
        state.fuzzy_search.query.push_str(text);
        fuzzy_runtime.preview_model = None;
        fuzzy_runtime.last_preview_key = None;
        fuzzy_runtime.run_search_for_mode(state);
        return true;
    }

    if state.prompt.is_some()
        || state.rule_picker.open
        || state.protocol_picker.open
        || state.show_help
        || games_modal_popup_open(state)
    {
        return false;
    }

    if let Some(command_line) = state.command_line.as_mut() {
        for ch in text.chars() {
            command_line.insert(ch);
        }
        return true;
    }

    if state.agents.artifacts_popup_open {
        let changed = insert_popup_chat_text(state, text);
        if changed {
            state.agents.note_event();
            vitals.record_agent_event(Instant::now());
        }
        return changed;
    }

    if state.focus == PaneId::Notes {
        let changed = insert_chat_input_text(state, text);
        if changed {
            state.agents.note_event();
            vitals.record_agent_event(Instant::now());
        }
        return changed;
    }

    if pane_accepts_text_input(state, state.focus) && state.mode == Mode::Insert {
        return insert_text_into_focused_buffer(state, syntax, text);
    }

    false
}

pub(super) fn insert_chat_input_text(state: &mut AppState, text: &str) -> bool {
    let normalized = normalize_chat_input_text(text);
    if normalized.is_empty() {
        return false;
    }
    delete_chat_input_selection(state);
    let insert_at = chat_input_byte_index(&state.agents.chat_input, state.agents.chat_input_cursor);
    state.agents.chat_input.insert_str(insert_at, &normalized);
    state.agents.chat_input_cursor = state
        .agents
        .chat_input_cursor
        .saturating_add(normalized.chars().count());
    state.agents.chat_input_scroll = usize::MAX;
    state.agents.chat_input_selection_anchor = None;
    true
}

pub(super) fn normalize_chat_input_text(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if matches!(chars.peek(), Some('\n')) {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(ch);
        }
    }
    out
}

fn normalize_buffer_input_text(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('\r') {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if matches!(chars.peek(), Some('\n')) {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(ch);
        }
    }
    std::borrow::Cow::Owned(out)
}

fn chat_input_selection_range(state: &AppState) -> Option<(usize, usize)> {
    let total = state.agents.chat_input.chars().count();
    let cursor = state.agents.chat_input_cursor.min(total);
    let anchor = state.agents.chat_input_selection_anchor?.min(total);
    if anchor == cursor {
        return None;
    }
    Some((anchor.min(cursor), anchor.max(cursor)))
}

pub(super) fn delete_chat_input_selection(state: &mut AppState) -> bool {
    let Some((start, end)) = chat_input_selection_range(state) else {
        return false;
    };
    let remove_start = chat_input_byte_index(&state.agents.chat_input, start);
    let remove_end = chat_input_byte_index(&state.agents.chat_input, end);
    state
        .agents
        .chat_input
        .replace_range(remove_start..remove_end, "");
    state.agents.chat_input_cursor = start;
    state.agents.chat_input_selection_anchor = None;
    true
}

pub(super) fn copy_chat_input_selection(state: &mut AppState, clipboard: &mut Option<Clipboard>) -> bool {
    let Some((start, end)) = chat_input_selection_range(state) else {
        return false;
    };
    let text = slice_by_char(&state.agents.chat_input, start, end);
    if text.is_empty() {
        return false;
    }
    state.yank_kind = if text.contains('\n') {
        YankKind::Line
    } else {
        YankKind::Char
    };
    state.yank = Some(text.clone());
    if let Some(cb) = clipboard.as_mut() {
        let _ = cb.set_text(text);
    }
    true
}

fn insert_text_into_focused_buffer(
    state: &mut AppState,
    syntax: &mut SyntaxRuntime,
    text: &str,
) -> bool {
    if text.is_empty() {
        return false;
    }
    let normalized = normalize_buffer_input_text(text);
    if normalized.is_empty() {
        return false;
    }
    let editor_id = state.active_editor_buffer_id;
    let notes_id = state.notes_buffer_id;
    let editor_version = state.editor_buffer().version();
    let notes_version = state.notes_buffer().version();
    {
        let Some(buffer) = state.focused_buffer_mut() else {
            return false;
        };
        buffer.break_undo_group();
        buffer.insert_str(normalized.as_ref());
    }
    if state.editor_buffer().version() != editor_version {
        let buf = state.editor_buffer_mut();
        syntax.note_buffer_change(editor_id, buf);
    }
    if state.notes_buffer().version() != notes_version {
        let buf = state.notes_buffer_mut();
        syntax.note_buffer_change(notes_id, buf);
    }
    true
}

pub(super) fn chat_history_reset_nav(state: &mut AppState) {
    state.agents.chat_prompt_history_pos = None;
    state.agents.chat_prompt_history_draft = None;
}

fn chat_history_prev(state: &mut AppState) -> bool {
    if state.agents.chat_prompt_history.is_empty() {
        return false;
    }
    let next_pos = match state.agents.chat_prompt_history_pos {
        None => {
            state.agents.chat_prompt_history_draft = Some(state.agents.chat_input.clone());
            Some(state.agents.chat_prompt_history.len().saturating_sub(1))
        }
        Some(0) => None,
        Some(pos) => Some(pos.saturating_sub(1)),
    };
    let Some(pos) = next_pos else {
        return false;
    };
    if pos >= state.agents.chat_prompt_history.len() {
        chat_history_reset_nav(state);
        return false;
    }
    state.agents.chat_prompt_history_pos = Some(pos);
    state.agents.chat_input = state.agents.chat_prompt_history[pos].clone();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    state.agents.chat_input_selection_anchor = None;
    true
}

fn chat_history_next(state: &mut AppState) -> bool {
    let Some(pos) = state.agents.chat_prompt_history_pos else {
        return false;
    };
    let history_len = state.agents.chat_prompt_history.len();
    if history_len == 0 || pos >= history_len {
        chat_history_reset_nav(state);
        return false;
    }
    if pos.saturating_add(1) < history_len {
        let next = pos.saturating_add(1);
        state.agents.chat_prompt_history_pos = Some(next);
        state.agents.chat_input = state.agents.chat_prompt_history[next].clone();
    } else {
        state.agents.chat_prompt_history_pos = None;
        state.agents.chat_input = state
            .agents
            .chat_prompt_history_draft
            .take()
            .unwrap_or_default();
    }
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    state.agents.chat_input_selection_anchor = None;
    true
}

fn chat_cursor_move_vertical(input: &str, cursor_char_idx: usize, direction: i8) -> usize {
    let total_chars = input.chars().count();
    let cursor = cursor_char_idx.min(total_chars);
    if input.is_empty() {
        return 0;
    }
    let line_starts = chat_line_starts(input);
    if line_starts.is_empty() {
        return cursor;
    }
    let current_line = line_starts
        .iter()
        .rposition(|start| *start <= cursor)
        .unwrap_or(0);
    let target_line = if direction < 0 {
        current_line.saturating_sub(1)
    } else {
        (current_line + 1).min(line_starts.len().saturating_sub(1))
    };
    if target_line == current_line {
        return cursor;
    }
    let current_start = line_starts[current_line];
    let current_len = chat_line_len(&line_starts, current_line, total_chars);
    let column = cursor.saturating_sub(current_start).min(current_len);
    let target_start = line_starts[target_line];
    let target_len = chat_line_len(&line_starts, target_line, total_chars);
    target_start + column.min(target_len)
}

fn chat_line_starts(input: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (idx, ch) in input.chars().enumerate() {
        if ch == '\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

fn chat_line_len(line_starts: &[usize], line_idx: usize, total_chars: usize) -> usize {
    let start = line_starts.get(line_idx).copied().unwrap_or(total_chars);
    let end = if let Some(next_start) = line_starts.get(line_idx + 1).copied() {
        next_start.saturating_sub(1)
    } else {
        total_chars
    };
    end.saturating_sub(start)
}

pub(super) fn chat_current_line_bounds(input: &str, cursor_char_idx: usize) -> (usize, usize) {
    let total_chars = input.chars().count();
    let cursor = cursor_char_idx.min(total_chars);
    if input.is_empty() {
        return (0, 0);
    }
    let line_starts = chat_line_starts(input);
    let line_idx = line_starts
        .iter()
        .rposition(|start| *start <= cursor)
        .unwrap_or(0);
    let start = line_starts.get(line_idx).copied().unwrap_or(0);
    let end = if let Some(next_start) = line_starts.get(line_idx + 1).copied() {
        next_start.saturating_sub(1)
    } else {
        total_chars
    };
    (start.min(total_chars), end.min(total_chars))
}

pub(super) fn chat_current_line_indent(input: &str, cursor_char_idx: usize) -> String {
    let (start, end) = chat_current_line_bounds(input, cursor_char_idx);
    let mut out = String::new();
    for (idx, ch) in input.chars().enumerate() {
        if idx >= end {
            break;
        }
        if idx >= start {
            if ch == ' ' || ch == '\t' {
                out.push(ch);
            } else {
                break;
            }
        }
    }
    out
}

fn chat_is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-')
}

pub(super) fn chat_cursor_move_word_left(input: &str, cursor_char_idx: usize) -> usize {
    let chars: Vec<char> = input.chars().collect();
    let mut idx = cursor_char_idx.min(chars.len());
    while idx > 0 && chars[idx.saturating_sub(1)].is_whitespace() {
        idx = idx.saturating_sub(1);
    }
    let Some(prev) = idx.checked_sub(1).and_then(|pos| chars.get(pos).copied()) else {
        return idx;
    };
    if chat_is_word_char(prev) {
        while idx > 0 && chat_is_word_char(chars[idx.saturating_sub(1)]) {
            idx = idx.saturating_sub(1);
        }
        return idx;
    }
    while idx > 0
        && !chars[idx.saturating_sub(1)].is_whitespace()
        && !chat_is_word_char(chars[idx.saturating_sub(1)])
    {
        idx = idx.saturating_sub(1);
    }
    idx
}

pub(super) fn chat_cursor_move_word_right(input: &str, cursor_char_idx: usize) -> usize {
    let chars: Vec<char> = input.chars().collect();
    let mut idx = cursor_char_idx.min(chars.len());
    while idx < chars.len() && chars[idx].is_whitespace() {
        idx = idx.saturating_add(1);
    }
    let Some(next) = chars.get(idx).copied() else {
        return idx;
    };
    if chat_is_word_char(next) {
        while idx < chars.len() && chat_is_word_char(chars[idx]) {
            idx = idx.saturating_add(1);
        }
        return idx;
    }
    while idx < chars.len() && !chars[idx].is_whitespace() && !chat_is_word_char(chars[idx]) {
        idx = idx.saturating_add(1);
    }
    idx
}

fn spawn_mock_mission(state: &mut AppState) {
    let mission_id = format!("mis-{:03}", state.agents.missions.len() + 1);
    let assigned_agents = if let Some(agent_id) = state.agents.selected_context_agent() {
        let mut agents = vec![agent_id.to_string()];
        for extra in state.agents.agents.iter().take(2) {
            if !agents.iter().any(|id| id == &extra.id) {
                agents.push(extra.id.clone());
            }
        }
        agents
    } else {
        state
            .agents
            .agents
            .iter()
            .take(2)
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>()
    };
    state.agents.missions.push(MissionRecord {
        id: mission_id.clone(),
        title: format!("Mission {}", state.agents.missions.len() + 1),
        phase: MissionPhase::Plan,
        swarm: assigned_agents.len() > 1,
        assigned_agents: assigned_agents.clone(),
        status: "QUEUED".into(),
        updated_at: timestamp_label(state),
    });
    state.agents.mission_selected = state.agents.missions.len().saturating_sub(1);
    state.agents.selected_mission = Some(mission_id.clone());
    let message_text = format!(
        "New mission queued with swarm agents: {}",
        assigned_agents.join(", ")
    );
    state.agents.messages.push(AgentMessage {
        at: timestamp_label(state),
        channel: AgentChannel::Broadcast,
        agent_id: None,
        mission_id: Some(mission_id.clone()),
        text: message_text.clone(),
        prompt_msg_idx: None,
    });
    let delta = estimate_codex_context_tokens(&message_text);
    let entry = state
        .agents
        .codex_estimated_tokens_used_by_mission
        .entry(mission_id.clone())
        .or_insert(0);
    *entry = entry.saturating_add(delta);

    let patch_base = state.agents.patches.len() + 1;
    state.agents.patches.push(PatchProposal {
        id: format!("patch-{patch_base:03}"),
        mission_id: Some(mission_id.clone()),
        agent_id: assigned_agents
            .first()
            .cloned()
            .unwrap_or_else(|| "coder".into()),
        title: "Swarm proposal A".into(),
        summary: "Primary implementation candidate from lane A.".into(),
        diff: "diff --git a/src/lib.rs b/src/lib.rs\n@@ -1,2 +1,4 @@\n+// swarm proposal A\n"
            .into(),
        status: PatchStatus::New,
    });
    state.agents.patches.push(PatchProposal {
        id: format!("patch-{:03}", patch_base + 1),
        mission_id: Some(mission_id.clone()),
        agent_id: assigned_agents
            .get(1)
            .cloned()
            .unwrap_or_else(|| "reviewer".into()),
        title: "Swarm proposal B".into(),
        summary: "Alternative implementation from parallel lane.".into(),
        diff: "diff --git a/src/lib.rs b/src/lib.rs\n@@ -1,2 +1,4 @@\n+// swarm proposal B\n"
            .into(),
        status: PatchStatus::New,
    });
    state.agents.patch_selected = 0;
    state.agents.alerts.push(AgentAlert {
        severity: AgentAlertSeverity::Info,
        source: "mission".into(),
        message: format!("Created mission {mission_id}"),
        at: timestamp_label(state),
    });
    mark_mission_provenance_dirty(state, &mission_id);
}

pub(super) fn mark_mission_provenance_dirty(state: &mut AppState, mission_id: &str) {
    if state
        .agents
        .pending_provenance_mission_ids
        .iter()
        .all(|id| id != mission_id)
    {
        state
            .agents
            .pending_provenance_mission_ids
            .push(mission_id.to_string());
    }
}

pub(super) fn timestamp_label(state: &AppState) -> String {
    format!("t+{}", state.metrics.frame_count)
}

fn map_key_to_action(key: KeyEvent, state: &AppState, input: &mut InputState) -> Option<Action> {
    input.expire_visualizer_jump();
    // Prompt confirm takes precedence
    if let Some(Prompt::ConfirmQuit) = state.prompt {
        return match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Some(Action::ConfirmQuitYes),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some(Action::ConfirmQuitNo),
            _ => None,
        };
    }
    if let Some(target) = map_focus_hotkey(&key) {
        return Some(Action::FocusPane(target));
    }

    if state.command_line.is_none() && state.prompt.is_none() {
        if is_games_history_open_key(&key, state) {
            return Some(Action::GamesHistoryOpen);
        }
        match key {
            KeyEvent {
                code: KeyCode::Char('p') | KeyCode::Char('P'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                return Some(Action::OpenSearchPopup(SearchMode::Files));
            }
            KeyEvent {
                code: KeyCode::Char('f') | KeyCode::Char('F'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                return Some(Action::OpenSearchPopup(SearchMode::Content));
            }
            _ => {}
        }
    }

    if state.command_line.is_some() {
        return match key.code {
            KeyCode::Esc => Some(Action::CommandPromptCancel),
            KeyCode::Enter => Some(Action::CommandPromptExecute),
            KeyCode::Backspace => Some(Action::CommandPromptBackspace),
            KeyCode::Left => Some(Action::CommandPromptMoveLeft),
            KeyCode::Right => Some(Action::CommandPromptMoveRight),
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                Some(Action::CommandPromptInput(c))
            }
            _ => None,
        };
    }

    if is_job_pause_key(&key) {
        return Some(Action::ToggleJobPause);
    }

    if is_petri_show_key(&key, state) {
        return Some(match state.app_kind {
            AppKind::Gol => Action::PetriShow,
            AppKind::Games => Action::GamesShow,
        });
    }

    if is_global_run_key(&key) {
        return Some(match state.app_kind {
            AppKind::Gol => Action::VisualizerRun,
            AppKind::Games => Action::GamesRun,
        });
    }

    if state.app_kind == AppKind::Gol {
        if let Some(action) = visualizer_ctrl_action(&key, state) {
            return Some(action);
        }

        if state.focus == PaneId::Visualizer {
            if let Some(action) = visualizer_inspector_action(&key, state, input) {
                return Some(action);
            }
        }
    }

    if let Some(dir) = ctrl_nav_dir(&key) {
        return Some(Action::FocusPane(focus_by_direction(state, dir)));
    }

    if let Some(action) = handle_insert_chords(&key, state, input) {
        return Some(action);
    }

    if state.focus == PaneId::Editor
        && state.mode == Mode::Insert
        && input.pending_insert_matches(&key)
    {
        return None;
    }

    if is_visual_mode(state) {
        match key.code {
            KeyCode::Char('y') => return Some(Action::YankSelection),
            KeyCode::Char('d') => return Some(Action::DeleteSelection),
            KeyCode::Char('v') => return Some(Action::ExitVisual),
            _ => {}
        }
    }

    if let Some(action) = handle_normal_chords(&key, state, input) {
        return Some(action);
    }

    if is_command_prompt_open_key(&key) && state.mode == Mode::Normal {
        return Some(Action::CommandPromptOpen);
    }

    match key {
        KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::Quit),
        KeyEvent {
            code: KeyCode::Char('s'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::Save),
        KeyEvent {
            code: KeyCode::Char('t'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::ToggleFileTree),
        KeyEvent {
            code: KeyCode::Char('z') | KeyCode::Char('Z'),
            modifiers,
            ..
        } if pane_accepts_text_input(state, state.focus)
            && modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
        {
            if modifiers.contains(KeyModifiers::SHIFT) {
                Some(Action::Redo)
            } else {
                Some(Action::Undo)
            }
        }
        KeyEvent {
            code: KeyCode::Char('\u{1a}'),
            modifiers: KeyModifiers::NONE,
            ..
        } if pane_accepts_text_input(state, state.focus) => Some(Action::Undo),
        KeyEvent {
            code: KeyCode::Char('y') | KeyCode::Char('Y'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } if pane_accepts_text_input(state, state.focus) => Some(Action::Redo),
        KeyEvent {
            code: KeyCode::Char('\u{19}'),
            modifiers: KeyModifiers::NONE,
            ..
        } if pane_accepts_text_input(state, state.focus) => Some(Action::Redo),
        key if is_help_toggle_key(&key) => {
            if state.mode != Mode::Insert {
                Some(if state.show_help {
                    Action::HideHelp
                } else {
                    Action::ShowHelp
                })
            } else {
                None
            }
        }
        KeyEvent {
            code: KeyCode::Char('S'),
            modifiers,
            ..
        } if state.focus == PaneId::Editor
            && state.mode != Mode::Insert
            && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            Some(Action::ToggleSyntax)
        }
        KeyEvent {
            code: KeyCode::Char('g'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::VisualizerToggleSearch),
        KeyEvent {
            code: KeyCode::Char('y'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }
        | KeyEvent {
            code: KeyCode::Char('\u{19}'),
            modifiers: KeyModifiers::NONE,
            ..
        } => Some(Action::VisualizerToggleSeedSource),
        KeyEvent {
            code: KeyCode::Char('n'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::VisualizerSnapshot),
        KeyEvent {
            code: KeyCode::Char('b') | KeyCode::Char('B'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::ToggleDebug),
        KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::SHIFT,
            ..
        } => Some(Action::FocusPrevPane),
        KeyEvent {
            code: KeyCode::Tab, ..
        } => {
            if pane_accepts_text_input(state, state.focus) && state.mode == Mode::Insert {
                Some(Action::InsertTab)
            } else {
                Some(Action::FocusNextPane)
            }
        }
        KeyEvent {
            code: KeyCode::Esc, ..
        } => Some(Action::SwitchMode(Mode::Normal)),
        KeyEvent {
            code: KeyCode::Char('i'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_normal_mode(state) => Some(Action::SwitchMode(Mode::Insert)),
        KeyEvent {
            code: KeyCode::Char('v'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_normal_mode(state) => Some(Action::EnterVisual),
        KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_normal_mode(state) => Some(Action::Append),
        KeyEvent {
            code: KeyCode::Enter,
            ..
        } => Some(Action::InsertNewline),
        KeyEvent {
            code: KeyCode::Backspace,
            ..
        } => Some(Action::Backspace),
        KeyEvent {
            code: KeyCode::Delete,
            ..
        } => Some(Action::Delete),
        KeyEvent {
            code: KeyCode::Left,
            ..
        } => Some(Action::MoveLeft),
        KeyEvent {
            code: KeyCode::Right,
            ..
        } => Some(Action::MoveRight),
        KeyEvent {
            code: KeyCode::Up, ..
        } => Some(Action::MoveUp),
        KeyEvent {
            code: KeyCode::Down,
            ..
        } => Some(Action::MoveDown),
        KeyEvent {
            code: KeyCode::PageUp,
            ..
        } => Some(Action::PageUp),
        KeyEvent {
            code: KeyCode::PageDown,
            ..
        } => Some(Action::PageDown),
        KeyEvent {
            code: KeyCode::Home,
            ..
        } => Some(Action::Home),
        KeyEvent {
            code: KeyCode::End, ..
        } => Some(Action::End),
        KeyEvent {
            code: KeyCode::Char('G'),
            ..
        } if is_motion_mode(state) => Some(Action::GoToBottom),
        KeyEvent {
            code: KeyCode::Char('e'),
            ..
        } if is_motion_mode(state) => Some(Action::MoveWordEnd),
        KeyEvent {
            code: KeyCode::Char('b'),
            ..
        } if is_motion_mode(state) => Some(Action::MoveWordBack),
        KeyEvent {
            code: KeyCode::Char('u'),
            ..
        } if is_normal_mode(state) => Some(Action::Undo),
        KeyEvent {
            code: KeyCode::Char('R'),
            ..
        } if is_normal_mode(state) => Some(Action::Redo),
        KeyEvent {
            code: KeyCode::Char('o'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_normal_mode(state) => Some(Action::OpenLineBelow),
        KeyEvent {
            code: KeyCode::Char('O'),
            ..
        } if is_normal_mode(state) => Some(Action::OpenLineAbove),
        KeyEvent {
            code: KeyCode::Char('$'),
            ..
        } if is_motion_mode(state) => Some(Action::End),
        KeyEvent {
            code: KeyCode::Char('%'),
            ..
        } if is_motion_mode(state) => Some(Action::Home),
        KeyEvent {
            code: KeyCode::Char('p'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_normal_mode(state) => Some(Action::Paste),
        KeyEvent {
            code: KeyCode::Char('P'),
            ..
        } if is_normal_mode(state) => Some(Action::PasteLineAbove),
        KeyEvent {
            code: KeyCode::Char('h'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => Some(Action::MoveLeft),
        KeyEvent {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => Some(Action::MoveDown),
        KeyEvent {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => Some(Action::MoveUp),
        KeyEvent {
            code: KeyCode::Char('l'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => Some(Action::MoveRight),
        KeyEvent {
            code: KeyCode::Char(' '),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::ToggleJobPause),
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers,
            ..
        } if (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT)
            && pane_accepts_text_input(state, state.focus)
            && state.mode == Mode::Insert =>
        {
            Some(Action::InsertChar(c))
        }
        _ => None,
    }
}

fn apply_action_with_syntax(
    state: &mut AppState,
    syntax: &mut SyntaxRuntime,
    action: Action,
) -> nit_core::state::ActionOutcome {
    let before_focus = state.focus;
    let before_mode = state.mode;
    let before_debug = state.debug;
    let before_editor_id = state.active_editor_buffer_id;
    let before_notes_id = state.notes_buffer_id;
    let editor_version = state.editor_buffer().version();
    let notes_version = state.notes_buffer().version();
    let outcome = apply_action(state, action.clone());
    let after_editor_id = state.active_editor_buffer_id;
    let after_notes_id = state.notes_buffer_id;

    log_action(state, &action, before_focus, before_mode, before_debug);

    if after_editor_id == before_editor_id && state.editor_buffer().version() != editor_version {
        let buf = state.editor_buffer_mut();
        syntax.note_buffer_change(after_editor_id, buf);
    }
    if after_notes_id == before_notes_id && state.notes_buffer().version() != notes_version {
        let buf = state.notes_buffer_mut();
        syntax.note_buffer_change(after_notes_id, buf);
    }

    if matches!(action, Action::ToggleSyntax) {
        syntax.update_config(state.settings.highlight.clone());
        syntax.prime_buffer(after_editor_id, state.editor_buffer(), true);
        syntax.prime_buffer(after_notes_id, state.notes_buffer(), false);
    }
    if matches!(action, Action::OpenFile(_)) {
        // Avoid blocking highlight warmup when hopping files from NITTree.
        syntax.prime_buffer(after_editor_id, state.editor_buffer(), false);
    }

    outcome
}

fn log_action(
    state: &AppState,
    action: &Action,
    before_focus: PaneId,
    before_mode: Mode,
    before_debug: bool,
) {
    match action {
        Action::ToggleDebug => {
            tracing::info!(
                "DEBUG mode {}",
                if state.debug { "ENABLED" } else { "DISABLED" }
            );
        }
        Action::Save | Action::SaveAndNormal => {
            if let Some(status) = &state.status {
                if status.contains("Save failed") || status.contains("No path") {
                    tracing::warn!("SAVE {}", status);
                } else {
                    tracing::info!("SAVE {}", status);
                }
            }
        }
        Action::ConfirmQuitYes => tracing::info!("QUIT confirmed"),
        Action::ConfirmQuitNo => tracing::info!("QUIT canceled"),
        _ => {}
    }

    if !state.debug {
        return;
    }

    if before_focus != state.focus {
        tracing::info!("DEBUG focus {:?} -> {:?}", before_focus, state.focus);
    }
    if before_mode != state.mode {
        tracing::info!("DEBUG mode {:?} -> {:?}", before_mode, state.mode);
    }
    if before_debug != state.debug {
        tracing::info!("DEBUG toggle {}", state.debug);
    }

    tracing::info!("DEBUG action {:?}", action);
}

fn handle_clipboard_copy(state: &AppState, clipboard: &mut Option<Clipboard>, action: &Action) {
    if !matches!(action, Action::YankSelection | Action::YankLine) {
        return;
    }
    if let (Some(text), Some(cb)) = (state.yank.as_ref(), clipboard.as_mut()) {
        let _ = cb.set_text(text.clone());
    }
}

fn handle_selection_autocopy(
    state: &mut AppState,
    clipboard: &mut Option<Clipboard>,
    input_state: &mut InputState,
) {
    if state.mode != Mode::Visual {
        input_state.last_selection = None;
        return;
    }
    let (pane, buffer) = match state.focus {
        PaneId::Editor => (PaneId::Editor, state.editor_buffer()),
        PaneId::JobOutput if state.agents.dock_tab == AgentOpsTab::Scratchpad => {
            (PaneId::JobOutput, state.notes_buffer())
        }
        _ => {
            input_state.last_selection = None;
            return;
        }
    };
    let Some((start, end)) = buffer.selection_range() else {
        input_state.last_selection = None;
        return;
    };
    let signature = SelectionSignature { pane, start, end };
    if input_state.last_selection == Some(signature) {
        return;
    }
    input_state.last_selection = Some(signature);
    if let Some(text) = buffer.yank_selection() {
        state.yank_kind = if text.contains('\n') {
            YankKind::Line
        } else {
            YankKind::Char
        };
        state.yank = Some(text.clone());
        if let Some(cb) = clipboard.as_mut() {
            let _ = cb.set_text(text);
        }
    }
}

fn prepare_clipboard_paste(
    state: &mut AppState,
    clipboard: &mut Option<Clipboard>,
    action: &Action,
) {
    if !matches!(action, Action::Paste | Action::PasteLineAbove) || state.yank.is_some() {
        return;
    }
    if let Some(cb) = clipboard.as_mut() {
        if let Ok(text) = cb.get_text() {
            if !text.is_empty() {
                state.yank = Some(text);
                state.yank_kind = if state.yank.as_ref().is_some_and(|t| t.contains('\n')) {
                    YankKind::Line
                } else {
                    YankKind::Char
                };
            }
        }
    }
}

#[derive(Copy, Clone, Debug)]
enum FocusDir {
    Left,
    Right,
    Up,
    Down,
}

fn focus_by_direction(state: &AppState, dir: FocusDir) -> PaneId {
    use FocusDir::*;
    match state.focus {
        PaneId::Notes => match dir {
            Left => PaneId::Notes,
            Right => PaneId::Editor,
            Up => PaneId::Notes,
            Down => PaneId::JobOutput,
        },
        PaneId::JobOutput => match dir {
            Left => PaneId::JobOutput,
            Right => PaneId::Editor,
            Up => PaneId::Notes,
            Down => PaneId::JobOutput,
        },
        PaneId::Visualizer => match dir {
            Left => PaneId::Editor,
            Right => PaneId::Visualizer,
            Up => PaneId::Visualizer,
            Down => PaneId::GateMonitor,
        },
        PaneId::GateMonitor => match dir {
            Left => PaneId::Editor,
            Right => PaneId::GateMonitor,
            Up => PaneId::Visualizer,
            Down => PaneId::GateMonitor,
        },
        PaneId::Editor => {
            let buf = state.editor_buffer();
            let cursor_line = buf.cursor.line.saturating_sub(buf.viewport.offset_line);
            let top_half = cursor_line < buf.viewport.height.saturating_div(2).max(1);
            match dir {
                Left => {
                    if top_half {
                        PaneId::Notes
                    } else {
                        PaneId::JobOutput
                    }
                }
                Right => {
                    if top_half {
                        PaneId::Visualizer
                    } else {
                        PaneId::GateMonitor
                    }
                }
                Up => PaneId::Notes,
                Down => PaneId::JobOutput,
            }
        }
    }
}

struct InputState {
    normal_last_char: Option<char>,
    normal_last_time: Instant,
    pending_insert: Option<(char, Instant)>,
    deferred_key: Option<KeyEvent>,
    visualizer_jump: Option<InspectorJump>,
    last_selection: Option<SelectionSignature>,
    mouse_select_anchor: Option<MouseSelectAnchor>,
    last_ui_selection: Option<UiSelectionSignature>,
}

impl InputState {
    fn new() -> Self {
        Self {
            normal_last_char: None,
            normal_last_time: Instant::now(),
            pending_insert: None,
            deferred_key: None,
            visualizer_jump: None,
            last_selection: None,
            mouse_select_anchor: None,
            last_ui_selection: None,
        }
    }

    fn reset_normal(&mut self) {
        self.normal_last_char = None;
    }

    fn reset_insert(&mut self) {
        self.pending_insert = None;
    }

    fn chord_normal(&mut self, c: char, now: Instant) -> bool {
        if self.normal_last_char == Some(c)
            && now.duration_since(self.normal_last_time) <= CHORD_TIMEOUT
        {
            self.normal_last_char = None;
            true
        } else {
            self.normal_last_char = Some(c);
            self.normal_last_time = now;
            false
        }
    }

    fn set_pending_insert(&mut self, c: char, now: Instant) {
        self.pending_insert = Some((c, now));
    }

    fn take_pending_insert(&mut self) -> Option<char> {
        self.pending_insert.take().map(|(c, _)| c)
    }

    fn flush_insert_timeout(&mut self) -> Option<Action> {
        if let Some((c, t)) = self.pending_insert {
            if Instant::now().duration_since(t) >= CHORD_TIMEOUT {
                self.pending_insert = None;
                return Some(Action::InsertChar(c));
            }
        }
        None
    }

    fn pending_insert_matches(&self, key: &KeyEvent) -> bool {
        match (self.pending_insert, key.code) {
            (Some((pending, _)), KeyCode::Char(c)) => pending == c,
            _ => false,
        }
    }

    fn defer_key(&mut self, key: KeyEvent) {
        self.deferred_key = Some(key);
    }

    fn take_deferred(&mut self) -> Option<KeyEvent> {
        self.deferred_key.take()
    }

    fn start_visualizer_jump(&mut self) {
        self.visualizer_jump = Some(InspectorJump {
            value: 0,
            digits: 0,
            started: Instant::now(),
        });
    }

    fn clear_visualizer_jump(&mut self) {
        self.visualizer_jump = None;
    }

    fn push_visualizer_digit(&mut self, digit: u8) {
        if let Some(jump) = self.visualizer_jump.as_mut() {
            if jump.digits >= 18 {
                return;
            }
            jump.value = jump.value.saturating_mul(10).saturating_add(digit as u64);
            jump.digits += 1;
            jump.started = Instant::now();
        }
    }

    fn pop_visualizer_digit(&mut self) {
        if let Some(jump) = self.visualizer_jump.as_mut() {
            if jump.digits == 0 {
                return;
            }
            jump.value /= 10;
            jump.digits -= 1;
            jump.started = Instant::now();
        }
    }

    fn visualizer_jump_value(&self) -> Option<u64> {
        self.visualizer_jump.as_ref().map(|jump| jump.value)
    }

    fn visualizer_jump_active(&self) -> bool {
        self.visualizer_jump.is_some()
    }

    fn expire_visualizer_jump(&mut self) {
        if let Some(jump) = self.visualizer_jump.as_ref() {
            if Instant::now().duration_since(jump.started) >= INSPECTOR_JUMP_TIMEOUT {
                self.visualizer_jump = None;
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct SelectionSignature {
    pane: PaneId,
    start: usize,
    end: usize,
}

#[derive(Copy, Clone, Debug)]
struct MouseSelectAnchor {
    target: MouseSelectTarget,
    line: usize,
    col: usize,
}

#[derive(Copy, Clone, Debug)]
enum MouseSelectTarget {
    Buffer(PaneId),
    Ui(UiSelectionPane),
    ChatInput,
    PopupChatInput,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct UiSelectionSignature {
    pane: UiSelectionPane,
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
}

struct InspectorJump {
    value: u64,
    digits: u8,
    started: Instant,
}

fn is_normal_mode(state: &AppState) -> bool {
    pane_accepts_text_input(state, state.focus) && state.mode == Mode::Normal
}

fn is_visual_mode(state: &AppState) -> bool {
    pane_accepts_text_input(state, state.focus) && state.mode == Mode::Visual
}

fn is_motion_mode(state: &AppState) -> bool {
    pane_accepts_text_input(state, state.focus) && matches!(state.mode, Mode::Normal | Mode::Visual)
}

fn is_insert_editing(state: &AppState) -> bool {
    pane_accepts_text_input(state, state.focus) && state.mode == Mode::Insert
}

fn pane_accepts_text_input(_state: &AppState, pane: PaneId) -> bool {
    match pane {
        PaneId::Editor => true,
        PaneId::JobOutput => _state.agents.dock_tab == AgentOpsTab::Scratchpad,
        _ => false,
    }
}

fn handle_normal_chords(
    key: &KeyEvent,
    state: &AppState,
    input: &mut InputState,
) -> Option<Action> {
    if !is_motion_mode(state) {
        input.reset_normal();
        return None;
    }

    if !key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT {
        input.reset_normal();
        return None;
    }

    let now = Instant::now();
    match key.code {
        KeyCode::Char('g') => {
            if input.chord_normal('g', now) {
                Some(Action::GoToTop)
            } else {
                None
            }
        }
        KeyCode::Char('y') => {
            if is_normal_mode(state) && input.chord_normal('y', now) {
                Some(Action::YankLine)
            } else {
                None
            }
        }
        KeyCode::Char('d') => {
            if is_normal_mode(state) && input.chord_normal('d', now) {
                Some(Action::DeleteLine)
            } else {
                None
            }
        }
        _ => {
            input.reset_normal();
            None
        }
    }
}

fn handle_insert_chords(
    key: &KeyEvent,
    state: &AppState,
    input: &mut InputState,
) -> Option<Action> {
    if !is_insert_editing(state) || state.focus != PaneId::Editor {
        input.reset_insert();
        return None;
    }

    if !key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT {
        input.reset_insert();
        return None;
    }

    if let Some((pending, _)) = input.pending_insert {
        match key.code {
            KeyCode::Char('j') => {
                input.reset_insert();
                return Some(Action::SaveAndNormal);
            }
            _ => {
                input.defer_key(*key);
                let c = input.take_pending_insert().unwrap_or(pending);
                return Some(Action::InsertChar(c));
            }
        }
    }

    match key.code {
        KeyCode::Char('j') => {
            input.set_pending_insert('j', Instant::now());
            None
        }
        _ => None,
    }
}

fn visualizer_ctrl_action(key: &KeyEvent, state: &AppState) -> Option<Action> {
    let petri_visible = state.visualizer.running && !state.visualizer.petri_hidden;
    if state.focus != PaneId::Visualizer && !petri_visible {
        return None;
    }
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        return match key.code {
            KeyCode::Char('v') | KeyCode::Char('V') => Some(Action::VisualizerCycleSeedOverlays),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Char('v') | KeyCode::Char('V') => Some(Action::VisualizerToggleSeedView),
        KeyCode::Char('r') | KeyCode::Char('R') => Some(Action::VisualizerCycleSeedView),
        KeyCode::Char('e') | KeyCode::Char('E') => Some(Action::VisualizerCycleEncoder),
        KeyCode::Char('a') | KeyCode::Char('A') => Some(Action::VisualizerApply),
        KeyCode::Char('g') | KeyCode::Char('G') => Some(Action::VisualizerToggleSearch),
        KeyCode::Char('y') | KeyCode::Char('Y') => Some(Action::VisualizerToggleSeedSource),
        KeyCode::Char('n') | KeyCode::Char('N') => Some(Action::VisualizerSnapshot),
        KeyCode::Char('m') | KeyCode::Char('M') => Some(Action::VisualizerCycleRenderMode),
        KeyCode::Char('j') | KeyCode::Char('J') => Some(Action::VisualizerToggleAgeShading),
        KeyCode::Char('k') | KeyCode::Char('K') => Some(Action::VisualizerToggleTrails),
        KeyCode::Char('b') | KeyCode::Char('B') => Some(Action::VisualizerToggleBBox),
        KeyCode::Char('h') | KeyCode::Char('H') => Some(Action::VisualizerToggleHeat),
        KeyCode::Char('l') | KeyCode::Char('L') => Some(Action::VisualizerToggleScanlines),
        _ => None,
    }
}

fn visualizer_inspector_action(
    key: &KeyEvent,
    state: &AppState,
    input: &mut InputState,
) -> Option<Action> {
    if input.visualizer_jump_active() {
        match key.code {
            KeyCode::Char(c) if c.is_ascii_digit() && key.modifiers.is_empty() => {
                input.push_visualizer_digit(c as u8 - b'0');
                return None;
            }
            KeyCode::Backspace => {
                input.pop_visualizer_digit();
                return None;
            }
            KeyCode::Enter => {
                let value = input.visualizer_jump_value().unwrap_or(0);
                input.clear_visualizer_jump();
                return Some(Action::VisualizerInspectJump(value));
            }
            KeyCode::Esc => {
                input.clear_visualizer_jump();
                return None;
            }
            _ => {
                input.clear_visualizer_jump();
                return None;
            }
        }
    }

    if !state.visualizer.inspector_enabled {
        if matches!(key.code, KeyCode::Char('i') | KeyCode::Char('I'))
            && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
        {
            return Some(Action::VisualizerInspectToggle);
        }
        return None;
    }

    match key.code {
        KeyCode::Home => return Some(Action::VisualizerInspectHome),
        KeyCode::End => return Some(Action::VisualizerInspectEnd),
        _ => {}
    }

    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
        match key.code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('H') => {
                return Some(Action::VisualizerInspectLeft)
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('L') => {
                return Some(Action::VisualizerInspectRight)
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
                return Some(Action::VisualizerInspectUp)
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
                return Some(Action::VisualizerInspectDown)
            }
            KeyCode::Char('0') => return Some(Action::VisualizerInspectHome),
            KeyCode::Char('$') => return Some(Action::VisualizerInspectEnd),
            KeyCode::Char('c') | KeyCode::Char('C') => {
                return Some(Action::VisualizerInspectCenter)
            }
            KeyCode::Char('i') | KeyCode::Char('I') => {
                return Some(Action::VisualizerInspectToggle)
            }
            KeyCode::Char('g') | KeyCode::Char('G') => {
                input.start_visualizer_jump();
                return None;
            }
            _ => {}
        }
    }
    None
}

fn is_global_run_key(key: &KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Enter | KeyCode::Char('\n') | KeyCode::Char('\r'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    )
}

fn is_global_quit_key(key: &KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('q'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    )
}

fn is_command_prompt_open_key(key: &KeyEvent) -> bool {
    match key {
        KeyEvent {
            code: KeyCode::Char(':'),
            ..
        } => true,
        KeyEvent {
            code: KeyCode::Char(';'),
            modifiers,
            ..
        } => modifiers.contains(KeyModifiers::SHIFT),
        _ => false,
    }
}

fn is_help_toggle_key(key: &KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::F(1),
            ..
        }
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('?'),
            modifiers,
            ..
        } if modifiers.is_empty() || *modifiers == KeyModifiers::SHIFT
    )
}

fn is_petri_show_key(key: &KeyEvent, state: &AppState) -> bool {
    match state.app_kind {
        AppKind::Gol => {
            if !state.visualizer.petri_hidden || !state.visualizer.running {
                return false;
            }
        }
        AppKind::Games => {
            if !state.games.petri_hidden || !games_petri_active(state) {
                return false;
            }
        }
    }
    match key {
        KeyEvent {
            code: KeyCode::Char('^') | KeyCode::Char('6'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => true,
        KeyEvent {
            code: KeyCode::Char('\u{1e}'),
            modifiers,
            ..
        } if modifiers.is_empty() || modifiers.contains(KeyModifiers::CONTROL) => true,
        _ => false,
    }
}

fn games_petri_active(state: &AppState) -> bool {
    state.games.running
        || matches!(
            state.games.status,
            nit_core::GamesStatus::Paused | nit_core::GamesStatus::Done
        )
        || !state.games.petri_lines.is_empty()
}

fn is_games_history_open_key(key: &KeyEvent, state: &AppState) -> bool {
    if state.app_kind != AppKind::Games {
        return false;
    }
    if !state.games.running && state.games.last_run.is_none() {
        return false;
    }
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('*') | KeyCode::Char('8'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    )
}

fn games_petri_visible(state: &AppState) -> bool {
    state.app_kind == AppKind::Games && games_petri_active(state) && !state.games.petri_hidden
}

fn current_games_config_result(
    state: &AppState,
) -> Option<&Result<nit_games::config::NormalizedConfig, nit_games::config::ConfigError>> {
    let version = state.editor_buffer().version();
    state
        .games
        .config_preview
        .as_ref()
        .and_then(|preview| (preview.version == version).then_some(&preview.result))
}

fn games_modal_popup_open(state: &AppState) -> bool {
    if state.app_kind != AppKind::Games {
        return false;
    }
    state.games.analysis.open
        || state.games.run_browser.open
        || state.games.replay.open
        || state.games.strategy_inspect.open
        || state.games.tm_sim.open
        || state.games.ca_sim.open
        || state.games.match_history.open
}

fn is_games_petri_control_key(key: &KeyEvent) -> bool {
    matches!(
        key.code,
        KeyCode::Char(' ')
            | KeyCode::Null
            | KeyCode::Char('\u{0}')
            | KeyCode::Enter
            | KeyCode::Char('\n')
            | KeyCode::Char('\r')
            | KeyCode::Char('+')
            | KeyCode::Char('=')
            | KeyCode::Char('-')
            | KeyCode::Char('_')
            | KeyCode::Tab
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Char('c')
            | KeyCode::Char('C')
            | KeyCode::Char('h')
            | KeyCode::Char('H')
            | KeyCode::Char('r')
            | KeyCode::Char('R')
            | KeyCode::Char('x')
            | KeyCode::Char('X')
            | KeyCode::Char('y')
            | KeyCode::Char('Y')
            | KeyCode::Char('n')
            | KeyCode::Char('N')
    )
}

fn is_job_pause_key(key: &KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Char(' '),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Null,
            ..
        }
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('\u{0}'),
            modifiers,
            ..
        } if modifiers.is_empty()
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::F(6),
            ..
        }
    )
}

fn ctrl_nav_dir(key: &KeyEvent) -> Option<FocusDir> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if !ctrl || key.modifiers.contains(KeyModifiers::SHIFT) {
        return None;
    }
    match key.code {
        KeyCode::Char('h') if ctrl => Some(FocusDir::Left),
        KeyCode::Char('j') if ctrl => Some(FocusDir::Down),
        KeyCode::Char('k') if ctrl => Some(FocusDir::Up),
        KeyCode::Char('l') if ctrl => Some(FocusDir::Right),
        KeyCode::Backspace if ctrl => Some(FocusDir::Left),
        KeyCode::Enter if ctrl => Some(FocusDir::Down),
        KeyCode::Char('\u{8}') => Some(FocusDir::Left),
        KeyCode::Char('\n') => Some(FocusDir::Down),
        KeyCode::Char('\u{0b}') => Some(FocusDir::Up),
        KeyCode::Char('\u{0c}') => Some(FocusDir::Right),
        _ => None,
    }
}

fn dynamic_popup_rect(screen: ratatui::layout::Rect, desired: (u16, u16)) -> ratatui::layout::Rect {
    use ratatui::layout::{Constraint, Direction, Layout};
    let max_w = screen.width.saturating_sub(4).max(10);
    let max_h = screen.height.saturating_sub(2).max(5);
    let width = desired.0.min(max_w);
    let height = desired.1.min(max_h);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((screen.height.saturating_sub(height)) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(screen)[1];
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((screen.width.saturating_sub(width)) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(vertical)[1]
}

fn fuzzy_popup_size(screen: ratatui::layout::Rect, state: &AppState) -> (u16, u16) {
    let _ = state;
    fuzzy_search_popup::preferred_size(screen)
}

#[allow(clippy::too_many_arguments)]
fn handle_mouse_event_with_swarm(
    swarm: &SwarmRuntime,
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    fuzzy_runtime: &mut FuzzySearchRuntime,
    input_state: &mut InputState,
    clipboard: &mut Option<Clipboard>,
    theme: &Theme,
) -> bool {
    match mouse.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let fast = mouse
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL);
            let step = if fast {
                SCROLL_LINES_FAST
            } else {
                SCROLL_LINES
            };
            let delta = if matches!(mouse.kind, MouseEventKind::ScrollUp) {
                -(step as i32)
            } else {
                step as i32
            };
            if state.command_line.is_some() || state.prompt.is_some() {
                return true;
            }

            if state.rule_picker.open || state.protocol_picker.open {
                return true;
            }

            if state.fuzzy_search.open {
                use ratatui::layout::{Constraint, Direction, Layout, Rect};
                let area = dynamic_popup_rect(screen, fuzzy_popup_size(screen, state));
                let list_height = area
                    .height
                    .saturating_sub(6) // outer(2) + header/footer(2) + results block(2)
                    .max(1) as usize;
                let mut over_preview = false;
                if point_in_rect(mouse.column, mouse.row, area) {
                    let inner = Rect {
                        x: area.x.saturating_add(1),
                        y: area.y.saturating_add(1),
                        width: area.width.saturating_sub(2),
                        height: area.height.saturating_sub(2),
                    };
                    let body = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(1),
                            Constraint::Min(1),
                            Constraint::Length(1),
                        ])
                        .split(inner)[1];
                    let halves = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(body);
                    over_preview = point_in_rect(mouse.column, mouse.row, halves[1]);
                }
                if over_preview {
                    fuzzy_runtime.preview_scroll_delta =
                        fuzzy_runtime.preview_scroll_delta.saturating_add(delta);
                } else {
                    let len = fuzzy_results_len(state);
                    if len > 0 {
                        if delta.is_negative() {
                            state.fuzzy_search.selected = state
                                .fuzzy_search
                                .selected
                                .saturating_sub(delta.unsigned_abs() as usize);
                        } else {
                            state.fuzzy_search.selected =
                                (state.fuzzy_search.selected + delta as usize).min(len - 1);
                        }
                        adjust_fuzzy_scroll(state, list_height);
                        fuzzy_runtime.request_preview_for_selection(state);
                    }
                }
                // Modal: don't scroll underlying panes while open.
                return true;
            }

            if state.agents.artifacts_history_popup_open {
                let area =
                    dynamic_popup_rect(screen, artifacts_history_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let (max_scroll, _) =
                        artifacts_history_popup_scroll_metrics(state, screen, theme);
                    bump_scroll_clamped(
                        &mut state.agents.artifacts_history_popup_scroll,
                        delta,
                        max_scroll,
                    );
                }
                return true;
            }

            if state.agents.artifacts_popup_open {
                let area = dynamic_popup_rect(screen, artifacts_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let (max_scroll, _) =
                        artifacts_popup_scroll_metrics(state, swarm, screen, theme);
                    bump_scroll_clamped(
                        &mut state.agents.artifacts_popup_scroll,
                        delta,
                        max_scroll,
                    );
                }
                return true;
            }

            if state.show_help {
                let area = dynamic_popup_rect(screen, help_overlay::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max_scroll = help_popup_max_scroll(screen, theme);
                    bump_scroll_clamped(&mut state.help_scroll, delta, max_scroll);
                }
                return true;
            }

            if state.app_kind == AppKind::Games && state.games.analysis.open {
                let area = dynamic_popup_rect(screen, games_analysis_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max_scroll = games_analysis_popup_max_scroll(state, screen, theme);
                    bump_scroll_clamped(&mut state.games.analysis.scroll_offset, delta, max_scroll);
                }
                return true;
            }

            if state.app_kind == AppKind::Games && state.games.run_browser.open {
                let area =
                    dynamic_popup_rect(screen, games_run_browser_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max_scroll = games_run_browser_popup_max_scroll(state, screen, theme);
                    bump_scroll_clamped(
                        &mut state.games.run_browser.scroll_offset,
                        delta,
                        max_scroll,
                    );
                }
                return true;
            }

            if state.app_kind == AppKind::Games && state.games.replay.open {
                let area = dynamic_popup_rect(screen, games_replay_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max_scroll = games_replay_popup_max_scroll(state, screen, theme);
                    bump_scroll_clamped(&mut state.games.replay.scroll_offset, delta, max_scroll);
                }
                return true;
            }

            if state.app_kind == AppKind::Games && state.games.strategy_inspect.open {
                let area = dynamic_popup_rect(screen, games_strategy_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max_scroll = games_strategy_popup_max_scroll(state, screen);
                    bump_scroll_clamped(
                        &mut state.games.strategy_inspect.scroll_offset,
                        delta,
                        max_scroll,
                    );
                }
                return true;
            }
            if state.app_kind == AppKind::Games && state.games.tm_sim.open {
                let area = dynamic_popup_rect(screen, games_tm_sim_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max_scroll = games_tm_sim_popup_max_scroll(state, screen, theme);
                    bump_scroll_clamped(&mut state.games.tm_sim.scroll_offset, delta, max_scroll);
                }
                return true;
            }
            if state.app_kind == AppKind::Games && state.games.ca_sim.open {
                let area = dynamic_popup_rect(screen, games_ca_sim_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max_scroll = games_ca_sim_popup_max_scroll(state, screen, theme);
                    bump_scroll_clamped(&mut state.games.ca_sim.scroll_offset, delta, max_scroll);
                }
                return true;
            }
            if state.app_kind == AppKind::Games && state.games.match_history.open {
                let area =
                    dynamic_popup_rect(screen, games_match_history_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max = games_match_history_max_offset(state, screen);
                    if delta > 0 {
                        state.games.match_history.column_offset =
                            state.games.match_history.column_offset.saturating_sub(1);
                    } else if games_match_history_total_entries(state) > 0 {
                        state.games.match_history.column_offset =
                            (state.games.match_history.column_offset + 1).min(max);
                    }
                }
                return true;
            }

            if games_petri_visible(state) {
                return true;
            }

            let layout = layout::split(screen);
            if point_in_rect(mouse.column, mouse.row, layout.editor) {
                scroll_buffer(state.editor_buffer_mut(), delta);
                return true;
            }
            if point_in_rect(mouse.column, mouse.row, layout.notes) {
                if let Some(metrics) =
                    agent_console_view::chat_input_scroll_metrics(layout.notes, state)
                {
                    if point_in_rect(mouse.column, mouse.row, metrics.area) {
                        let mut start = metrics.window_start;
                        bump_scroll(&mut start, delta);
                        state.agents.chat_input_scroll = start.min(metrics.max_scroll);
                        return true;
                    }
                }
                if let Some(thread_area) = agent_console_view::thread_text_area(layout.notes, state)
                {
                    let lines = agent_console_view::thread_lines_for_selection_with_swarm(
                        state,
                        swarm,
                        thread_area.width.max(1) as usize,
                    );
                    let max_scroll = lines
                        .len()
                        .saturating_sub(thread_area.height.max(1) as usize);
                    let mut scroll = state.agents.console_scroll.min(max_scroll);
                    bump_scroll(&mut scroll, delta);
                    state.agents.console_scroll = scroll.min(max_scroll);
                } else {
                    let mut scroll = state.agents.console_scroll;
                    bump_scroll(&mut scroll, delta);
                    state.agents.console_scroll = scroll;
                }
                return true;
            }
            if point_in_rect(mouse.column, mouse.row, layout.job) {
                if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                    scroll_buffer(state.notes_buffer_mut(), delta);
                } else {
                    let text_area = job_output_text_area(layout.job);
                    let text_width = text_area.width as usize;
                    let lines = agent_ops_view::current_lines_for_width_with_swarm(
                        state,
                        Some(swarm),
                        text_width,
                    );
                    let height = text_area.height as usize;
                    let max_scroll = lines.len().saturating_sub(height);
                    let mut scroll = state.agents.ops_scroll;
                    bump_scroll(&mut scroll, delta);
                    state.agents.ops_scroll = scroll.min(max_scroll);
                }
                return true;
            }
            false
        }
        MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
            if state.fuzzy_search.open {
                handle_fuzzy_search_mouse_down(mouse, screen, state, fuzzy_runtime)
            } else {
                handle_mouse_down_with_swarm(
                    swarm,
                    mouse,
                    screen,
                    state,
                    input_state,
                    clipboard,
                    theme,
                )
            }
        }
        MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
            handle_mouse_drag_with_swarm(swarm, mouse, screen, state, input_state, clipboard, theme)
        }
        MouseEventKind::Up(crossterm::event::MouseButton::Left) => {
            maybe_open_artifacts_popup_url_on_click(
                swarm,
                mouse,
                screen,
                state,
                input_state,
                theme,
            );
            input_state.mouse_select_anchor = None;
            true
        }
        _ => false,
    }
}

const SCROLL_LINES: usize = 1;
const SCROLL_LINES_FAST: usize = 5;

fn handle_fuzzy_search_mouse_down(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    runtime: &mut FuzzySearchRuntime,
) -> bool {
    use ratatui::layout::{Constraint, Direction, Layout, Rect};

    let area = dynamic_popup_rect(screen, fuzzy_popup_size(screen, state));
    // Modal: ignore clicks outside the popup (and prevent underlying panes from receiving them).
    if !point_in_rect(mouse.column, mouse.row, area) {
        return true;
    }

    let list_height = area
        .height
        .saturating_sub(6) // outer(2) + header/footer(2) + results block(2)
        .max(1) as usize;

    let inner = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner)[1];
    let halves = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(body);

    // RESULTS block has its own border.
    let results_inner = Rect {
        x: halves[0].x.saturating_add(1),
        y: halves[0].y.saturating_add(1),
        width: halves[0].width.saturating_sub(2),
        height: halves[0].height.saturating_sub(2),
    };
    if point_in_rect(mouse.column, mouse.row, results_inner) {
        let idx_in_view = mouse.row.saturating_sub(results_inner.y) as usize;
        let target = state.fuzzy_search.scroll_offset.saturating_add(idx_in_view);
        let len = fuzzy_results_len(state);
        if len > 0 && target < len {
            state.fuzzy_search.selected = target;
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
        }
    }

    true
}

fn bump_scroll(value: &mut usize, delta: i32) {
    if delta.is_negative() {
        *value = value.saturating_sub(delta.unsigned_abs() as usize);
    } else if delta > 0 {
        *value = value.saturating_add(delta as usize);
    }
}

fn bump_scroll_clamped(value: &mut usize, delta: i32, max_scroll: usize) {
    let mut scroll = (*value).min(max_scroll);
    bump_scroll(&mut scroll, delta);
    *value = scroll.min(max_scroll);
}

fn popup_max_scroll(line_count: usize, text_area: ratatui::layout::Rect) -> usize {
    if text_area.height == 0 {
        return 0;
    }
    line_count.saturating_sub(text_area.height as usize)
}

fn max_scroll_for_height(line_count: usize, height: usize) -> usize {
    if height == 0 {
        return 0;
    }
    line_count.saturating_sub(height)
}

fn popup_page_step(text_area: ratatui::layout::Rect) -> usize {
    text_area.height.max(1) as usize
}

fn popup_text_metrics(area: ratatui::layout::Rect, line_count: usize) -> (usize, usize) {
    let text_area = popup_text_area(area);
    (
        popup_max_scroll(line_count, text_area),
        popup_page_step(text_area),
    )
}

fn artifacts_history_popup_scroll_metrics(
    state: &AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> (usize, usize) {
    let area = dynamic_popup_rect(screen, artifacts_history_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let lines = artifacts_history_popup::build_lines(state, theme, text_area.width);
    (
        popup_max_scroll(lines.len(), text_area),
        popup_page_step(text_area),
    )
}

fn help_popup_max_scroll(screen: ratatui::layout::Rect, theme: &Theme) -> usize {
    let area = dynamic_popup_rect(screen, help_overlay::preferred_size(screen));
    let text_area = popup_text_area(area);
    let lines = help_overlay::build_lines(theme);
    popup_max_scroll(lines.len(), text_area)
}

fn help_popup_scroll_metrics(screen: ratatui::layout::Rect, theme: &Theme) -> (usize, usize) {
    let area = dynamic_popup_rect(screen, help_overlay::preferred_size(screen));
    popup_text_metrics(area, help_overlay::build_lines(theme).len())
}

fn games_analysis_popup_max_scroll(
    state: &AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> usize {
    let area = dynamic_popup_rect(screen, games_analysis_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let lines = games_analysis_popup::build_lines(state, theme, text_area.width);
    popup_max_scroll(lines.len(), text_area)
}

fn games_analysis_popup_scroll_metrics(
    state: &AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> (usize, usize) {
    let area = dynamic_popup_rect(screen, games_analysis_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let lines = games_analysis_popup::build_lines(state, theme, text_area.width);
    (
        popup_max_scroll(lines.len(), text_area),
        popup_page_step(text_area),
    )
}

fn games_run_browser_popup_max_scroll(
    state: &AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> usize {
    let area = dynamic_popup_rect(screen, games_run_browser_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let lines = games_run_browser_popup::build_lines(state, theme, text_area.width);
    popup_max_scroll(lines.len(), text_area)
}

fn games_replay_popup_max_scroll(
    state: &AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> usize {
    let area = dynamic_popup_rect(screen, games_replay_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let lines = games_replay_popup::build_lines(state, theme, text_area.width);
    popup_max_scroll(lines.len(), text_area)
}

fn games_strategy_popup_max_scroll(state: &AppState, screen: ratatui::layout::Rect) -> usize {
    let area = dynamic_popup_rect(screen, games_strategy_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    games_strategy_popup::max_scroll(state, text_area)
}

fn games_tm_sim_popup_max_scroll(
    state: &AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> usize {
    let area = dynamic_popup_rect(screen, games_tm_sim_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let (left_area, right_area) = games_tm_sim_popup::layout_for_tm_sim(text_area);
    if let Some(right_area) = right_area {
        let right_inner = Block::default().borders(Borders::ALL).inner(right_area);
        let (left_lines, right_lines) = games_tm_sim_popup::build_columns(
            state,
            theme,
            left_area.width.max(1) as usize,
            right_inner.width.max(1) as usize,
        );
        let content_height = left_area.height.min(right_inner.height) as usize;
        max_scroll_for_height(left_lines.len().max(right_lines.len()), content_height)
    } else {
        let (lines, _) =
            games_tm_sim_popup::build_columns(state, theme, text_area.width.max(1) as usize, 0);
        popup_max_scroll(lines.len(), text_area)
    }
}

fn games_ca_sim_popup_max_scroll(
    state: &AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> usize {
    let area = dynamic_popup_rect(screen, games_ca_sim_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let (left_area, right_area) = games_ca_sim_popup::layout_for_ca_sim(text_area);
    if let Some(right_area) = right_area {
        let right_inner = Block::default().borders(Borders::ALL).inner(right_area);
        let (left_lines, right_lines) = games_ca_sim_popup::build_columns(
            state,
            theme,
            left_area.width.max(1) as usize,
            right_inner.width.max(1) as usize,
        );
        let content_height = left_area.height.min(right_inner.height) as usize;
        max_scroll_for_height(left_lines.len().max(right_lines.len()), content_height)
    } else {
        let (lines, _) =
            games_ca_sim_popup::build_columns(state, theme, text_area.width.max(1) as usize, 0);
        popup_max_scroll(lines.len(), text_area)
    }
}

fn games_match_history_max_offset(state: &AppState, screen: ratatui::layout::Rect) -> usize {
    let area = dynamic_popup_rect(screen, games_match_history_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    games_match_history_popup::max_column_offset(
        games_match_history_total_entries(state),
        text_area.width,
    )
}

fn games_match_history_max_rounds(state: &AppState) -> usize {
    if state.games.match_history.max_rounds_seen > 0 {
        state.games.match_history.max_rounds_seen
    } else {
        games_match_history_popup::max_round_limit(state.games.match_history.entries.as_slice())
    }
}

fn games_match_history_total_entries(state: &AppState) -> usize {
    if state.games.match_history.total_entries > 0 {
        state.games.match_history.total_entries
    } else {
        state.games.match_history.entries.len()
    }
}

fn games_match_history_default_rounds(state: &AppState) -> usize {
    games_match_history_popup::default_round_limit(games_match_history_max_rounds(state))
}

fn clamp_modal_scroll_offsets(state: &mut AppState, screen: ratatui::layout::Rect, theme: &Theme) {
    if state.show_help {
        let max_scroll = help_popup_max_scroll(screen, theme);
        state.help_scroll = state.help_scroll.min(max_scroll);
    }
    if state.agents.artifacts_history_popup_open {
        let (max_scroll, _) = artifacts_history_popup_scroll_metrics(state, screen, theme);
        state.agents.artifacts_history_popup_scroll =
            state.agents.artifacts_history_popup_scroll.min(max_scroll);
        let max = agent_ops_view::artifacts_history_visible_entries(state)
            .len()
            .saturating_sub(1);
        state.agents.artifacts_history_selected = state.agents.artifacts_history_selected.min(max);
    }
    if state.app_kind != AppKind::Games {
        return;
    }
    if state.games.analysis.open {
        let max_scroll = games_analysis_popup_max_scroll(state, screen, theme);
        state.games.analysis.scroll_offset = state.games.analysis.scroll_offset.min(max_scroll);
    }
    if state.games.run_browser.open {
        let max_scroll = games_run_browser_popup_max_scroll(state, screen, theme);
        state.games.run_browser.scroll_offset =
            state.games.run_browser.scroll_offset.min(max_scroll);
    }
    if state.games.replay.open {
        let max_scroll = games_replay_popup_max_scroll(state, screen, theme);
        state.games.replay.scroll_offset = state.games.replay.scroll_offset.min(max_scroll);
    }
    if state.games.strategy_inspect.open {
        let max_scroll = games_strategy_popup_max_scroll(state, screen);
        state.games.strategy_inspect.scroll_offset =
            state.games.strategy_inspect.scroll_offset.min(max_scroll);
    }
    if state.games.tm_sim.open {
        let max_scroll = games_tm_sim_popup_max_scroll(state, screen, theme);
        state.games.tm_sim.scroll_offset = state.games.tm_sim.scroll_offset.min(max_scroll);
    }
    if state.games.ca_sim.open {
        let max_scroll = games_ca_sim_popup_max_scroll(state, screen, theme);
        state.games.ca_sim.scroll_offset = state.games.ca_sim.scroll_offset.min(max_scroll);
    }
    if state.games.match_history.open {
        let max_offset = games_match_history_max_offset(state, screen);
        state.games.match_history.column_offset =
            state.games.match_history.column_offset.min(max_offset);
        let max_rounds = games_match_history_max_rounds(state);
        let default_rounds = games_match_history_default_rounds(state);
        if let Some(limit) = state.games.match_history.round_limit {
            let clamped = limit.min(max_rounds);
            state.games.match_history.round_limit = if clamped == default_rounds {
                None
            } else {
                Some(clamped)
            };
        }
    }
}

fn scroll_buffer(buf: &mut nit_core::Buffer, delta: i32) {
    let height = buf.viewport.height.max(1);
    let max_offset = buf.lines_len().saturating_sub(height);
    let offset = buf.viewport.offset_line as i32 + delta;
    let clamped = offset.clamp(0, max_offset as i32);
    buf.viewport.offset_line = clamped as usize;
}

fn point_in_rect(x: u16, y: u16, rect: ratatui::layout::Rect) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

fn map_agent_console_mouse_with_swarm(
    swarm: &SwarmRuntime,
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    let layout = layout::split(screen);
    let text_area = agent_console_view::thread_text_area(layout.notes, state)?;
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = agent_console_view::thread_lines_for_selection_with_swarm(
        state,
        swarm,
        text_area.width as usize,
    );
    if lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let total = lines.len();
    let max_scroll = total.saturating_sub(height);
    let scroll = state.agents.console_scroll.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, lines))
}

fn map_job_output_mouse_with_swarm(
    swarm: &SwarmRuntime,
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    clamp: bool,
) -> Option<(usize, usize, usize, Vec<String>)> {
    let layout = layout::split(screen);
    let text_area = job_output_text_area(layout.job);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let text_width = text_area.width as usize;
    let lines = agent_ops_view::current_lines_for_width_with_swarm(state, Some(swarm), text_width);
    if lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let total = lines.len();
    let max_scroll = total.saturating_sub(height);
    let scroll = state.agents.ops_scroll.min(max_scroll);
    let start = scroll;
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &lines,
        start,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_width, lines))
}

fn map_help_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if !state.show_help {
        return None;
    }
    let area = dynamic_popup_rect(screen, help_overlay::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = help_overlay::build_lines(theme);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.help_scroll.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

fn map_artifacts_popup_mouse_with_swarm(
    swarm: &SwarmRuntime,
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if !state.agents.artifacts_popup_open {
        return None;
    }
    let area = dynamic_popup_rect(screen, artifacts_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = artifacts_popup::build_lines(state, swarm, theme, text_area.width);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.agents.artifacts_popup_scroll.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

fn map_artifacts_history_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if !state.agents.artifacts_history_popup_open {
        return None;
    }
    let area = dynamic_popup_rect(screen, artifacts_history_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = artifacts_history_popup::build_lines(state, theme, text_area.width);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.agents.artifacts_history_popup_scroll.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

fn maybe_open_artifacts_popup_url_on_click(
    swarm: &SwarmRuntime,
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    input_state: &InputState,
    theme: &Theme,
) {
    if !state.agents.artifacts_popup_open {
        return;
    }
    let Some(anchor) = input_state.mouse_select_anchor else {
        return;
    };
    if !matches!(
        anchor.target,
        MouseSelectTarget::Ui(UiSelectionPane::ArtifactsPopup)
    ) {
        return;
    }

    let Some(selection) = state.ui_selection else {
        return;
    };
    if selection.pane != UiSelectionPane::ArtifactsPopup {
        return;
    }
    if selection.start_line != selection.end_line || selection.start_col != selection.end_col {
        // Drag selection: never open.
        return;
    }
    if selection.start_line != anchor.line || selection.start_col != anchor.col {
        return;
    }

    let Some((line_idx, col, lines)) =
        map_artifacts_popup_mouse_with_swarm(swarm, mouse, screen, state, theme, false)
    else {
        return;
    };
    if line_idx != anchor.line || col != anchor.col {
        return;
    }

    let Some(url) = http_url_at_line_col(&lines, line_idx, col) else {
        return;
    };
    match open_url_in_browser(&url) {
        Ok(()) => {
            state.status = Some(format!("Opened {url}"));
        }
        Err(err) => {
            state.status = Some(format!("Open URL failed: {err}"));
        }
    }
}

fn open_url_in_browser(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("empty url".into());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("url must start with http:// or https://".into());
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = url;
        Err("unsupported platform".into())
    }
}

fn http_url_at_line_col(lines: &[String], line_idx: usize, col: usize) -> Option<String> {
    let line = lines.get(line_idx)?.as_str();
    let (start, end) = token_bounds_at_col(line, col)?;
    let token = slice_by_char(line, start, end);
    if let Some(url) = normalize_http_url(&token) {
        return Some(url);
    }

    // Best-effort: if a long URL was wrapped mid-token, stitch contiguous chunks from adjacent
    // lines (where the token touches the line boundary) and then re-scan.
    let mut blob = token;
    let mut start_line = line_idx;
    let mut end_line = line_idx;
    let mut token_start = start;
    let mut token_end = end;

    for _ in 0..8 {
        if start_line == 0 {
            break;
        }
        let current = lines.get(start_line)?.as_str();
        let first_nonspace = current.chars().take_while(|ch| ch.is_whitespace()).count();
        if token_start > first_nonspace {
            break;
        }
        let prev = lines.get(start_line.saturating_sub(1))?.as_str();
        let (prev_token, prev_start, prev_end) = last_token(prev)?;
        let prev_trim_len = prev
            .trim_end_matches(|ch: char| ch.is_whitespace())
            .chars()
            .count();
        if prev_end != prev_trim_len {
            break;
        }
        if !looks_like_url_token(prev_token.as_str()) {
            break;
        }
        blob = format!("{prev_token}{blob}");
        start_line = start_line.saturating_sub(1);
        token_start = prev_start;
    }

    for _ in 0..8 {
        let current = lines.get(end_line)?.as_str();
        let trim_len = current
            .trim_end_matches(|ch: char| ch.is_whitespace())
            .chars()
            .count();
        if token_end < trim_len {
            break;
        }
        let next_line = end_line.saturating_add(1);
        if next_line >= lines.len() {
            break;
        }
        let next = lines.get(next_line)?.as_str();
        let (next_token, _next_start, next_end) = first_token(next)?;
        if !looks_like_url_token(next_token.as_str()) {
            break;
        }
        blob = format!("{blob}{next_token}");
        end_line = next_line;
        token_end = next_end;
    }

    normalize_http_url(&blob)
}

fn token_bounds_at_col(line: &str, col: usize) -> Option<(usize, usize)> {
    let chars = line.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return None;
    }
    let mut pos = col.min(chars.len());
    if pos == chars.len() && pos > 0 {
        pos = pos.saturating_sub(1);
    }
    while pos > 0 && chars[pos].is_whitespace() {
        pos = pos.saturating_sub(1);
    }
    if chars[pos].is_whitespace() {
        return None;
    }
    let mut start = pos;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start = start.saturating_sub(1);
    }
    let mut end = pos.saturating_add(1);
    while end < chars.len() && !chars[end].is_whitespace() {
        end = end.saturating_add(1);
    }
    Some((start, end))
}

fn first_token(line: &str) -> Option<(String, usize, usize)> {
    let len = line.chars().count();
    if len == 0 {
        return None;
    }
    let start = line.chars().take_while(|ch| ch.is_whitespace()).count();
    if start >= len {
        return None;
    }
    let mut end = start;
    for (idx, ch) in line.chars().enumerate().skip(start) {
        if ch.is_whitespace() {
            break;
        }
        end = idx.saturating_add(1);
    }
    let token = slice_by_char(line, start, end);
    Some((token, start, end))
}

fn last_token(line: &str) -> Option<(String, usize, usize)> {
    let trimmed = line.trim_end_matches(|ch: char| ch.is_whitespace());
    let len = trimmed.chars().count();
    if len == 0 {
        return None;
    }
    let end = len;
    let mut start = end.saturating_sub(1);
    let chars = trimmed.chars().collect::<Vec<_>>();
    while start > 0 && !chars[start - 1].is_whitespace() {
        start = start.saturating_sub(1);
    }
    let token = slice_by_char(trimmed, start, end);
    Some((token, start, end))
}

fn looks_like_url_token(token: &str) -> bool {
    let token = token.trim_matches(|ch: char| matches!(ch, '`' | '<' | '>' | '"' | '\''));
    if token.is_empty() {
        return false;
    }
    token.chars().all(|ch| {
        ch.is_ascii_alphanumeric()
            || matches!(
                ch,
                ':' | '/'
                    | '?'
                    | '#'
                    | '['
                    | ']'
                    | '@'
                    | '!'
                    | '$'
                    | '&'
                    | '\''
                    | '('
                    | ')'
                    | '*'
                    | '+'
                    | ','
                    | ';'
                    | '='
                    | '.'
                    | '_'
                    | '-'
                    | '~'
                    | '%'
            )
    })
}

fn normalize_http_url(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let https = text.find("https://");
    let http = text.find("http://");
    let start = match (https, http) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }?;
    let mut url = &text[start..];
    url = url.trim_matches(|ch: char| matches!(ch, '`' | '<' | '>' | '"' | '\''));
    url = url.trim_end_matches(['.', ',', ';', ':', ')', ']', '}']);
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url.to_string())
    } else {
        None
    }
}

fn map_analysis_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.analysis.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_analysis_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = games_analysis_popup::build_lines(state, theme, text_area.width);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.games.analysis.scroll_offset.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

fn map_run_browser_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.run_browser.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_run_browser_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = games_run_browser_popup::build_lines(state, theme, text_area.width);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.games.run_browser.scroll_offset.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

fn map_replay_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.replay.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_replay_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = games_replay_popup::build_lines(state, theme, text_area.width);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.games.replay.scroll_offset.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

fn map_strategy_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.strategy_inspect.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_strategy_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = games_strategy_popup::build_lines(state, theme, text_area.width);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.games.strategy_inspect.scroll_offset.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

fn map_tm_sim_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(UiSelectionPane, usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.tm_sim.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_tm_sim_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let (_left_area, right_area) = games_tm_sim_popup::layout_for_tm_sim(text_area);
    let right_inner = right_area.map(|area| Block::default().borders(Borders::ALL).inner(area));
    let pane = if let Some(right_inner) = right_inner {
        if point_in_rect(mouse.column, mouse.row, right_inner) {
            UiSelectionPane::GamesTmSimPopupRight
        } else {
            UiSelectionPane::GamesTmSimPopupLeft
        }
    } else {
        UiSelectionPane::GamesTmSimPopupLeft
    };
    let (line_idx, col, lines) =
        map_tm_sim_popup_mouse_for_pane(mouse, screen, state, theme, clamp, pane)?;
    Some((pane, line_idx, col, lines))
}

fn map_tm_sim_popup_mouse_for_pane(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
    pane: UiSelectionPane,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.tm_sim.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_tm_sim_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let (left_area, right_area) = games_tm_sim_popup::layout_for_tm_sim(text_area);
    let right_inner = right_area.map(|area| Block::default().borders(Borders::ALL).inner(area));
    let (target_area, lines) = match pane {
        UiSelectionPane::GamesTmSimPopupRight => {
            let right_inner = right_inner?;
            let right_width = right_inner.width.max(1) as usize;
            let (_left_lines, right_lines) = games_tm_sim_popup::build_columns(
                state,
                theme,
                left_area.width.max(1) as usize,
                right_width,
            );
            (right_inner, right_lines)
        }
        _ => {
            let right_width = right_inner
                .map(|area| area.width.max(1) as usize)
                .unwrap_or(0);
            let (left_lines, _right_lines) = games_tm_sim_popup::build_columns(
                state,
                theme,
                left_area.width.max(1) as usize,
                right_width,
            );
            (left_area, left_lines)
        }
    };
    if !point_in_rect(mouse.column, mouse.row, target_area) && !clamp {
        return None;
    }
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = target_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.games.tm_sim.scroll_offset.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        target_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

fn map_ca_sim_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(UiSelectionPane, usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.ca_sim.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_ca_sim_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let (_left_area, right_area) = games_ca_sim_popup::layout_for_ca_sim(text_area);
    let right_inner = right_area.map(|area| Block::default().borders(Borders::ALL).inner(area));
    let pane = if let Some(right_inner) = right_inner {
        if point_in_rect(mouse.column, mouse.row, right_inner) {
            UiSelectionPane::GamesCaSimPopupRight
        } else {
            UiSelectionPane::GamesCaSimPopupLeft
        }
    } else {
        UiSelectionPane::GamesCaSimPopupLeft
    };
    let (line_idx, col, lines) =
        map_ca_sim_popup_mouse_for_pane(mouse, screen, state, theme, clamp, pane)?;
    Some((pane, line_idx, col, lines))
}

fn map_ca_sim_popup_mouse_for_pane(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
    pane: UiSelectionPane,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.ca_sim.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_ca_sim_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let (left_area, right_area) = games_ca_sim_popup::layout_for_ca_sim(text_area);
    let right_inner = right_area.map(|area| Block::default().borders(Borders::ALL).inner(area));
    let (target_area, lines) = match pane {
        UiSelectionPane::GamesCaSimPopupRight => {
            let right_inner = right_inner?;
            let right_width = right_inner.width.max(1) as usize;
            let (_left_lines, right_lines) = games_ca_sim_popup::build_columns(
                state,
                theme,
                left_area.width.max(1) as usize,
                right_width,
            );
            (right_inner, right_lines)
        }
        _ => {
            let right_width = right_inner
                .map(|area| area.width.max(1) as usize)
                .unwrap_or(0);
            let (left_lines, _right_lines) = games_ca_sim_popup::build_columns(
                state,
                theme,
                left_area.width.max(1) as usize,
                right_width,
            );
            (left_area, left_lines)
        }
    };
    if !point_in_rect(mouse.column, mouse.row, target_area) && !clamp {
        return None;
    }
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = target_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.games.ca_sim.scroll_offset.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        target_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

fn map_match_history_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.match_history.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_match_history_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = games_match_history_popup::build_lines(state, theme, text_area);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        0,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

fn map_games_petri_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if !games_petri_visible(state) {
        return None;
    }
    let area = crate::games_petri_dish::petri_rect(screen);
    let text_area = Block::default().borders(Borders::ALL).inner(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = state.games.petri_lines.clone();
    if lines.is_empty() {
        return None;
    }
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &lines,
        0,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, lines))
}

fn map_visualizer_main_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games {
        return None;
    }
    let layout = layout::split(screen);
    let inner = Block::default()
        .borders(Borders::ALL)
        .inner(layout.visualizer);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let config_result = current_games_config_result(state);
    let layout_info = games_visualizer_view::layout_for_config(
        inner,
        state,
        config_result.and_then(|result| result.as_ref().ok()),
    );
    let area = layout_info.main;
    if !point_in_rect(mouse.column, mouse.row, area) && !clamp {
        return None;
    }
    let lines = games_visualizer_view::build_main_lines(
        state,
        theme,
        config_result,
        state.games.config_preview_pending,
        layout_info.show_payoff_side,
        area.width as usize,
    );
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        area,
        &text_lines,
        0,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

fn map_visualizer_side_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games {
        return None;
    }
    let layout = layout::split(screen);
    let inner = Block::default()
        .borders(Borders::ALL)
        .inner(layout.visualizer);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let config_result = current_games_config_result(state);
    let layout_info = games_visualizer_view::layout_for_config(
        inner,
        state,
        config_result.and_then(|result| result.as_ref().ok()),
    );
    let side_area = layout_info.side?;
    let side_inner = Block::default().borders(Borders::ALL).inner(side_area);
    if side_inner.width == 0 || side_inner.height == 0 {
        return None;
    }
    if !point_in_rect(mouse.column, mouse.row, side_inner) && !clamp {
        return None;
    }
    let lines = games_visualizer_view::build_side_lines(state, theme, side_inner.width as usize);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        side_inner,
        &text_lines,
        0,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

fn map_gate_monitor_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    let layout = layout::split(screen);
    let inner = Block::default().borders(Borders::ALL).inner(layout.gate);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    if !point_in_rect(mouse.column, mouse.row, inner) && !clamp {
        return None;
    }
    let lines = gate_monitor_view::build_lines(state, theme, inner.width as usize);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        inner,
        &text_lines,
        0,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

fn map_mouse_to_line_col(
    mouse: MouseEvent,
    area: ratatui::layout::Rect,
    lines: &[String],
    scroll: usize,
    tab_width: usize,
    clamp: bool,
) -> Option<(usize, usize)> {
    if lines.is_empty() || area.height == 0 || area.width == 0 {
        return None;
    }
    let max_row = area.height.saturating_sub(1);
    let row = if clamp {
        if mouse.row < area.y {
            0
        } else {
            mouse.row.saturating_sub(area.y).min(max_row) as usize
        }
    } else if mouse.row < area.y || mouse.row > area.y.saturating_add(max_row) {
        return None;
    } else {
        mouse.row.saturating_sub(area.y) as usize
    };
    let line_idx = scroll
        .saturating_add(row)
        .min(lines.len().saturating_sub(1));
    let line = &lines[line_idx];
    let display_col = if mouse.column <= area.x {
        0
    } else {
        (mouse.column - area.x) as usize
    };
    let col = char_idx_for_display_col(line, display_col, tab_width);
    Some((line_idx, col))
}

fn job_output_text_area(area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::widgets::{Block, Borders};
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        // Keep in sync with Agent Ops layout: tabs + spacer + body + footer hints.
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    chunks[2]
}

fn agent_ops_tab_bar_area(area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    use ratatui::widgets::{Block, Borders};
    let inner = Block::default().borders(Borders::ALL).inner(area);
    ratatui::layout::Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height.min(1),
    }
}

fn agent_ops_scratchpad_editor_area(area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::widgets::{Block, Borders};
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        // Keep in sync with Agent Ops Scratchpad layout: tabs + editor body.
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);
    chunks[1]
}

fn popup_text_area(area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    use ratatui::widgets::{Block, Borders};
    Block::default().borders(Borders::ALL).inner(area)
}

fn lines_to_strings(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

fn update_ui_selection_text(
    state: &mut AppState,
    pane: UiSelectionPane,
    lines: &[String],
    clipboard: &mut Option<Clipboard>,
    input_state: &mut InputState,
) {
    let Some(selection) = state.ui_selection else {
        return;
    };
    if selection.pane != pane {
        return;
    }
    let signature = UiSelectionSignature {
        pane,
        start_line: selection.start_line,
        start_col: selection.start_col,
        end_line: selection.end_line,
        end_col: selection.end_col,
    };
    if input_state.last_ui_selection == Some(signature) {
        return;
    }
    input_state.last_ui_selection = Some(signature);
    let text = if matches!(pane, UiSelectionPane::AgentConsole) {
        selection_text_agent_console(lines, selection)
    } else {
        selection_text(lines, selection)
    };
    if text.is_empty() {
        return;
    }
    state.yank_kind = if text.contains('\n') {
        YankKind::Line
    } else {
        YankKind::Char
    };
    state.yank = Some(text.clone());
    if let Some(cb) = clipboard.as_mut() {
        let _ = cb.set_text(text);
    }
}

fn selection_text(lines: &[String], selection: UiSelection) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let (start_line, start_col, end_line, end_col) =
        if (selection.start_line, selection.start_col) <= (selection.end_line, selection.end_col) {
            (
                selection.start_line,
                selection.start_col,
                selection.end_line,
                selection.end_col,
            )
        } else {
            (
                selection.end_line,
                selection.end_col,
                selection.start_line,
                selection.start_col,
            )
        };
    let mut out = String::new();
    let last_line = lines.len().saturating_sub(1);
    let end_line = end_line.min(last_line);
    for (line_idx, line) in lines
        .iter()
        .enumerate()
        .take(end_line.saturating_add(1))
        .skip(start_line)
    {
        let line_len = line.chars().count();
        let sel_start = if line_idx == start_line { start_col } else { 0 };
        let sel_end = if line_idx == end_line {
            end_col
        } else {
            line_len
        };
        let sel_start = sel_start.min(line_len);
        let sel_end = sel_end.min(line_len);
        out.push_str(&slice_by_char(line, sel_start, sel_end));
        if line_idx < end_line {
            out.push('\n');
        }
    }
    out
}

fn selection_text_agent_console(lines: &[String], selection: UiSelection) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let (start_line, start_col, end_line, end_col) =
        if (selection.start_line, selection.start_col) <= (selection.end_line, selection.end_col) {
            (
                selection.start_line,
                selection.start_col,
                selection.end_line,
                selection.end_col,
            )
        } else {
            (
                selection.end_line,
                selection.end_col,
                selection.start_line,
                selection.start_col,
            )
        };
    let last_line = lines.len().saturating_sub(1);
    let end_line = end_line.min(last_line);
    let mut out_lines = Vec::new();
    for line_idx in start_line..=end_line {
        let line = &lines[line_idx];
        let line_len = line.chars().count();
        let mut sel_start = if line_idx == start_line { start_col } else { 0 };
        let mut sel_end = if line_idx == end_line {
            end_col
        } else {
            line_len
        };
        sel_start = sel_start.min(line_len);
        sel_end = sel_end.min(line_len);
        let slice = if let Some((payload_start, payload_end)) =
            user_prompt_payload_bounds_in_block(lines, line_idx)
        {
            let sel_start = sel_start.max(payload_start);
            let sel_end = sel_end.min(payload_end);
            slice_by_char(line, sel_start, sel_end)
                .trim_end_matches(' ')
                .to_string()
        } else {
            slice_by_char(line, sel_start, sel_end)
        };
        out_lines.push(slice);
    }
    out_lines.join("\n")
}

const USER_PROMPT_INDENT: usize = 2;

fn is_user_prompt_row(line: &str) -> bool {
    // User prompts are padded out to the full thread width so the background fills the row.
    // That makes them easy to detect for clipboard trimming: they start with the fixed indent and
    // end with spaces.
    line.starts_with("  ") && line.ends_with(' ')
}

fn user_prompt_payload_bounds_in_block(lines: &[String], idx: usize) -> Option<(usize, usize)> {
    let line = lines.get(idx)?;
    if !is_user_prompt_row(line) {
        return None;
    }

    // Find the contiguous block of padded user rows that this line belongs to.
    let mut start = idx;
    while start > 0 && is_user_prompt_row(&lines[start - 1]) {
        start = start.saturating_sub(1);
    }
    let mut end = idx;
    while end + 1 < lines.len() && is_user_prompt_row(&lines[end + 1]) {
        end = end.saturating_add(1);
    }

    // Only treat the block as a user prompt if it contains the "You" label line.
    let has_label = (start..=end).any(|line_idx| lines[line_idx].trim() == "You");
    if !has_label {
        return None;
    }

    let len = line.chars().count();
    let start_col = USER_PROMPT_INDENT.min(len);
    Some((start_col, len))
}

fn handle_mouse_down_with_swarm(
    swarm: &SwarmRuntime,
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    input_state: &mut InputState,
    clipboard: &mut Option<Clipboard>,
    theme: &Theme,
) -> bool {
    if state.command_line.is_some() || state.prompt.is_some() {
        return true;
    }
    if state.rule_picker.open || state.protocol_picker.open {
        return true;
    }
    if state.agents.artifacts_history_popup_open {
        if let Some((line_idx, col, lines)) =
            map_artifacts_history_popup_mouse(mouse, screen, state, theme, false)
        {
            if let Some(entry_idx) = artifacts_history_popup::entry_index_for_line(state, line_idx)
            {
                clear_artifacts_history_pending_action(state);
                state.agents.artifacts_history_selected = entry_idx;
            }
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::ArtifactsHistoryPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::ArtifactsHistoryPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::ArtifactsHistoryPopup,
                &lines,
                clipboard,
                input_state,
            );
        }
        return true;
    }
    if state.agents.artifacts_popup_open {
        // Check if the click is inside the popup's chat input box first.
        let popup_area = dynamic_popup_rect(screen, artifacts_popup::preferred_size(screen));
        if let Some(cursor_char_idx) = artifacts_popup::map_chat_input_point_to_cursor(
            state,
            swarm,
            popup_area,
            mouse.column,
            mouse.row,
            false,
        ) {
            reset_ui_selection(state, input_state);
            let total_chars = state.agents.artifacts_popup_chat_input.chars().count();
            let new_cursor = cursor_char_idx.min(total_chars);
            if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                if state.agents.artifacts_popup_chat_selection_anchor.is_none() {
                    state.agents.artifacts_popup_chat_selection_anchor = Some(
                        state
                            .agents
                            .artifacts_popup_chat_cursor
                            .min(total_chars),
                    );
                }
            } else {
                state.agents.artifacts_popup_chat_selection_anchor = Some(new_cursor);
            }
            state.agents.artifacts_popup_chat_cursor = new_cursor;
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::PopupChatInput,
                line: 0,
                col: 0,
            });
            copy_popup_chat_input_selection(state, clipboard);
        } else if let Some((line_idx, col, lines)) =
            map_artifacts_popup_mouse_with_swarm(swarm, mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::ArtifactsPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::ArtifactsPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::ArtifactsPopup,
                &lines,
                clipboard,
                input_state,
            );
        } else if !point_in_rect(mouse.column, mouse.row, popup_area) {
            reset_ui_selection(state, input_state);
            state.agents.artifacts_popup_open = false;
            state.agents.artifacts_popup_scroll = 0;
        }
        return true;
    }
    if state.show_help {
        if let Some((line_idx, col, lines)) =
            map_help_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::HelpPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::HelpPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::HelpPopup,
                &lines,
                clipboard,
                input_state,
            );
        }
        return true;
    }
    if state.app_kind == AppKind::Games && state.games.run_browser.open {
        if let Some((line_idx, col, lines)) =
            map_run_browser_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::GamesRunBrowserPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::GamesRunBrowserPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::GamesRunBrowserPopup,
                &lines,
                clipboard,
                input_state,
            );
        }
        return true;
    }
    if state.app_kind == AppKind::Games && state.games.replay.open {
        if let Some((line_idx, col, lines)) =
            map_replay_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::GamesReplayPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::GamesReplayPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::GamesReplayPopup,
                &lines,
                clipboard,
                input_state,
            );
        }
        return true;
    }
    if state.app_kind == AppKind::Games && state.games.strategy_inspect.open {
        if let Some((line_idx, col, lines)) =
            map_strategy_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::GamesStrategyPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::GamesStrategyPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::GamesStrategyPopup,
                &lines,
                clipboard,
                input_state,
            );
        }
        return true;
    }
    if state.app_kind == AppKind::Games && state.games.tm_sim.open {
        if let Some((pane, line_idx, col, lines)) =
            map_tm_sim_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(pane),
                line: line_idx,
                col,
            });
            update_ui_selection_text(state, pane, &lines, clipboard, input_state);
        }
        return true;
    }
    if state.app_kind == AppKind::Games && state.games.ca_sim.open {
        if let Some((pane, line_idx, col, lines)) =
            map_ca_sim_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(pane),
                line: line_idx,
                col,
            });
            update_ui_selection_text(state, pane, &lines, clipboard, input_state);
        }
        return true;
    }
    if state.app_kind == AppKind::Games && state.games.match_history.open {
        if let Some((line_idx, col, lines)) =
            map_match_history_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::GamesMatchHistoryPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::GamesMatchHistoryPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::GamesMatchHistoryPopup,
                &lines,
                clipboard,
                input_state,
            );
        }
        return true;
    }
    if state.app_kind == AppKind::Games && state.games.analysis.open {
        if let Some((line_idx, col, lines)) =
            map_analysis_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::GamesAnalysisPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::GamesAnalysisPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::GamesAnalysisPopup,
                &lines,
                clipboard,
                input_state,
            );
        }
        return true;
    }
    if games_petri_visible(state) {
        if let Some((line_idx, col, lines)) = map_games_petri_mouse(mouse, screen, state, false) {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::GamesPetriDish,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::GamesPetriDish),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::GamesPetriDish,
                &lines,
                clipboard,
                input_state,
            );
        }
        return true;
    }
    reset_ui_selection(state, input_state);
    let layout = layout::split(screen);
    if point_in_rect(mouse.column, mouse.row, layout.editor) {
        set_buffer_cursor_from_mouse(
            state,
            PaneId::Editor,
            mouse,
            layout.editor,
            state.settings.editor.tab_width as usize,
            false,
        );
        if state.mode == Mode::Visual {
            state.mode = Mode::Normal;
        }
        state.editor_buffer_mut().clear_selection();
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::Buffer(PaneId::Editor),
            line: state.editor_buffer().cursor.line,
            col: state.editor_buffer().cursor.col,
        });
        return true;
    }
    if let Some(cursor_char_idx) = agent_console_view::map_chat_input_point_to_cursor(
        layout.notes,
        state,
        mouse.column,
        mouse.row,
        false,
    ) {
        reset_ui_selection(state, input_state);
        state.focus = PaneId::Notes;
        state.mode = Mode::Normal;
        let total_chars = state.agents.chat_input.chars().count();
        let new_cursor = cursor_char_idx.min(total_chars);
        if mouse.modifiers.contains(KeyModifiers::SHIFT) {
            if state.agents.chat_input_selection_anchor.is_none() {
                state.agents.chat_input_selection_anchor =
                    Some(state.agents.chat_input_cursor.min(total_chars));
            }
        } else {
            state.agents.chat_input_selection_anchor = Some(new_cursor);
        }
        state.agents.chat_input_cursor = new_cursor;
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::ChatInput,
            line: 0,
            col: 0,
        });
        copy_chat_input_selection(state, clipboard);
        return true;
    }
    if let Some((line_idx, col, lines)) =
        map_agent_console_mouse_with_swarm(swarm, mouse, screen, state, false)
    {
        state.focus = PaneId::Notes;
        state.mode = Mode::Normal;
        state.agents.chat_input_selection_anchor = None;
        if let Some(text_area) = agent_console_view::thread_text_area(layout.notes, state) {
            if maybe_open_artifact_popup_from_console_line(
                state,
                swarm,
                text_area.width as usize,
                line_idx,
            ) {
                input_state.mouse_select_anchor = None;
                return true;
            }
        }
        state.ui_selection = Some(UiSelection {
            pane: UiSelectionPane::AgentConsole,
            start_line: line_idx,
            start_col: col,
            end_line: line_idx,
            end_col: col,
        });
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::Ui(UiSelectionPane::AgentConsole),
            line: line_idx,
            col,
        });
        update_ui_selection_text(
            state,
            UiSelectionPane::AgentConsole,
            &lines,
            clipboard,
            input_state,
        );
        return true;
    }
    if point_in_rect(mouse.column, mouse.row, layout.notes) {
        reset_ui_selection(state, input_state);
        state.focus = PaneId::Notes;
        state.mode = Mode::Normal;
        state.agents.chat_input_selection_anchor = None;
        input_state.mouse_select_anchor = None;
        return true;
    }
    let agent_ops_tabs_area = agent_ops_tab_bar_area(layout.job);
    if point_in_rect(mouse.column, mouse.row, agent_ops_tabs_area) {
        reset_ui_selection(state, input_state);
        state.focus = PaneId::JobOutput;
        let rel_col = mouse.column.saturating_sub(agent_ops_tabs_area.x) as usize;
        if let Some(tab) = agent_ops_view::tab_at_column(rel_col) {
            if state.agents.dock_tab != tab {
                state.agents.dock_tab = tab;
                state.agents.roster_tree_selected = None;
                state.agents.ops_scroll = 0;
            }
            state.mode = if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                Mode::Insert
            } else {
                Mode::Normal
            };
            state.agents.note_event();
        }
        input_state.mouse_select_anchor = None;
        return true;
    }
    let scratchpad_area = agent_ops_scratchpad_editor_area(layout.job);
    if point_in_rect(mouse.column, mouse.row, scratchpad_area)
        && state.agents.dock_tab == AgentOpsTab::Scratchpad
    {
        set_buffer_cursor_from_mouse(
            state,
            PaneId::JobOutput,
            mouse,
            scratchpad_area,
            state.settings.editor.tab_width as usize,
            false,
        );
        if state.mode == Mode::Visual {
            state.mode = Mode::Normal;
        }
        state.notes_buffer_mut().clear_selection();
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::Buffer(PaneId::JobOutput),
            line: state.notes_buffer().cursor.line,
            col: state.notes_buffer().cursor.col,
        });
        return true;
    }
    if point_in_rect(mouse.column, mouse.row, layout.job)
        && state.agents.dock_tab == AgentOpsTab::Scratchpad
    {
        state.focus = PaneId::JobOutput;
        state.mode = Mode::Insert;
        input_state.mouse_select_anchor = None;
        return true;
    }
    if let Some((line_idx, col, lines)) =
        map_visualizer_side_mouse(mouse, screen, state, theme, false)
    {
        state.focus = PaneId::Visualizer;
        state.ui_selection = Some(UiSelection {
            pane: UiSelectionPane::VisualizerSide,
            start_line: line_idx,
            start_col: col,
            end_line: line_idx,
            end_col: col,
        });
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::Ui(UiSelectionPane::VisualizerSide),
            line: line_idx,
            col,
        });
        update_ui_selection_text(
            state,
            UiSelectionPane::VisualizerSide,
            &lines,
            clipboard,
            input_state,
        );
        return true;
    }
    if let Some((line_idx, col, lines)) =
        map_visualizer_main_mouse(mouse, screen, state, theme, false)
    {
        state.focus = PaneId::Visualizer;
        state.ui_selection = Some(UiSelection {
            pane: UiSelectionPane::VisualizerMain,
            start_line: line_idx,
            start_col: col,
            end_line: line_idx,
            end_col: col,
        });
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::Ui(UiSelectionPane::VisualizerMain),
            line: line_idx,
            col,
        });
        update_ui_selection_text(
            state,
            UiSelectionPane::VisualizerMain,
            &lines,
            clipboard,
            input_state,
        );
        return true;
    }
    if let Some((line_idx, col, lines)) = map_gate_monitor_mouse(mouse, screen, state, theme, false)
    {
        state.focus = PaneId::GateMonitor;
        state.ui_selection = Some(UiSelection {
            pane: UiSelectionPane::GateMonitor,
            start_line: line_idx,
            start_col: col,
            end_line: line_idx,
            end_col: col,
        });
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::Ui(UiSelectionPane::GateMonitor),
            line: line_idx,
            col,
        });
        update_ui_selection_text(
            state,
            UiSelectionPane::GateMonitor,
            &lines,
            clipboard,
            input_state,
        );
        return true;
    }
    if let Some((line_idx, col, text_width, lines)) =
        map_job_output_mouse_with_swarm(swarm, mouse, screen, state, false)
    {
        state.focus = PaneId::JobOutput;
        apply_agent_ops_click_selection(state, line_idx, col, text_width, &lines);
        if state.agents.dock_tab == AgentOpsTab::Scratchpad {
            state.mode = Mode::Insert;
        }
        state.ui_selection = Some(UiSelection {
            pane: UiSelectionPane::JobOutput,
            start_line: line_idx,
            start_col: col,
            end_line: line_idx,
            end_col: col,
        });
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::Ui(UiSelectionPane::JobOutput),
            line: line_idx,
            col,
        });
        update_ui_selection_text(
            state,
            UiSelectionPane::JobOutput,
            &lines,
            clipboard,
            input_state,
        );
        return true;
    }
    false
}

fn apply_agent_ops_click_selection(
    state: &mut AppState,
    line_idx: usize,
    col: usize,
    text_width: usize,
    lines: &[String],
) {
    let offset = match state.agents.dock_tab {
        AgentOpsTab::Roster => agent_ops_view::roster_body_offset(state),
        _ => 2,
    };
    if state.agents.dock_tab == AgentOpsTab::Roster
        && line_idx == agent_ops_view::roster_swarm_template_line_idx(state)
    {
        if let Some(template) = agent_ops_view::roster_swarm_template_hit(col) {
            state.agents.swarm_default_template = template.to_string();
            state.agents.roster_tree_selected = None;
        }
        return;
    }
    if state.agents.dock_tab == AgentOpsTab::Roster
        && line_idx == agent_ops_view::roster_swarm_mission_line_idx(state)
    {
        if let Some(mission) = agent_ops_view::roster_swarm_mission_hit(col) {
            state.agents.swarm_default_mission = mission.to_string();
            state.agents.roster_tree_selected = None;
        }
        return;
    }
    if line_idx < offset {
        return;
    }
    let data_line = line_idx.saturating_sub(offset);
    match state.agents.dock_tab {
        AgentOpsTab::Roster => {
            let Some(meta) = agent_ops_view::roster_meta_for_body_line(state, data_line) else {
                return;
            };
            if let agent_ops_view::RosterBodyNode::Backend { backend } = meta.node {
                select_roster_backend(state, backend);
                let _ = toggle_roster_backend_expanded(state, backend);
                return;
            }
            let Some(agent_idx) = meta.agent_idx else {
                return;
            };
            // Clicking the roster priority checkbox should NOT change selection or expand/collapse.
            if matches!(meta.node, agent_ops_view::RosterBodyNode::Agent) {
                if let Some(agent) = state.agents.agents.get(agent_idx) {
                    let checkbox_hit = agent.supports_swarm_priority()
                        && !agent.id.contains("#swarm-")
                        && (1..5).contains(&col);
                    if checkbox_hit {
                        if state.agents.swarm_priority_agent_ids.remove(&agent.id) {
                            // removed
                        } else {
                            state
                                .agents
                                .swarm_priority_agent_ids
                                .insert(agent.id.clone());
                        }
                        return;
                    }
                }
            }

            let was_selected = agent_idx == state.agents.roster_selected;
            sync_roster_selected_agent(state, agent_idx);
            if let Some(agent) = state.agents.agents.get(agent_idx) {
                match meta.node {
                    agent_ops_view::RosterBodyNode::Agent => {
                        let model_hit = agent_ops_view::roster_role_cell_hit(col, text_width);
                        if model_hit && !was_selected {
                            state
                                .agents
                                .roster_tree_collapsed_agent_ids
                                .remove(&agent.id);
                        } else if was_selected
                            && model_hit
                            && !state
                                .agents
                                .roster_tree_collapsed_agent_ids
                                .remove(&agent.id)
                        {
                            state
                                .agents
                                .roster_tree_collapsed_agent_ids
                                .insert(agent.id.clone());
                        }
                    }
                    agent_ops_view::RosterBodyNode::Branch { branch } => {
                        let leaf_idx = match branch {
                            nit_core::RosterTreeBranch::Size => {
                                let efforts = state
                                    .agents
                                    .codex_supported_reasoning_efforts
                                    .get(&agent.id)
                                    .map(|v| v.as_slice())
                                    .unwrap_or(&[]);
                                let current = state
                                    .agents
                                    .codex_selected_reasoning_effort
                                    .get(&agent.id)
                                    .or_else(|| {
                                        state.agents.codex_default_reasoning_effort.get(&agent.id)
                                    })
                                    .map(|s| s.as_str());
                                current
                                    .and_then(|effort| efforts.iter().position(|e| e == effort))
                                    .unwrap_or(0)
                                    .min(efforts.len().saturating_sub(1))
                            }
                            nit_core::RosterTreeBranch::Role => {
                                let current = state
                                    .agents
                                    .swarm_role_by_agent_id
                                    .get(&agent.id)
                                    .map(|s| s.trim())
                                    .filter(|s| !s.is_empty());
                                current
                                    .and_then(|role| {
                                        let current = normalize_swarm_role_hint_for_roster(role);
                                        SWARM_ROLE_OPTIONS.iter().position(|candidate| {
                                            current
                                                == normalize_swarm_role_hint_for_roster(candidate)
                                        })
                                    })
                                    .unwrap_or(0)
                                    .min(SWARM_ROLE_OPTIONS.len().saturating_sub(1))
                            }
                        };
                        state.agents.roster_tree_selected =
                            Some(nit_core::RosterTreeSelection { branch, leaf_idx });
                    }
                    agent_ops_view::RosterBodyNode::Leaf { branch, leaf_idx } => {
                        state.agents.roster_tree_selected =
                            Some(nit_core::RosterTreeSelection { branch, leaf_idx });
                        let _ = select_roster_tree_leaf(state);
                    }
                    agent_ops_view::RosterBodyNode::Backend { .. } => (),
                }
            }
        }
        AgentOpsTab::Missions => {
            let Some(mission_idx) = agent_ops_view::mission_index_for_body_line(state, data_line)
            else {
                return;
            };
            state.agents.mission_selected = mission_idx;
            if let Some(mission) = state.agents.missions.get(mission_idx) {
                state.agents.selected_mission = Some(mission.id.clone());
            }
        }
        AgentOpsTab::Alerts => {
            let Some(alert_idx) =
                agent_ops_view::alert_index_for_body_line(state, text_width, data_line)
            else {
                return;
            };
            state.agents.alert_selected = alert_idx;
        }
        AgentOpsTab::Evidence => {
            if let Some(card_idx) = agent_ops_view::artifacts_card_index_for_line(lines, line_idx) {
                state.agents.artifacts_selected = card_idx;
                state.agents.artifacts_popup_open = true;
                state.agents.artifacts_popup_scroll = 0;
            }
        }
        AgentOpsTab::Patch
        | AgentOpsTab::Diagnostics
        | AgentOpsTab::Dag
        | AgentOpsTab::Scratchpad => {}
        AgentOpsTab::Mcp => {}
    }
}

fn handle_mouse_drag_with_swarm(
    swarm: &SwarmRuntime,
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    input_state: &mut InputState,
    clipboard: &mut Option<Clipboard>,
    theme: &Theme,
) -> bool {
    let Some(anchor) = input_state.mouse_select_anchor else {
        return false;
    };
    if !mouse_drag_allowed(state, anchor) {
        input_state.mouse_select_anchor = None;
        return true;
    }
    match anchor.target {
        MouseSelectTarget::Buffer(pane) => {
            let layout = layout::split(screen);
            let (pane_rect, tab_width) = match pane {
                PaneId::Editor => (layout.editor, state.settings.editor.tab_width as usize),
                PaneId::JobOutput if state.agents.dock_tab == AgentOpsTab::Scratchpad => (
                    agent_ops_scratchpad_editor_area(layout.job),
                    state.settings.editor.tab_width as usize,
                ),
                _ => return false,
            };
            state.focus = pane;
            let buffer = match pane {
                PaneId::Editor => state.editor_buffer_mut(),
                PaneId::JobOutput if state.agents.dock_tab == AgentOpsTab::Scratchpad => {
                    state.notes_buffer_mut()
                }
                _ => return false,
            };
            let Some((line, col)) = mouse_to_buffer_pos(mouse, pane_rect, buffer, tab_width, true)
            else {
                return false;
            };
            if buffer.selection_range().is_none() {
                buffer.cursor.line = anchor.line;
                buffer.cursor.col = anchor.col;
                buffer.set_selection_anchor();
            }
            buffer.cursor.line = line;
            buffer.cursor.col = col;
            buffer.ensure_visible();
            state.mode = Mode::Visual;
            handle_selection_autocopy(state, clipboard, input_state);
            true
        }
        MouseSelectTarget::ChatInput => {
            let layout = layout::split(screen);
            state.focus = PaneId::Notes;
            state.mode = Mode::Normal;
            let Some(cursor_char_idx) = agent_console_view::map_chat_input_point_to_cursor(
                layout.notes,
                state,
                mouse.column,
                mouse.row,
                true,
            ) else {
                return false;
            };
            let total_chars = state.agents.chat_input.chars().count();
            if state.agents.chat_input_selection_anchor.is_none() {
                state.agents.chat_input_selection_anchor =
                    Some(state.agents.chat_input_cursor.min(total_chars));
            }
            state.agents.chat_input_cursor = cursor_char_idx.min(total_chars);
            state.agents.chat_input_scroll = usize::MAX;
            copy_chat_input_selection(state, clipboard);
            true
        }
        MouseSelectTarget::PopupChatInput => {
            let popup_area =
                dynamic_popup_rect(screen, artifacts_popup::preferred_size(screen));
            let Some(cursor_char_idx) = artifacts_popup::map_chat_input_point_to_cursor(
                state,
                swarm,
                popup_area,
                mouse.column,
                mouse.row,
                true,
            ) else {
                return false;
            };
            let total_chars = state.agents.artifacts_popup_chat_input.chars().count();
            if state.agents.artifacts_popup_chat_selection_anchor.is_none() {
                state.agents.artifacts_popup_chat_selection_anchor = Some(
                    state
                        .agents
                        .artifacts_popup_chat_cursor
                        .min(total_chars),
                );
            }
            state.agents.artifacts_popup_chat_cursor = cursor_char_idx.min(total_chars);
            state.agents.artifacts_popup_chat_scroll = usize::MAX;
            copy_popup_chat_input_selection(state, clipboard);
            true
        }
        MouseSelectTarget::Ui(pane) => {
            let result = match pane {
                UiSelectionPane::JobOutput => {
                    { map_job_output_mouse_with_swarm(swarm, mouse, screen, state, true) }
                        .map(|(line_idx, col, _text_width, lines)| (line_idx, col, lines))
                }
                UiSelectionPane::AgentConsole => {
                    map_agent_console_mouse_with_swarm(swarm, mouse, screen, state, true)
                }
                UiSelectionPane::GamesPetriDish => {
                    map_games_petri_mouse(mouse, screen, state, true)
                }
                UiSelectionPane::VisualizerMain => {
                    map_visualizer_main_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::VisualizerSide => {
                    map_visualizer_side_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::GateMonitor => {
                    map_gate_monitor_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::HelpPopup => {
                    map_help_popup_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::ArtifactsHistoryPopup => {
                    map_artifacts_history_popup_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::ArtifactsPopup => {
                    map_artifacts_popup_mouse_with_swarm(swarm, mouse, screen, state, theme, true)
                }
                UiSelectionPane::GamesAnalysisPopup => {
                    map_analysis_popup_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::GamesRunBrowserPopup => {
                    map_run_browser_popup_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::GamesReplayPopup => {
                    map_replay_popup_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::GamesStrategyPopup => {
                    map_strategy_popup_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::GamesTmSimPopupLeft | UiSelectionPane::GamesTmSimPopupRight => {
                    map_tm_sim_popup_mouse_for_pane(mouse, screen, state, theme, true, pane)
                }
                UiSelectionPane::GamesCaSimPopupLeft | UiSelectionPane::GamesCaSimPopupRight => {
                    map_ca_sim_popup_mouse_for_pane(mouse, screen, state, theme, true, pane)
                }
                UiSelectionPane::GamesMatchHistoryPopup => {
                    map_match_history_popup_mouse(mouse, screen, state, theme, true)
                }
            };
            let Some((line_idx, col, lines)) = result else {
                return false;
            };
            let adjusted_col = if matches!(pane, UiSelectionPane::AgentConsole) {
                adjust_agent_console_drag_col(&lines, anchor.line, line_idx, col)
            } else {
                col
            };
            state.ui_selection = Some(UiSelection {
                pane,
                start_line: anchor.line,
                start_col: anchor.col,
                end_line: line_idx,
                end_col: adjusted_col,
            });
            update_ui_selection_text(state, pane, &lines, clipboard, input_state);
            true
        }
    }
}

fn reset_ui_selection(state: &mut AppState, input_state: &mut InputState) {
    input_state.mouse_select_anchor = None;
    state.ui_selection = None;
    input_state.last_ui_selection = None;
}

fn adjust_agent_console_drag_col(
    lines: &[String],
    anchor_line: usize,
    line_idx: usize,
    col: usize,
) -> usize {
    let Some(line) = lines.get(line_idx) else {
        return col;
    };
    let Some(payload_start) = user_bubble_payload_start_col(line) else {
        return col;
    };
    if col > payload_start {
        return col;
    }
    if line_idx > anchor_line {
        line.chars().count()
    } else if line_idx < anchor_line {
        0
    } else {
        col
    }
}

fn user_bubble_payload_start_col(line: &str) -> Option<usize> {
    is_user_prompt_row(line).then_some(USER_PROMPT_INDENT)
}

fn mouse_drag_allowed(state: &AppState, anchor: MouseSelectAnchor) -> bool {
    if state.command_line.is_some() || state.prompt.is_some() {
        return false;
    }
    if state.rule_picker.open || state.protocol_picker.open {
        return false;
    }
    if state.agents.artifacts_history_popup_open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::ArtifactsHistoryPopup)
        );
    }
    if state.agents.artifacts_popup_open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::ArtifactsPopup)
                | MouseSelectTarget::PopupChatInput
        );
    }
    if state.show_help {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::HelpPopup)
        );
    }
    if state.app_kind == AppKind::Games && state.games.analysis.open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesAnalysisPopup)
        );
    }
    if state.app_kind == AppKind::Games && state.games.run_browser.open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesRunBrowserPopup)
        );
    }
    if state.app_kind == AppKind::Games && state.games.replay.open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesReplayPopup)
        );
    }
    if state.app_kind == AppKind::Games && state.games.strategy_inspect.open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesStrategyPopup)
        );
    }
    if state.app_kind == AppKind::Games && state.games.tm_sim.open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesTmSimPopupLeft)
                | MouseSelectTarget::Ui(UiSelectionPane::GamesTmSimPopupRight)
        );
    }
    if state.app_kind == AppKind::Games && state.games.ca_sim.open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesCaSimPopupLeft)
                | MouseSelectTarget::Ui(UiSelectionPane::GamesCaSimPopupRight)
        );
    }
    if state.app_kind == AppKind::Games && state.games.match_history.open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesMatchHistoryPopup)
        );
    }
    if games_petri_visible(state) {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesPetriDish)
        );
    }
    true
}

fn set_buffer_cursor_from_mouse(
    state: &mut AppState,
    pane: PaneId,
    mouse: MouseEvent,
    area: ratatui::layout::Rect,
    tab_width: usize,
    clamp: bool,
) {
    state.focus = pane;
    let buffer = match pane {
        PaneId::Editor => state.editor_buffer_mut(),
        PaneId::Notes => state.notes_buffer_mut(),
        PaneId::JobOutput if state.agents.dock_tab == AgentOpsTab::Scratchpad => {
            state.notes_buffer_mut()
        }
        _ => return,
    };
    let Some((line, col)) = mouse_to_buffer_pos(mouse, area, buffer, tab_width, clamp) else {
        return;
    };
    buffer.cursor.line = line;
    buffer.cursor.col = col;
    buffer.ensure_visible();
}

fn mouse_to_buffer_pos(
    mouse: MouseEvent,
    area: ratatui::layout::Rect,
    buffer: &nit_core::Buffer,
    tab_width: usize,
    clamp: bool,
) -> Option<(usize, usize)> {
    let inner_x = area.x.saturating_add(1);
    let inner_y = area.y.saturating_add(1);
    let inner_width = area.width.saturating_sub(2) as usize;
    let inner_height = area.height.saturating_sub(2) as usize;
    if inner_width == 0 || inner_height == 0 {
        return None;
    }

    let total_lines = buffer.lines_len().max(1);
    let line_num_width = total_lines.to_string().len().max(3);
    let gutter_width = line_num_width + 4;
    let content_x = inner_x.saturating_add(gutter_width as u16);

    let row = if clamp {
        if mouse.row < inner_y {
            0
        } else {
            let max_row = inner_height.saturating_sub(1) as u16;
            mouse.row.saturating_sub(inner_y).min(max_row) as usize
        }
    } else if mouse.row < inner_y || mouse.row >= inner_y.saturating_add(inner_height as u16) {
        return None;
    } else {
        mouse.row.saturating_sub(inner_y) as usize
    };

    let line_idx = buffer
        .viewport
        .offset_line
        .saturating_add(row)
        .min(total_lines.saturating_sub(1));

    let mut line = buffer.line_as_string(line_idx);
    if line.ends_with('\n') {
        line.pop();
    }

    let display_offset = display_col_for_char_idx(&line, buffer.viewport.offset_col, tab_width);
    let display_col = if mouse.column <= content_x {
        0
    } else {
        (mouse.column - content_x) as usize
    };
    let target_display = display_offset.saturating_add(display_col);
    let col = char_idx_for_display_col(&line, target_display, tab_width);
    Some((line_idx, col))
}

fn display_col_for_char_idx(line: &str, char_idx: usize, tab_width: usize) -> usize {
    let mut col = 0;
    for (count, ch) in line.chars().enumerate() {
        if count >= char_idx {
            break;
        }
        if ch == '\t' {
            let tab = tab_width.max(1);
            let advance = tab - (col % tab);
            col += advance;
        } else {
            let w = unicode_width::UnicodeWidthChar::width(ch)
                .unwrap_or(1)
                .max(1);
            col += w;
        }
    }
    col
}

fn char_idx_for_display_col(line: &str, target_col: usize, tab_width: usize) -> usize {
    let mut col = 0;
    let mut idx = 0;
    for ch in line.chars() {
        let w = if ch == '\t' {
            let tab = tab_width.max(1);
            tab - (col % tab)
        } else {
            unicode_width::UnicodeWidthChar::width(ch)
                .unwrap_or(1)
                .max(1)
        };
        if col + w > target_col {
            break;
        }
        col += w;
        idx += 1;
    }
    idx
}

fn handle_analysis_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> bool {
    if !state.games.analysis.open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    let (max_scroll, page_step) = games_analysis_popup_scroll_metrics(state, screen, theme);
    match key.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
            state.games.analysis.open = false;
            if let Some(selection) = state.ui_selection {
                if matches!(selection.pane, UiSelectionPane::GamesAnalysisPopup) {
                    state.ui_selection = None;
                }
            }
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            bump_scroll_clamped(&mut state.games.analysis.scroll_offset, -1, max_scroll);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            bump_scroll_clamped(&mut state.games.analysis.scroll_offset, 1, max_scroll);
            true
        }
        KeyCode::PageUp => {
            bump_scroll_clamped(
                &mut state.games.analysis.scroll_offset,
                -(page_step as i32),
                max_scroll,
            );
            true
        }
        KeyCode::PageDown => {
            bump_scroll_clamped(
                &mut state.games.analysis.scroll_offset,
                page_step as i32,
                max_scroll,
            );
            true
        }
        KeyCode::Home => {
            state.games.analysis.scroll_offset = 0;
            true
        }
        KeyCode::End => {
            state.games.analysis.scroll_offset = max_scroll;
            true
        }
        _ => true,
    }
}

fn file_tree_tick(state: &mut AppState, runner: &FileTreeRunner) -> bool {
    if !state.file_tree.open {
        return false;
    }

    let preserve = file_tree::selected_path(state);
    let mut requested = false;
    for dir in file_tree::needed_dirs(state) {
        if state.file_tree.cache.contains_key(&dir) || state.file_tree.loading_dirs.contains(&dir) {
            continue;
        }
        state.file_tree.loading_dirs.insert(dir.clone());
        runner.send(FileTreeCommand::ListDir {
            dir,
            show_hidden: state.file_tree.show_hidden,
            show_ignored: state.file_tree.show_ignored,
        });
        requested = true;
    }

    if requested || state.file_tree.rows.is_empty() {
        file_tree::rebuild_view(state, Some(preserve));
        return true;
    }
    false
}

fn handle_fuzzy_index_event(
    state: &mut AppState,
    runtime: &mut FuzzySearchRuntime,
    event: IndexEvent,
) {
    let open = state.fuzzy_search.open;
    match event {
        IndexEvent::Started { generation } => {
            if generation != runtime.index_gen {
                return;
            }
            if open {
                state.fuzzy_search.indexing = true;
                state.fuzzy_search.status_msg = "Indexing…".into();
            }
        }
        IndexEvent::Batch {
            generation,
            files,
            total_indexed,
        } => {
            if generation != runtime.index_gen {
                return;
            }
            if open {
                state.fuzzy_search.indexing = true;
                state.fuzzy_search.status_msg = format!("Indexing… ({total_indexed} files)");
            }
            runtime
                .fuzzy
                .send(FuzzyCommand::IndexBatch { generation, files });
        }
        IndexEvent::Done {
            generation,
            total_files,
            duration_ms,
        } => {
            if generation != runtime.index_gen {
                return;
            }
            runtime.index_ready = true;
            runtime.fuzzy.send(FuzzyCommand::IndexDone { generation });
            if open {
                state.fuzzy_search.indexing = false;
                if state.fuzzy_search.mode == SearchMode::Files
                    && state.fuzzy_search.query.is_empty()
                {
                    state.fuzzy_search.status_msg = format!("{total_files} files");
                } else if !state.fuzzy_search.searching {
                    state.fuzzy_search.status_msg =
                        format!("Indexed {total_files} files in {duration_ms}ms");
                }
            }
        }
        IndexEvent::Error {
            generation,
            message,
        } => {
            if generation != runtime.index_gen {
                return;
            }
            runtime.index_ready = false;
            if open {
                state.fuzzy_search.indexing = false;
                state.fuzzy_search.status_msg = format!("Index error: {message}");
                state.status = Some(format!("Search index error: {message}"));
            }
        }
    }
}

fn handle_fuzzy_file_event(
    state: &mut AppState,
    runtime: &mut FuzzySearchRuntime,
    event: FuzzyEvent,
) {
    if !state.fuzzy_search.open {
        return;
    }
    match event {
        FuzzyEvent::ResultsReplace {
            generation,
            results,
            total_indexed,
            total_matches,
            duration_ms,
        } => {
            if generation != runtime.file_gen {
                return;
            }
            state.fuzzy_search.file_results = results;
            let len = state.fuzzy_search.file_results.len();
            state.fuzzy_search.selected = state.fuzzy_search.selected.min(len.saturating_sub(1));
            state.fuzzy_search.scroll_offset = state.fuzzy_search.scroll_offset.min(len);
            if state.fuzzy_search.mode == SearchMode::Files {
                if state.fuzzy_search.query.is_empty() {
                    if state.fuzzy_search.indexing {
                        state.fuzzy_search.status_msg =
                            format!("Indexing… ({total_indexed} files)");
                    } else {
                        state.fuzzy_search.status_msg = format!("{total_indexed} files");
                    }
                } else {
                    state.fuzzy_search.status_msg =
                        format!("{total_matches} matches (showing {len}) · {duration_ms}ms");
                }
                runtime.request_preview_for_selection(state);
            }
        }
        FuzzyEvent::ResultsAppend {
            generation,
            results,
            total_indexed,
        } => {
            if generation != runtime.file_gen {
                return;
            }
            state.fuzzy_search.file_results.extend(results);
            if state.fuzzy_search.mode == SearchMode::Files {
                if state.fuzzy_search.indexing {
                    state.fuzzy_search.status_msg = format!("Indexing… ({total_indexed} files)");
                } else if state.fuzzy_search.query.is_empty() {
                    state.fuzzy_search.status_msg = format!("{total_indexed} files");
                }
                runtime.request_preview_for_selection(state);
            }
        }
    }
}

fn handle_fuzzy_content_event(
    state: &mut AppState,
    runtime: &mut FuzzySearchRuntime,
    event: ContentEvent,
) {
    if !state.fuzzy_search.open {
        return;
    }
    match event {
        ContentEvent::Started { generation } => {
            if generation != runtime.content_gen {
                return;
            }
            state.fuzzy_search.searching = true;
            state.fuzzy_search.status_msg = "Searching…".into();
        }
        ContentEvent::MatchBatch {
            generation,
            results,
        } => {
            if generation != runtime.content_gen {
                return;
            }
            state.fuzzy_search.match_results.extend(results);
            if state.fuzzy_search.mode == SearchMode::Content {
                state.fuzzy_search.status_msg = format!(
                    "Searching… ({} matches)",
                    state.fuzzy_search.match_results.len()
                );
                runtime.request_preview_for_selection(state);
            }
        }
        ContentEvent::Done {
            generation,
            total_matches,
            duration_ms,
        } => {
            if generation != runtime.content_gen {
                return;
            }
            state.fuzzy_search.searching = false;
            if state.fuzzy_search.mode == SearchMode::Content {
                state.fuzzy_search.status_msg =
                    format!("{total_matches} matches · {duration_ms}ms");
                runtime.request_preview_for_selection(state);
            }
        }
        ContentEvent::Error {
            generation,
            message,
        } => {
            if generation != runtime.content_gen {
                return;
            }
            state.fuzzy_search.searching = false;
            state.fuzzy_search.status_msg = format!("Search error: {message}");
            state.status = Some(format!("Search error: {message}"));
        }
    }
}

fn handle_fuzzy_preview_event(
    state: &mut AppState,
    runtime: &mut FuzzySearchRuntime,
    event: PreviewEvent,
) {
    if !state.fuzzy_search.open {
        return;
    }
    match event {
        PreviewEvent::Ready { generation, model } => {
            if generation != runtime.preview_gen {
                return;
            }
            runtime.preview_model = Some(model);
        }
        PreviewEvent::Error {
            generation,
            message,
        } => {
            if generation != runtime.preview_gen {
                return;
            }
            runtime.preview_model = None;
            tracing::debug!("preview error: {message}");
        }
    }
}

fn handle_file_tree_key(
    key: &KeyEvent,
    state: &mut AppState,
    syntax: &mut SyntaxRuntime,
    editor_area: ratatui::layout::Rect,
) -> bool {
    if !state.file_tree.open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    if is_petri_show_key(key, state) {
        return false;
    }
    if ctrl_nav_dir(key).is_some() {
        return false;
    }
    if is_command_prompt_open_key(key)
        || is_help_toggle_key(key)
        || is_games_history_open_key(key, state)
    {
        return false;
    }

    if matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('t'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    ) {
        state.file_tree.open = false;
        return true;
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.file_tree.open = false;
            true
        }
        KeyCode::Char('r') => {
            file_tree::clear_cache(state);
            state.status = Some("NITTree refreshed".into());
            true
        }
        KeyCode::Char('.') => {
            state.file_tree.show_hidden = !state.file_tree.show_hidden;
            file_tree::clear_cache(state);
            state.status = Some(if state.file_tree.show_hidden {
                "NITTree: hidden files ON".into()
            } else {
                "NITTree: hidden files OFF".into()
            });
            true
        }
        KeyCode::Char('i') => {
            state.file_tree.show_ignored = !state.file_tree.show_ignored;
            file_tree::clear_cache(state);
            state.status = Some(if state.file_tree.show_ignored {
                "NITTree: ignored files ON".into()
            } else {
                "NITTree: ignored files OFF".into()
            });
            true
        }
        KeyCode::Enter => {
            let Some(row) = state.file_tree.rows.get(state.file_tree.selected) else {
                return true;
            };
            match row.kind {
                nit_core::FileTreeKind::File => {
                    let path = row.path.clone();
                    let _ = apply_action_with_syntax(state, syntax, Action::OpenFile(path));
                    state.file_tree.open = false;
                    true
                }
                nit_core::FileTreeKind::Dir => {
                    let path = row.path.clone();
                    if state.file_tree.expanded_dirs.contains(&path) {
                        // Collapse the directory and any expanded descendants to avoid
                        // background work for items that are no longer visible.
                        state
                            .file_tree
                            .expanded_dirs
                            .retain(|p| !p.starts_with(&path));
                    } else {
                        state.file_tree.expanded_dirs.insert(path.clone());
                    }
                    file_tree::rebuild_view(state, Some(path));
                    adjust_file_tree_scroll(state, editor_area);
                    true
                }
                nit_core::FileTreeKind::Loading => true,
            }
        }
        KeyCode::Up
        | KeyCode::Char('k')
        | KeyCode::Down
        | KeyCode::Char('j')
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Home
        | KeyCode::End => {
            let inner_height = editor_area.height.saturating_sub(2) as usize;
            let page = inner_height.max(1);
            let len = state.file_tree.rows.len();
            if len == 0 {
                return true;
            }
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    state.file_tree.selected = state.file_tree.selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    state.file_tree.selected = (state.file_tree.selected + 1).min(len - 1);
                }
                KeyCode::PageUp => {
                    state.file_tree.selected = state.file_tree.selected.saturating_sub(page);
                }
                KeyCode::PageDown => {
                    state.file_tree.selected = (state.file_tree.selected + page).min(len - 1);
                }
                KeyCode::Home => {
                    state.file_tree.selected = 0;
                }
                KeyCode::End => {
                    state.file_tree.selected = len - 1;
                }
                _ => {}
            }
            adjust_file_tree_scroll(state, editor_area);
            true
        }
        _ => true,
    }
}

fn handle_fuzzy_search_key(
    key: &KeyEvent,
    state: &mut AppState,
    syntax: &mut SyntaxRuntime,
    runtime: &mut FuzzySearchRuntime,
    screen: ratatui::layout::Rect,
) -> bool {
    if !state.fuzzy_search.open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    // Allow global pause/resume while the modal is open.
    if is_job_pause_key(key) {
        return false;
    }

    let popup = dynamic_popup_rect(screen, fuzzy_popup_size(screen, state));
    let list_height = popup
        .height
        .saturating_sub(6) // outer(2) + header/footer(2) + results block(2)
        .max(1) as usize;
    let preview_page = ((list_height as i32) / 2).max(1);

    match key {
        KeyEvent {
            code: KeyCode::Esc, ..
        } => {
            state.fuzzy_search.close();
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Enter | KeyCode::Char('\n') | KeyCode::Char('\r'),
            modifiers,
            ..
        } if modifiers.is_empty() => match state.fuzzy_search.mode {
            SearchMode::Files => {
                let Some(item) = state
                    .fuzzy_search
                    .file_results
                    .get(state.fuzzy_search.selected)
                else {
                    return true;
                };
                let path = item.abs_path.clone();
                let _ = apply_action_with_syntax(state, syntax, Action::OpenFile(path));
                state.file_tree.open = false;
                state.fuzzy_search.close();
                runtime.preview_model = None;
                runtime.last_preview_key = None;
                true
            }
            SearchMode::Content => {
                let Some(item) = state
                    .fuzzy_search
                    .match_results
                    .get(state.fuzzy_search.selected)
                else {
                    return true;
                };
                let path = item.abs_path.clone();
                let line = item.line;
                let col = item.col;
                let _ = apply_action_with_syntax(state, syntax, Action::OpenFile(path));
                {
                    let buf = state.editor_buffer_mut();
                    let total = buf.lines_len().max(1);
                    let target_line = line.saturating_sub(1).min(total.saturating_sub(1));
                    buf.cursor.line = target_line;
                    buf.cursor.col = col.saturating_sub(1);
                    buf.ensure_visible();
                }
                state.file_tree.open = false;
                state.fuzzy_search.close();
                runtime.preview_model = None;
                runtime.last_preview_key = None;
                true
            }
        },
        KeyEvent {
            code: KeyCode::Tab, ..
        } => {
            state.fuzzy_search.mode = match state.fuzzy_search.mode {
                SearchMode::Files => SearchMode::Content,
                SearchMode::Content => SearchMode::Files,
            };
            state.fuzzy_search.selected = 0;
            state.fuzzy_search.scroll_offset = 0;
            runtime.run_search_for_mode(state);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::F(2),
            ..
        } => {
            state.fuzzy_search.show_hidden = !state.fuzzy_search.show_hidden;
            runtime.rebuild_index(state);
            runtime.run_search_for_mode(state);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Char('.'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            state.fuzzy_search.show_hidden = !state.fuzzy_search.show_hidden;
            runtime.rebuild_index(state);
            runtime.run_search_for_mode(state);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{1e}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            state.fuzzy_search.show_hidden = !state.fuzzy_search.show_hidden;
            runtime.rebuild_index(state);
            runtime.run_search_for_mode(state);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::F(3),
            ..
        } => {
            state.fuzzy_search.show_ignored = !state.fuzzy_search.show_ignored;
            runtime.rebuild_index(state);
            runtime.run_search_for_mode(state);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Char('g') | KeyCode::Char('G'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            state.fuzzy_search.show_ignored = !state.fuzzy_search.show_ignored;
            runtime.rebuild_index(state);
            runtime.run_search_for_mode(state);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{7}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            state.fuzzy_search.show_ignored = !state.fuzzy_search.show_ignored;
            runtime.rebuild_index(state);
            runtime.run_search_for_mode(state);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::F(5),
            ..
        } => {
            match state.fuzzy_search.mode {
                SearchMode::Files => {
                    runtime.rebuild_index(state);
                    runtime.run_file_query(state);
                }
                SearchMode::Content => {
                    runtime.run_content_query(state);
                }
            }
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Char('r') | KeyCode::Char('R'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            match state.fuzzy_search.mode {
                SearchMode::Files => {
                    runtime.rebuild_index(state);
                    runtime.run_file_query(state);
                }
                SearchMode::Content => {
                    runtime.run_content_query(state);
                }
            }
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{12}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            match state.fuzzy_search.mode {
                SearchMode::Files => {
                    runtime.rebuild_index(state);
                    runtime.run_file_query(state);
                }
                SearchMode::Content => {
                    runtime.run_content_query(state);
                }
            }
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            true
        }
        KeyEvent {
            code: KeyCode::Backspace,
            modifiers,
            ..
        } => {
            if modifiers.contains(KeyModifiers::CONTROL) {
                delete_last_word(&mut state.fuzzy_search.query);
            } else {
                state.fuzzy_search.query.pop();
            }
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            runtime.run_search_for_mode(state);
            true
        }
        KeyEvent {
            code: KeyCode::Char('u') | KeyCode::Char('U'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta =
                runtime.preview_scroll_delta.saturating_sub(preview_page);
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{15}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            runtime.preview_scroll_delta =
                runtime.preview_scroll_delta.saturating_sub(preview_page);
            true
        }
        KeyEvent {
            code: KeyCode::PageUp,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta =
                runtime.preview_scroll_delta.saturating_sub(preview_page);
            true
        }
        KeyEvent {
            code: KeyCode::Char('d') | KeyCode::Char('D'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta =
                runtime.preview_scroll_delta.saturating_add(preview_page);
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{4}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            runtime.preview_scroll_delta =
                runtime.preview_scroll_delta.saturating_add(preview_page);
            true
        }
        KeyEvent {
            code: KeyCode::PageDown,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta =
                runtime.preview_scroll_delta.saturating_add(preview_page);
            true
        }
        KeyEvent {
            code: KeyCode::Char('y') | KeyCode::Char('Y'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta = runtime.preview_scroll_delta.saturating_sub(1);
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{19}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            runtime.preview_scroll_delta = runtime.preview_scroll_delta.saturating_sub(1);
            true
        }
        KeyEvent {
            code: KeyCode::Up,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta = runtime.preview_scroll_delta.saturating_sub(1);
            true
        }
        KeyEvent {
            code: KeyCode::Char('e') | KeyCode::Char('E'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta = runtime.preview_scroll_delta.saturating_add(1);
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{5}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            runtime.preview_scroll_delta = runtime.preview_scroll_delta.saturating_add(1);
            true
        }
        KeyEvent {
            code: KeyCode::Down,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            runtime.preview_scroll_delta = runtime.preview_scroll_delta.saturating_add(1);
            true
        }
        KeyEvent {
            code: KeyCode::Char('\u{0b}'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            // Some terminals report Ctrl+K as a raw control character.
            state.fuzzy_search.selected = state.fuzzy_search.selected.saturating_sub(1);
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Char('\u{0b}'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            state.fuzzy_search.selected = state.fuzzy_search.selected.saturating_sub(1);
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::Char('j') | KeyCode::Char('J') | KeyCode::Char('\n'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            let len = fuzzy_results_len(state);
            if len > 0 {
                state.fuzzy_search.selected = (state.fuzzy_search.selected + 1).min(len - 1);
            }
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::Up, ..
        } => {
            state.fuzzy_search.selected = state.fuzzy_search.selected.saturating_sub(1);
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::Down,
            ..
        } => {
            let len = fuzzy_results_len(state);
            if len > 0 {
                state.fuzzy_search.selected = (state.fuzzy_search.selected + 1).min(len - 1);
            }
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::PageUp,
            ..
        } => {
            state.fuzzy_search.selected = state
                .fuzzy_search
                .selected
                .saturating_sub(list_height.max(1));
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::PageDown,
            ..
        } => {
            let len = fuzzy_results_len(state);
            if len > 0 {
                state.fuzzy_search.selected =
                    (state.fuzzy_search.selected + list_height.max(1)).min(len - 1);
            }
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::Home,
            ..
        } => {
            state.fuzzy_search.selected = 0;
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::End, ..
        } => {
            let len = fuzzy_results_len(state);
            if len > 0 {
                state.fuzzy_search.selected = len - 1;
            }
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
            true
        }
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers,
            ..
        } if (modifiers.is_empty() || *modifiers == KeyModifiers::SHIFT) && !c.is_control() => {
            state.fuzzy_search.query.push(*c);
            runtime.preview_model = None;
            runtime.last_preview_key = None;
            runtime.run_search_for_mode(state);
            true
        }
        _ => true,
    }
}

fn fuzzy_results_len(state: &AppState) -> usize {
    match state.fuzzy_search.mode {
        SearchMode::Files => state.fuzzy_search.file_results.len(),
        SearchMode::Content => state.fuzzy_search.match_results.len(),
    }
}

fn adjust_fuzzy_scroll(state: &mut AppState, list_height: usize) {
    let len = fuzzy_results_len(state);
    if len == 0 || list_height == 0 {
        state.fuzzy_search.selected = 0;
        state.fuzzy_search.scroll_offset = 0;
        return;
    }
    state.fuzzy_search.selected = state.fuzzy_search.selected.min(len - 1);
    let selected = state.fuzzy_search.selected;
    let mut scroll = state.fuzzy_search.scroll_offset.min(len.saturating_sub(1));
    if selected < scroll {
        scroll = selected;
    } else if selected >= scroll + list_height {
        scroll = selected.saturating_sub(list_height - 1);
    }
    let max_scroll = len.saturating_sub(list_height);
    state.fuzzy_search.scroll_offset = scroll.min(max_scroll);
}

fn delete_last_word(query: &mut String) {
    while query.chars().last().is_some_and(|c| c.is_whitespace()) {
        query.pop();
    }
    while query.chars().last().is_some_and(|c| !c.is_whitespace()) {
        query.pop();
    }
}

fn adjust_file_tree_scroll(state: &mut AppState, editor_area: ratatui::layout::Rect) {
    let inner_height = editor_area.height.saturating_sub(2) as usize;
    if inner_height == 0 {
        return;
    }
    let total = state.file_tree.rows.len();
    if total == 0 {
        state.file_tree.scroll_offset = 0;
        state.file_tree.selected = 0;
        return;
    }
    state.file_tree.selected = state.file_tree.selected.min(total - 1);
    let selected = state.file_tree.selected;
    if selected < state.file_tree.scroll_offset {
        state.file_tree.scroll_offset = selected;
    } else if selected >= state.file_tree.scroll_offset + inner_height {
        state.file_tree.scroll_offset = selected.saturating_sub(inner_height - 1);
    }
    let max_scroll = total.saturating_sub(inner_height);
    state.file_tree.scroll_offset = state.file_tree.scroll_offset.min(max_scroll);
}

fn handle_run_browser_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
) -> bool {
    if state.app_kind != AppKind::Games || !state.games.run_browser.open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    let page_step = popup_page_step(popup_text_area(dynamic_popup_rect(
        screen,
        games_run_browser_popup::preferred_size(screen),
    )));
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.games.run_browser.open = false;
            if let Some(selection) = state.ui_selection {
                if matches!(selection.pane, UiSelectionPane::GamesRunBrowserPopup) {
                    state.ui_selection = None;
                }
            }
            true
        }
        KeyCode::Char('r') => {
            state.games.pending_run_browser = true;
            true
        }
        KeyCode::Enter => {
            if let Some(entry) = state
                .games
                .run_browser
                .entries
                .get(state.games.run_browser.selected)
            {
                state.games.pending_run_load = Some(entry.summary_path.clone());
                state.games.run_browser.loading = true;
            }
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if state.games.run_browser.selected > 0 {
                state.games.run_browser.selected -= 1;
            }
            adjust_run_browser_scroll(state, screen);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let max = state.games.run_browser.entries.len().saturating_sub(1);
            if state.games.run_browser.selected < max {
                state.games.run_browser.selected += 1;
            }
            adjust_run_browser_scroll(state, screen);
            true
        }
        KeyCode::PageUp => {
            state.games.run_browser.selected =
                state.games.run_browser.selected.saturating_sub(page_step);
            adjust_run_browser_scroll(state, screen);
            true
        }
        KeyCode::PageDown => {
            let max = state.games.run_browser.entries.len().saturating_sub(1);
            state.games.run_browser.selected =
                (state.games.run_browser.selected + page_step).min(max);
            adjust_run_browser_scroll(state, screen);
            true
        }
        _ => true,
    }
}

fn adjust_run_browser_scroll(state: &mut AppState, screen: ratatui::layout::Rect) {
    let area = dynamic_popup_rect(screen, games_run_browser_popup::preferred_size(screen));
    let inner_height = area.height.saturating_sub(2) as usize;
    if inner_height == 0 {
        return;
    }
    let selected = state.games.run_browser.selected;
    if selected < state.games.run_browser.scroll_offset {
        state.games.run_browser.scroll_offset = selected;
    } else if selected >= state.games.run_browser.scroll_offset + inner_height {
        state.games.run_browser.scroll_offset = selected.saturating_sub(inner_height - 1);
    }
}

fn handle_replay_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> bool {
    if state.app_kind != AppKind::Games || !state.games.replay.open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    let page_step = popup_page_step(popup_text_area(dynamic_popup_rect(
        screen,
        games_replay_popup::preferred_size(screen),
    )));
    let max_scroll = games_replay_popup_max_scroll(state, screen, theme);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.games.replay.open = false;
            if let Some(selection) = state.ui_selection {
                if matches!(selection.pane, UiSelectionPane::GamesReplayPopup) {
                    state.ui_selection = None;
                }
            }
            true
        }
        KeyCode::Char('r') => {
            state.games.replay.title = None;
            state.games.replay.lines.clear();
            state.games.replay.cycle = None;
            state.games.replay.scroll_offset = 0;
            true
        }
        KeyCode::Enter => {
            if state.games.replay.lines.is_empty() {
                let selection = games_replay_popup::pair_list(state)
                    .get(state.games.replay.selected_index)
                    .map(|(a, b)| (a.clone(), b.clone()));
                if let Some((a, b)) = selection {
                    state.games.pending_replay = Some(nit_core::GamesReplayRequest {
                        a_id: a.clone(),
                        b_id: b.clone(),
                    });
                    state.games.replay.selected_pair = Some((a.clone(), b.clone()));
                    state.games.replay.loading = true;
                    state.games.replay.last_error = None;
                }
            }
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if state.games.replay.lines.is_empty() {
                if state.games.replay.selected_index > 0 {
                    state.games.replay.selected_index -= 1;
                }
                adjust_replay_scroll(state, screen);
            } else {
                bump_scroll_clamped(&mut state.games.replay.scroll_offset, -1, max_scroll);
            }
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if state.games.replay.lines.is_empty() {
                let max = games_replay_popup::pair_list(state).len().saturating_sub(1);
                if state.games.replay.selected_index < max {
                    state.games.replay.selected_index += 1;
                }
                adjust_replay_scroll(state, screen);
            } else {
                bump_scroll_clamped(&mut state.games.replay.scroll_offset, 1, max_scroll);
            }
            true
        }
        KeyCode::PageUp => {
            if state.games.replay.lines.is_empty() {
                state.games.replay.selected_index =
                    state.games.replay.selected_index.saturating_sub(page_step);
                adjust_replay_scroll(state, screen);
            } else {
                bump_scroll_clamped(
                    &mut state.games.replay.scroll_offset,
                    -(page_step as i32),
                    max_scroll,
                );
            }
            true
        }
        KeyCode::PageDown => {
            if state.games.replay.lines.is_empty() {
                let max = games_replay_popup::pair_list(state).len().saturating_sub(1);
                state.games.replay.selected_index =
                    (state.games.replay.selected_index + page_step).min(max);
                adjust_replay_scroll(state, screen);
            } else {
                bump_scroll_clamped(
                    &mut state.games.replay.scroll_offset,
                    page_step as i32,
                    max_scroll,
                );
            }
            true
        }
        _ => true,
    }
}

fn adjust_replay_scroll(state: &mut AppState, screen: ratatui::layout::Rect) {
    let area = dynamic_popup_rect(screen, games_replay_popup::preferred_size(screen));
    let inner_height = area.height.saturating_sub(2) as usize;
    if inner_height == 0 {
        return;
    }
    let selected = state.games.replay.selected_index;
    if selected < state.games.replay.scroll_offset {
        state.games.replay.scroll_offset = selected;
    } else if selected >= state.games.replay.scroll_offset + inner_height {
        state.games.replay.scroll_offset = selected.saturating_sub(inner_height - 1);
    }
}

fn handle_strategy_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
) -> bool {
    if state.app_kind != AppKind::Games || !state.games.strategy_inspect.open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    let page_step = popup_page_step(popup_text_area(dynamic_popup_rect(
        screen,
        games_strategy_popup::preferred_size(screen),
    )));
    let max_scroll = games_strategy_popup_max_scroll(state, screen);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.games.strategy_inspect.open = false;
            if let Some(selection) = state.ui_selection {
                if matches!(selection.pane, UiSelectionPane::GamesStrategyPopup) {
                    state.ui_selection = None;
                }
            }
            true
        }
        KeyCode::Char('r') => {
            state.games.strategy_inspect.title = None;
            state.games.strategy_inspect.lines.clear();
            state.games.strategy_inspect.definition = None;
            state.games.strategy_inspect.scroll_offset = 0;
            true
        }
        KeyCode::Enter => {
            if state.games.strategy_inspect.lines.is_empty() {
                let defs = state.games.strategy_inspect.definitions.as_slice();
                if let Some(def) = defs.get(state.games.strategy_inspect.selected_index) {
                    state.games.strategy_inspect.title = Some(format!(
                        "{} — {}",
                        def.id,
                        games_visualizer_view::strategy_display_name_from_def(def)
                    ));
                    let mut lines = games_strategy_popup::build_definition_lines(def);
                    state.games.strategy_inspect.definition = Some(def.clone());
                    if state.games.strategy_inspect.source_label.as_deref() == Some("run") {
                        if let Some(run) = state.games.last_run.as_ref() {
                            if let Some(result) =
                                run.results.ranking.iter().find(|r| r.id == def.id)
                            {
                                if let Some(metrics) = result.tm_metrics.as_ref() {
                                    lines.push(String::new());
                                    lines.push("tm_metrics:".to_string());
                                    lines.push(format!(
                                        "avg_steps_per_move: {:.3}",
                                        metrics.avg_steps_per_move
                                    ));
                                    lines.push(format!(
                                        "min_steps_per_move: {}",
                                        metrics.min_steps_per_move
                                    ));
                                    lines.push(format!(
                                        "max_steps_per_move: {}",
                                        metrics.max_steps_per_move
                                    ));
                                    lines.push(format!(
                                        "max_steps_hit_count: {}",
                                        metrics.max_steps_hit_count
                                    ));
                                    lines.push(format!(
                                        "output_event_hit_rate: {:.3}",
                                        metrics.output_event_hit_rate
                                    ));
                                    lines.push(format!(
                                        "fallback_rate: {:.3}",
                                        metrics.fallback_rate
                                    ));
                                }
                            }
                        }
                    }
                    state.games.strategy_inspect.lines = lines;
                    state.games.strategy_inspect.scroll_offset = 0;
                }
            }
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if state.games.strategy_inspect.lines.is_empty() {
                if state.games.strategy_inspect.selected_index > 0 {
                    state.games.strategy_inspect.selected_index -= 1;
                }
                adjust_strategy_scroll(state, screen);
            } else {
                bump_scroll_clamped(
                    &mut state.games.strategy_inspect.scroll_offset,
                    -1,
                    max_scroll,
                );
            }
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if state.games.strategy_inspect.lines.is_empty() {
                let max = state
                    .games
                    .strategy_inspect
                    .definitions
                    .len()
                    .saturating_sub(1);
                if state.games.strategy_inspect.selected_index < max {
                    state.games.strategy_inspect.selected_index += 1;
                }
                adjust_strategy_scroll(state, screen);
            } else {
                bump_scroll_clamped(
                    &mut state.games.strategy_inspect.scroll_offset,
                    1,
                    max_scroll,
                );
            }
            true
        }
        KeyCode::PageUp => {
            if state.games.strategy_inspect.lines.is_empty() {
                state.games.strategy_inspect.selected_index = state
                    .games
                    .strategy_inspect
                    .selected_index
                    .saturating_sub(page_step);
                adjust_strategy_scroll(state, screen);
            } else {
                bump_scroll_clamped(
                    &mut state.games.strategy_inspect.scroll_offset,
                    -(page_step as i32),
                    max_scroll,
                );
            }
            true
        }
        KeyCode::PageDown => {
            if state.games.strategy_inspect.lines.is_empty() {
                let max = state
                    .games
                    .strategy_inspect
                    .definitions
                    .len()
                    .saturating_sub(1);
                state.games.strategy_inspect.selected_index =
                    (state.games.strategy_inspect.selected_index + page_step).min(max);
                adjust_strategy_scroll(state, screen);
            } else {
                bump_scroll_clamped(
                    &mut state.games.strategy_inspect.scroll_offset,
                    page_step as i32,
                    max_scroll,
                );
            }
            true
        }
        _ => true,
    }
}

fn handle_tm_sim_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> bool {
    if state.app_kind != AppKind::Games || !state.games.tm_sim.open {
        return false;
    }
    if state.command_line.is_some() || state.prompt.is_some() {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    if is_command_prompt_open_key(key) {
        return false;
    }
    let page_step = popup_page_step(popup_text_area(dynamic_popup_rect(
        screen,
        games_tm_sim_popup::preferred_size(screen),
    )));
    let max_scroll = games_tm_sim_popup_max_scroll(state, screen, theme);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.games.tm_sim.open = false;
            if let Some(selection) = state.ui_selection {
                if matches!(
                    selection.pane,
                    UiSelectionPane::GamesTmSimPopupLeft | UiSelectionPane::GamesTmSimPopupRight
                ) {
                    state.ui_selection = None;
                }
            }
            true
        }
        KeyCode::Char('r') => {
            state.games.tm_sim.scroll_offset = 0;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            bump_scroll_clamped(&mut state.games.tm_sim.scroll_offset, -1, max_scroll);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            bump_scroll_clamped(&mut state.games.tm_sim.scroll_offset, 1, max_scroll);
            true
        }
        KeyCode::PageUp => {
            bump_scroll_clamped(
                &mut state.games.tm_sim.scroll_offset,
                -(page_step as i32),
                max_scroll,
            );
            true
        }
        KeyCode::PageDown => {
            bump_scroll_clamped(
                &mut state.games.tm_sim.scroll_offset,
                page_step as i32,
                max_scroll,
            );
            true
        }
        _ => true,
    }
}

fn handle_ca_sim_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> bool {
    if state.app_kind != AppKind::Games || !state.games.ca_sim.open {
        return false;
    }
    if state.command_line.is_some() || state.prompt.is_some() {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    if is_command_prompt_open_key(key) {
        return false;
    }
    let page_step = popup_page_step(popup_text_area(dynamic_popup_rect(
        screen,
        games_ca_sim_popup::preferred_size(screen),
    )));
    let max_scroll = games_ca_sim_popup_max_scroll(state, screen, theme);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.games.ca_sim.open = false;
            if let Some(selection) = state.ui_selection {
                if matches!(
                    selection.pane,
                    UiSelectionPane::GamesCaSimPopupLeft | UiSelectionPane::GamesCaSimPopupRight
                ) {
                    state.ui_selection = None;
                }
            }
            true
        }
        KeyCode::Char('r') => {
            state.games.ca_sim.scroll_offset = 0;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            bump_scroll_clamped(&mut state.games.ca_sim.scroll_offset, -1, max_scroll);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            bump_scroll_clamped(&mut state.games.ca_sim.scroll_offset, 1, max_scroll);
            true
        }
        KeyCode::PageUp => {
            bump_scroll_clamped(
                &mut state.games.ca_sim.scroll_offset,
                -(page_step as i32),
                max_scroll,
            );
            true
        }
        KeyCode::PageDown => {
            bump_scroll_clamped(
                &mut state.games.ca_sim.scroll_offset,
                page_step as i32,
                max_scroll,
            );
            true
        }
        _ => true,
    }
}

fn handle_match_history_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
) -> bool {
    if state.app_kind != AppKind::Games || !state.games.match_history.open {
        return false;
    }
    if state.command_line.is_some() || state.prompt.is_some() {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    if is_command_prompt_open_key(key) {
        return false;
    }
    let total = games_match_history_total_entries(state);
    let max_offset = games_match_history_max_offset(state, screen);
    let max_rounds = games_match_history_max_rounds(state);
    let default_rounds = games_match_history_default_rounds(state);
    let current_round_limit = state
        .games
        .match_history
        .round_limit
        .unwrap_or(default_rounds)
        .min(max_rounds);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.games.match_history.open = false;
            if let Some(selection) = state.ui_selection {
                if matches!(selection.pane, UiSelectionPane::GamesMatchHistoryPopup) {
                    state.ui_selection = None;
                }
            }
            true
        }
        KeyCode::Left | KeyCode::Char('h') => {
            state.games.match_history.column_offset =
                state.games.match_history.column_offset.saturating_sub(1);
            true
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if total > 0 {
                state.games.match_history.column_offset =
                    (state.games.match_history.column_offset + 1).min(max_offset);
            }
            true
        }
        KeyCode::PageUp => {
            state.games.match_history.column_offset =
                state.games.match_history.column_offset.saturating_sub(5);
            true
        }
        KeyCode::PageDown => {
            if total > 0 {
                state.games.match_history.column_offset =
                    (state.games.match_history.column_offset + 5).min(max_offset);
            }
            true
        }
        KeyCode::Home => {
            state.games.match_history.column_offset = 0;
            true
        }
        KeyCode::End => {
            if total > 0 {
                state.games.match_history.column_offset = max_offset;
            }
            true
        }
        KeyCode::Char('-') | KeyCode::Char('_') => {
            if max_rounds > 0 {
                let new_limit = current_round_limit.saturating_sub(10).max(1);
                state.games.match_history.round_limit = if new_limit == default_rounds {
                    None
                } else {
                    Some(new_limit)
                };
            }
            true
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            if max_rounds > 0 {
                let new_limit = current_round_limit.saturating_add(10).min(max_rounds);
                state.games.match_history.round_limit = if new_limit == default_rounds {
                    None
                } else {
                    Some(new_limit)
                };
            }
            true
        }
        KeyCode::Char('r') => {
            state.games.match_history.round_limit = None;
            true
        }
        _ => true,
    }
}

fn adjust_strategy_scroll(state: &mut AppState, screen: ratatui::layout::Rect) {
    let area = dynamic_popup_rect(screen, games_strategy_popup::preferred_size(screen));
    let inner_height = area.height.saturating_sub(2) as usize;
    if inner_height == 0 {
        return;
    }
    let selected = state.games.strategy_inspect.selected_index;
    if selected < state.games.strategy_inspect.scroll_offset {
        state.games.strategy_inspect.scroll_offset = selected;
    } else if selected >= state.games.strategy_inspect.scroll_offset + inner_height {
        state.games.strategy_inspect.scroll_offset = selected.saturating_sub(inner_height - 1);
    }
}

fn maybe_follow_swarm_artifact_in_popup(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    focus: Option<&SwarmArtifactFocus>,
) {
    let Some(focus) = focus else {
        return;
    };
    if state.agents.artifacts_selected_saved_run_path.is_some() {
        return;
    }
    let mission_id = match focus {
        SwarmArtifactFocus::Task { mission_id, .. } => mission_id.as_str(),
        SwarmArtifactFocus::Report { mission_id } => mission_id.as_str(),
    };
    if state.agents.selected_context_mission() != Some(mission_id) {
        return;
    }

    let width = state.agents.ops_viewport_width.max(32);
    let card_idx = match focus {
        SwarmArtifactFocus::Task {
            mission_id,
            task_id,
        } => agent_ops_view::artifacts_card_index_for_swarm_task(
            state, swarm, width, mission_id, task_id,
        ),
        SwarmArtifactFocus::Report { mission_id } => {
            agent_ops_view::artifacts_card_index_for_swarm_report(state, swarm, width, mission_id)
        }
    };
    let Some(card_idx) = card_idx else {
        return;
    };

    state.agents.artifacts_selected = card_idx;
    state.agents.artifacts_popup_scroll = 0;
    if let Some(selection) = state.ui_selection {
        if matches!(selection.pane, UiSelectionPane::ArtifactsPopup) {
            state.ui_selection = None;
        }
    }
}

fn maybe_open_artifact_popup_from_console_line(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    text_width: usize,
    line_idx: usize,
) -> bool {
    let Some(message_idx) = agent_console_view::artifact_message_index_for_line_with_swarm(
        state,
        Some(swarm),
        text_width,
        line_idx,
    ) else {
        return false;
    };
    let Some(message) = state.agents.messages.get(message_idx).cloned() else {
        return false;
    };

    if let Some(mission_id) = message.mission_id.as_deref() {
        state.agents.selected_mission = Some(mission_id.to_string());
        if let Some(mission_idx) = state
            .agents
            .missions
            .iter()
            .position(|mission| mission.id == mission_id)
        {
            state.agents.mission_selected = mission_idx;
        }
    } else if let Some(agent_id) = message.agent_id.as_deref() {
        // Resolve chat-clone ids to the base agent so the context stays on the
        // user-selected model and other artifacts remain visible.
        let resolved = chat_clone_base_id(agent_id).unwrap_or(agent_id);
        state.agents.selected_mission = None;
        state.agents.selected_agent = Some(resolved.to_string());
    }

    let selected = agent_ops_view::artifacts_popup_ref_for_message(
        state,
        Some(swarm),
        text_width,
        message_idx,
    )
    .and_then(|popup_ref| {
        agent_ops_view::artifacts_card_index_for_popup_ref(
            state,
            Some(swarm),
            text_width,
            &popup_ref,
        )
    });
    let Some(card_idx) = selected else {
        return false;
    };

    state.agents.artifacts_selected_saved_run_path = None;
    state.agents.artifacts_selected = card_idx;
    state.agents.artifacts_popup_open = true;
    state.agents.artifacts_popup_scroll = 0;
    true
}

fn load_selected_artifacts_history_entry(state: &mut AppState) {
    let entries = agent_ops_view::artifacts_history_visible_entries(state);
    if entries.is_empty() {
        state.agents.artifacts_selected_saved_run_path = None;
    } else {
        let selected = state
            .agents
            .artifacts_history_selected
            .min(entries.len().saturating_sub(1));
        state.agents.artifacts_selected_saved_run_path = entries
            .get(selected)
            .and_then(|entry| entry.run_path.clone());
    }
    state.agents.artifacts_selected = 0;
    state.agents.ops_scroll = 0;
    state.agents.artifacts_history_pending_action = None;
}

fn sync_artifacts_history_popup_selection(state: &mut AppState) {
    let entries = agent_ops_view::artifacts_history_visible_entries(state);
    if entries.is_empty() {
        state.agents.artifacts_history_selected = 0;
        state.agents.artifacts_selected_saved_run_path = None;
        return;
    }
    let selected_path = state.agents.artifacts_selected_saved_run_path.as_deref();
    let selected = entries
        .iter()
        .position(|entry| entry.run_path.as_deref() == selected_path)
        .unwrap_or_else(|| {
            state
                .agents
                .artifacts_history_selected
                .min(entries.len().saturating_sub(1))
        });
    state.agents.artifacts_history_selected = selected.min(entries.len().saturating_sub(1));
}

fn clear_artifacts_history_pending_action(state: &mut AppState) {
    state.agents.artifacts_history_pending_action = None;
}

fn remove_saved_run_entry(entry: &agent_ops_view::SavedArtifactsRunEntry) -> io::Result<bool> {
    let Some(run_path) = entry.run_path.as_deref() else {
        return Ok(false);
    };
    let run_path = PathBuf::from(run_path);
    let target = if run_path.file_name().and_then(|name| name.to_str()) == Some("run.json") {
        run_path.parent().map(Path::to_path_buf).unwrap_or(run_path)
    } else {
        run_path
    };
    if !target.exists() {
        return Ok(false);
    }
    fs::remove_dir_all(target)?;
    Ok(true)
}

fn delete_selected_artifacts_history_entry(
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> bool {
    let entries = agent_ops_view::artifacts_history_visible_entries(state);
    let Some(entry) = entries.get(
        state
            .agents
            .artifacts_history_selected
            .min(entries.len().saturating_sub(1)),
    ) else {
        state.status = Some("No saved run selected.".into());
        clear_artifacts_history_pending_action(state);
        return true;
    };
    if !matches!(entry.kind, agent_ops_view::SavedArtifactsRunKind::Archived) {
        state.status = Some("Current/latest saved run cannot be deleted.".into());
        clear_artifacts_history_pending_action(state);
        return true;
    }
    if !matches!(
        state.agents.artifacts_history_pending_action,
        Some(SavedRunHistoryPendingAction::DeleteSelected)
    ) {
        state.agents.artifacts_history_pending_action =
            Some(SavedRunHistoryPendingAction::DeleteSelected);
        state.status = Some(format!("Confirm delete for {}.", entry.label));
        return true;
    }
    let deleted = match remove_saved_run_entry(entry) {
        Ok(deleted) => deleted,
        Err(err) => {
            state.status = Some(format!("Failed to delete saved run: {err}"));
            clear_artifacts_history_pending_action(state);
            return true;
        }
    };
    if deleted
        && state.agents.artifacts_selected_saved_run_path.as_deref() == entry.run_path.as_deref()
    {
        state.agents.artifacts_selected_saved_run_path = None;
    }
    clear_artifacts_history_pending_action(state);
    sync_artifacts_history_popup_selection(state);
    adjust_artifacts_history_popup_scroll(state, screen, theme);
    state.status = Some(if deleted {
        format!("Deleted {}.", entry.label)
    } else {
        format!("Saved run {} was already missing.", entry.label)
    });
    true
}

fn prune_filtered_artifacts_history_entries(
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> bool {
    let prunable = agent_ops_view::artifacts_history_prunable_entries(state);
    if prunable.is_empty() {
        state.status = Some("No saved runs match the current filter.".into());
        clear_artifacts_history_pending_action(state);
        return true;
    }
    if !matches!(
        state.agents.artifacts_history_pending_action,
        Some(SavedRunHistoryPendingAction::PruneFiltered)
    ) {
        state.agents.artifacts_history_pending_action =
            Some(SavedRunHistoryPendingAction::PruneFiltered);
        state.status = Some(format!("Confirm prune for {} saved runs.", prunable.len()));
        return true;
    }
    let selected_path = state.agents.artifacts_selected_saved_run_path.clone();
    let mut removed = 0usize;
    for entry in &prunable {
        match remove_saved_run_entry(entry) {
            Ok(true) => removed = removed.saturating_add(1),
            Ok(false) => {}
            Err(err) => {
                state.status = Some(format!("Failed to prune saved runs: {err}"));
                clear_artifacts_history_pending_action(state);
                sync_artifacts_history_popup_selection(state);
                adjust_artifacts_history_popup_scroll(state, screen, theme);
                return true;
            }
        }
    }
    if selected_path.as_deref().is_some_and(|path| {
        prunable
            .iter()
            .any(|entry| entry.run_path.as_deref() == Some(path))
    }) {
        state.agents.artifacts_selected_saved_run_path = None;
    }
    clear_artifacts_history_pending_action(state);
    sync_artifacts_history_popup_selection(state);
    adjust_artifacts_history_popup_scroll(state, screen, theme);
    state.status = Some(format!("Pruned {removed} saved runs."));
    true
}

fn adjust_artifacts_history_popup_scroll(
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) {
    let area = dynamic_popup_rect(screen, artifacts_history_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let inner_height = text_area.height.max(1) as usize;
    let total = artifacts_history_popup::build_lines(state, theme, text_area.width).len();
    let max_scroll = total.saturating_sub(inner_height);
    let selected_line = 6usize.saturating_add(state.agents.artifacts_history_selected);
    if selected_line < state.agents.artifacts_history_popup_scroll {
        state.agents.artifacts_history_popup_scroll = selected_line;
    } else if selected_line
        >= state
            .agents
            .artifacts_history_popup_scroll
            .saturating_add(inner_height)
    {
        state.agents.artifacts_history_popup_scroll =
            selected_line.saturating_sub(inner_height.saturating_sub(1));
    }
    state.agents.artifacts_history_popup_scroll =
        state.agents.artifacts_history_popup_scroll.min(max_scroll);
}

fn handle_artifacts_history_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> bool {
    if !state.agents.artifacts_history_popup_open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    let entries = agent_ops_view::artifacts_history_visible_entries(state);
    let max = entries.len().saturating_sub(1);
    let (_, page_step) = artifacts_history_popup_scroll_metrics(state, screen, theme);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            clear_artifacts_history_pending_action(state);
            state.agents.artifacts_history_popup_open = false;
            state.agents.artifacts_history_popup_scroll = 0;
            if let Some(selection) = state.ui_selection {
                if matches!(selection.pane, UiSelectionPane::ArtifactsHistoryPopup) {
                    state.ui_selection = None;
                }
            }
            true
        }
        KeyCode::Enter => {
            load_selected_artifacts_history_entry(state);
            state.agents.artifacts_history_popup_open = false;
            state.status = Some(format!(
                "Artifacts source: {}",
                agent_ops_view::artifacts_history_summary_label(state)
            ));
            true
        }
        KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::Char('c') | KeyCode::Char('C') => {
            state.agents.artifacts_history_selected = 0;
            load_selected_artifacts_history_entry(state);
            state.agents.artifacts_history_popup_open = false;
            state.status = Some("Artifacts source: current / latest saved run".into());
            true
        }
        KeyCode::Delete | KeyCode::Char('x') | KeyCode::Char('X') => {
            delete_selected_artifacts_history_entry(state, screen, theme)
        }
        KeyCode::Char('p') | KeyCode::Char('P') => {
            prune_filtered_artifacts_history_entries(state, screen, theme)
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            clear_artifacts_history_pending_action(state);
            state.agents.artifacts_history_filter = SavedRunHistoryFilter::All;
            sync_artifacts_history_popup_selection(state);
            adjust_artifacts_history_popup_scroll(state, screen, theme);
            true
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            clear_artifacts_history_pending_action(state);
            state.agents.artifacts_history_filter = SavedRunHistoryFilter::LastDay;
            sync_artifacts_history_popup_selection(state);
            adjust_artifacts_history_popup_scroll(state, screen, theme);
            true
        }
        KeyCode::Char('w') | KeyCode::Char('W') => {
            clear_artifacts_history_pending_action(state);
            state.agents.artifacts_history_filter = SavedRunHistoryFilter::LastWeek;
            sync_artifacts_history_popup_selection(state);
            adjust_artifacts_history_popup_scroll(state, screen, theme);
            true
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            clear_artifacts_history_pending_action(state);
            state.agents.artifacts_history_filter = SavedRunHistoryFilter::LastMonth;
            sync_artifacts_history_popup_selection(state);
            adjust_artifacts_history_popup_scroll(state, screen, theme);
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            clear_artifacts_history_pending_action(state);
            state.agents.artifacts_history_selected =
                state.agents.artifacts_history_selected.saturating_sub(1);
            adjust_artifacts_history_popup_scroll(state, screen, theme);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            clear_artifacts_history_pending_action(state);
            state.agents.artifacts_history_selected =
                (state.agents.artifacts_history_selected + 1).min(max);
            adjust_artifacts_history_popup_scroll(state, screen, theme);
            true
        }
        KeyCode::PageUp => {
            clear_artifacts_history_pending_action(state);
            state.agents.artifacts_history_selected = state
                .agents
                .artifacts_history_selected
                .saturating_sub(page_step);
            adjust_artifacts_history_popup_scroll(state, screen, theme);
            true
        }
        KeyCode::PageDown => {
            clear_artifacts_history_pending_action(state);
            state.agents.artifacts_history_selected =
                (state.agents.artifacts_history_selected + page_step).min(max);
            adjust_artifacts_history_popup_scroll(state, screen, theme);
            true
        }
        KeyCode::Home => {
            clear_artifacts_history_pending_action(state);
            state.agents.artifacts_history_selected = 0;
            adjust_artifacts_history_popup_scroll(state, screen, theme);
            true
        }
        KeyCode::End => {
            clear_artifacts_history_pending_action(state);
            state.agents.artifacts_history_selected = max;
            adjust_artifacts_history_popup_scroll(state, screen, theme);
            true
        }
        _ => true,
    }
}

fn handle_help_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> bool {
    if !state.show_help {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
    let (max_scroll, page_step) = help_popup_scroll_metrics(screen, theme);
    let close = match key.code {
        KeyCode::Esc | KeyCode::F(1) | KeyCode::Enter | KeyCode::Char('q') => true,
        KeyCode::Char('?') if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            true
        }
        _ => false,
    };
    if close {
        state.show_help = false;
        state.help_scroll = 0;
        if let Some(selection) = state.ui_selection {
            if matches!(selection.pane, UiSelectionPane::HelpPopup) {
                state.ui_selection = None;
            }
        }
        true
    } else {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                bump_scroll_clamped(&mut state.help_scroll, -1, max_scroll);
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                bump_scroll_clamped(&mut state.help_scroll, 1, max_scroll);
                true
            }
            KeyCode::PageUp => {
                bump_scroll_clamped(&mut state.help_scroll, -(page_step as i32), max_scroll);
                true
            }
            KeyCode::PageDown => {
                bump_scroll_clamped(&mut state.help_scroll, page_step as i32, max_scroll);
                true
            }
            KeyCode::Home => {
                state.help_scroll = 0;
                true
            }
            KeyCode::End => {
                state.help_scroll = max_scroll;
                true
            }
            _ => true,
        }
    }
}

fn prompt_size(message: &str) -> (u16, u16) {
    let width = message.chars().count().max(12) as u16 + 4;
    let height = 3;
    (width, height)
}

fn command_prompt_cursor(
    area: ratatui::layout::Rect,
    input: &str,
    cursor: usize,
) -> Option<(u16, u16)> {
    if area.width < 3 || area.height < 3 {
        return None;
    }
    let inner_width = area.width.saturating_sub(2);
    if inner_width == 0 {
        return None;
    }
    let inner_x = area.x.saturating_add(1);
    let inner_y = area.y.saturating_add(1);
    let prefix_width = unicode_width::UnicodeWidthStr::width(":");
    let cursor_text: String = input.chars().take(cursor).collect();
    let cursor_width = unicode_width::UnicodeWidthStr::width(cursor_text.as_str());
    let mut col = inner_x.saturating_add((prefix_width + cursor_width) as u16);
    let max_col = inner_x.saturating_add(inner_width.saturating_sub(1));
    if col > max_col {
        col = max_col;
    }
    Some((col, inner_y))
}

fn fuzzy_search_cursor(area: ratatui::layout::Rect, state: &AppState) -> Option<(u16, u16)> {
    if area.width < 3 || area.height < 3 {
        return None;
    }
    let inner_width = area.width.saturating_sub(2);
    if inner_width == 0 {
        return None;
    }
    let inner_x = area.x.saturating_add(1);
    let inner_y = area.y.saturating_add(1);
    let mode = match state.fuzzy_search.mode {
        SearchMode::Files => "[FILES]",
        SearchMode::Content => "[CONTENT]",
    };
    let prefix_width =
        unicode_width::UnicodeWidthStr::width(mode) + unicode_width::UnicodeWidthStr::width("  > ");
    let query_width = unicode_width::UnicodeWidthStr::width(state.fuzzy_search.query.as_str());
    let mut col = inner_x.saturating_add((prefix_width + query_width) as u16);
    let max_col = inner_x.saturating_add(inner_width.saturating_sub(1));
    if col > max_col {
        col = max_col;
    }
    Some((col, inner_y))
}

fn render_prompt(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    message: &str,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title("CONFIRM")
        .style(Style::default().bg(theme.selection_bg));
    let paragraph = Paragraph::new(message)
        .style(Style::default().bg(theme.selection_bg).fg(theme.foreground))
        .block(block);
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

fn render_command_prompt(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    message: &str,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title("COMMAND")
        .style(Style::default().bg(theme.selection_bg));
    let paragraph = Paragraph::new(message)
        .style(Style::default().bg(theme.selection_bg).fg(theme.foreground))
        .block(block);
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

fn save_notes_on_exit(state: &AppState) -> core_io::Result<()> {
    let buffer = state.notes_buffer();
    if buffer.path().is_none() {
        return Ok(());
    }
    if !buffer.is_dirty() {
        return Ok(());
    }
    core_io::save_buffer(buffer)
}

fn flush_agent_run_provenance(state: &mut AppState, swarm: &SwarmRuntime) -> io::Result<()> {
    let pending = std::mem::take(&mut state.agents.pending_provenance_mission_ids);
    let pending_agents = std::mem::take(&mut state.agents.pending_provenance_agent_ids);
    if !pending.is_empty() {
        let unique = pending.into_iter().collect::<BTreeSet<_>>();
        for mission_id in unique {
            write_agent_run_provenance(state, swarm, &mission_id)?;
        }
    }
    if !pending_agents.is_empty() {
        let unique = pending_agents.into_iter().collect::<BTreeSet<_>>();
        for agent_id in unique {
            write_ad_hoc_run_provenance(state, &agent_id)?;
        }
    }
    Ok(())
}

fn write_agent_run_provenance(
    state: &AppState,
    swarm: &SwarmRuntime,
    mission_id: &str,
) -> io::Result<()> {
    let Some(mission) = state
        .agents
        .missions
        .iter()
        .find(|mission| mission.id == mission_id)
    else {
        return Ok(());
    };
    let run_dir = state
        .workspace_root
        .join(".nit")
        .join("agents")
        .join("runs")
        .join(mission_id);
    let patches_dir = run_dir.join("patches");
    fs::create_dir_all(&patches_dir)?;

    let messages = state
        .agents
        .messages
        .iter()
        .filter(|msg| msg.mission_id.as_deref() == Some(mission_id))
        .cloned()
        .collect::<Vec<_>>();
    let patches = state
        .agents
        .patches
        .iter()
        .filter(|patch| patch.mission_id.as_deref() == Some(mission_id))
        .cloned()
        .collect::<Vec<_>>();
    let evidence = state
        .agents
        .evidence
        .iter()
        .filter(|item| item.mission_id.as_deref() == Some(mission_id))
        .cloned()
        .collect::<Vec<_>>();
    let run_payload = serde_json::json!({
            "id": mission.id,
            "title": mission.title,
            "phase": mission.phase.label(),
        "status": mission.status,
        "swarm": mission.swarm,
            "assigned_agents": mission.assigned_agents,
            "updated_at": mission.updated_at,
            "selected_agent": state.agents.selected_context_agent(),
            "codex_thread_id": state
                .agents
                .selected_context_agent()
                .and_then(|agent| state.agents.codex_mission_thread_ids.get(mission_id)?.get(agent)),
            "codex_thread_ids": state.agents.codex_mission_thread_ids.get(mission_id),
            "mcp": {
                "state": state.agents.mcp.state.label(),
                "endpoint": state.agents.mcp.endpoint,
                "latency_ms": state.agents.mcp.latency_ms,
            "last_error": state.agents.mcp.last_error,
        },
        "messages": messages.clone(),
        "patches": patches.clone(),
        "evidence": evidence,
    });
    let run_json = serde_json::to_string_pretty(&run_payload)
        .map_err(|err| io::Error::other(format!("serde run.json: {err}")))?;
    fs::write(run_dir.join("run.json"), run_json)?;

    let mut thread_md = String::new();
    thread_md.push_str(&format!("# Mission {}\n\n", mission.id));
    thread_md.push_str(&format!("Title: {}\n\n", mission.title));
    thread_md.push_str("## Thread\n\n");
    for msg in messages.iter() {
        let channel = match msg.channel {
            AgentChannel::Agent => "",
            AgentChannel::Broadcast => "@all ",
        };
        let src = msg.agent_id.as_deref().unwrap_or("user");
        thread_md.push_str(&format!(
            "- [{}] {}{}: {}\n",
            msg.at, channel, src, msg.text
        ));
    }
    fs::write(run_dir.join("thread.md"), thread_md)?;

    for patch in patches.iter() {
        let filename = format!("{}.diff", sanitize_for_filename(&patch.id));
        fs::write(patches_dir.join(filename), &patch.diff)?;
    }
    write_swarm_run_provenance(state, swarm, mission_id)?;
    Ok(())
}

fn write_ad_hoc_run_provenance(state: &AppState, agent_id: &str) -> io::Result<()> {
    let run_dir = state
        .workspace_root
        .join(".nit")
        .join("agents")
        .join("ad-hoc")
        .join(sanitize_for_filename(agent_id));
    let patches_dir = run_dir.join("patches");
    fs::create_dir_all(&patches_dir)?;

    let is_own_or_clone = |id: Option<&str>| -> bool {
        id == Some(agent_id) || id.is_some_and(|id| chat_clone_base_id(id) == Some(agent_id))
    };
    let messages = state
        .agents
        .messages
        .iter()
        .filter(|message| {
            message.mission_id.is_none()
                && (message.agent_id.is_none() || is_own_or_clone(message.agent_id.as_deref()))
        })
        .cloned()
        .collect::<Vec<_>>();
    let patches = state
        .agents
        .patches
        .iter()
        .filter(|patch| {
            patch.mission_id.is_none()
                && (patch.agent_id == agent_id
                    || chat_clone_base_id(&patch.agent_id) == Some(agent_id))
        })
        .cloned()
        .collect::<Vec<_>>();
    let evidence = state
        .agents
        .evidence
        .iter()
        .filter(|item| {
            item.mission_id.is_none() && is_own_or_clone(item.agent_id.as_deref())
        })
        .cloned()
        .collect::<Vec<_>>();

    let run_payload = serde_json::json!({
        "agent_id": agent_id,
        "context": "ad-hoc",
        "updated_at": timestamp_label(state),
        "codex_thread_id": state.agents.codex_thread_ids.get(agent_id),
        "messages": messages.clone(),
        "patches": patches.clone(),
        "evidence": evidence,
    });
    let run_json = serde_json::to_vec_pretty(&run_payload)
        .map_err(|err| io::Error::other(format!("serde ad-hoc run.json: {err}")))?;
    write_file_atomic(&run_dir.join("run.json"), &run_json)?;

    let mut thread_md = String::new();
    thread_md.push_str(&format!("# Ad-hoc thread for {agent_id}\n\n"));
    thread_md.push_str("## Thread\n\n");
    for msg in messages.iter() {
        let channel = match msg.channel {
            AgentChannel::Agent => "",
            AgentChannel::Broadcast => "@all ",
        };
        let src = msg.agent_id.as_deref().unwrap_or("user");
        thread_md.push_str(&format!(
            "- [{}] {}{}: {}\n",
            msg.at, channel, src, msg.text
        ));
    }
    write_file_atomic(&run_dir.join("thread.md"), thread_md.as_bytes())?;

    for patch in patches.iter() {
        let filename = format!("{}.diff", sanitize_for_filename(&patch.id));
        write_file_atomic(&patches_dir.join(filename), patch.diff.as_bytes())?;
    }

    Ok(())
}

fn write_swarm_run_provenance(
    state: &AppState,
    swarm: &SwarmRuntime,
    mission_id: &str,
) -> io::Result<()> {
    let Some(mission) = state
        .agents
        .missions
        .iter()
        .find(|mission| mission.id == mission_id)
    else {
        return Ok(());
    };
    if !mission.swarm {
        return Ok(());
    }
    let Some(view) = swarm.swarm_persistence(mission_id) else {
        return Ok(());
    };

    let run_dir = state
        .workspace_root
        .join(".nit")
        .join("swarm")
        .join(mission_id);
    let tasks_dir = run_dir.join("tasks");
    let gates_dir = run_dir.join("gates");
    let report_dir = run_dir.join("report");
    fs::create_dir_all(&tasks_dir)?;
    fs::create_dir_all(&gates_dir)?;
    fs::create_dir_all(&report_dir)?;

    let run_payload = serde_json::json!({
        "id": mission.id,
        "title": mission.title,
        "phase": mission.phase.label(),
        "status": mission.status,
        "template": view.template,
        "swarm": mission.swarm,
        "updated_at": mission.updated_at,
        "gate_bundle": view.gate_bundle,
        "gate_selection": view.gate_selection,
        "report_status": view.report_status,
        "report_agent_id": view.report_agent_id,
        "report_present": view.report_output.is_some(),
        "task_count": view.tasks.len(),
        "tasks": view.tasks.iter().map(|task| {
            serde_json::json!({
                "id": task.id,
                "agent_id": task.agent_id,
                "role": task.role,
                "title": task.title,
                "state": task.state,
                "deps": task.deps,
                "blocked_on": task.blocked_on,
                "writes": task.writes,
                "expected_artifacts": task.expected_artifacts,
                "expected_artifacts_missing": task.expected_artifacts_missing,
                "output_present": task.output_present
            })
        }).collect::<Vec<_>>()
    });
    let run_json = serde_json::to_vec_pretty(&run_payload)
        .map_err(|err| io::Error::other(format!("serde swarm run.json: {err}")))?;
    write_file_atomic(&run_dir.join("run.json"), &run_json)?;

    let mut summary_entries = Vec::new();
    for task in view.tasks.iter() {
        let task_dir = tasks_dir.join(sanitize_for_filename(&task.id));
        fs::create_dir_all(&task_dir)?;

        if let Some(artifacts) = task.artifacts.as_ref() {
            let artifacts_json = serde_json::to_vec_pretty(artifacts)
                .map_err(|err| io::Error::other(format!("serde artifacts.json: {err}")))?;
            write_file_atomic(&task_dir.join("artifacts.json"), &artifacts_json)?;
            if let Some(summary) = artifacts.summary.as_deref().map(str::trim) {
                if !summary.is_empty() {
                    summary_entries.push(serde_json::json!({
                        "task_id": task.id,
                        "summary": summary
                    }));
                }
            }
        }

        if let Some(output) = task.output.as_deref() {
            write_file_atomic(&task_dir.join("output.md"), output.as_bytes())?;
        }
    }

    if !summary_entries.is_empty() {
        let summary_json = serde_json::to_vec_pretty(&serde_json::json!({
            "mission_id": mission_id,
            "summaries": summary_entries
        }))
        .map_err(|err| io::Error::other(format!("serde summary.json: {err}")))?;
        write_file_atomic(&run_dir.join("summary.json"), &summary_json)?;
    }

    if let Some(report) = view.gate_report.as_ref() {
        let report_json = serde_json::to_vec_pretty(report)
            .map_err(|err| io::Error::other(format!("serde gate report: {err}")))?;
        write_file_atomic(&gates_dir.join("report.json"), &report_json)?;
    }
    if let Some(output) = view.gate_output.as_deref() {
        write_file_atomic(&gates_dir.join("output.txt"), output.as_bytes())?;
    }
    if view.gate_bundle.is_some() || view.gate_report.is_some() || view.gate_output.is_some() {
        let verify_md = render_verify_markdown(
            mission_id,
            view.gate_bundle.as_deref(),
            view.gate_selection.as_str(),
            view.gate_report.as_ref(),
            view.gate_output.as_deref(),
        );
        write_file_atomic(&gates_dir.join("verify.md"), verify_md.as_bytes())?;
    }
    if let Some(report_output) = view.report_output.as_deref() {
        write_file_atomic(&report_dir.join("final.md"), report_output.as_bytes())?;
    }

    Ok(())
}

const MAX_SAVED_RUN_HISTORY_PER_CONTEXT: usize = 200;

fn prune_saved_run_history(history_root: &Path, keep_latest: usize) -> io::Result<()> {
    let read_dir = match fs::read_dir(history_root) {
        Ok(read_dir) => read_dir,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let mut archive_dirs = read_dir
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    archive_dirs.sort_by(|left, right| right.cmp(left));
    for archive_dir in archive_dirs.into_iter().skip(keep_latest) {
        fs::remove_dir_all(archive_dir)?;
    }
    Ok(())
}

fn archive_saved_run_snapshot(run_dir: &Path) -> io::Result<Option<PathBuf>> {
    let run_json = run_dir.join("run.json");
    if !run_json.exists() {
        return Ok(None);
    }

    let archive_id_base = format!(
        "{:020}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros()
    );
    let history_root = run_dir.join("history");
    fs::create_dir_all(&history_root)?;

    let mut archive_dir = history_root.join(&archive_id_base);
    let mut suffix = 1usize;
    while archive_dir.exists() {
        archive_dir = history_root.join(format!("{archive_id_base}-{suffix}"));
        suffix = suffix.saturating_add(1);
    }
    fs::create_dir_all(archive_dir.join("patches"))?;

    for file_name in ["run.json", "thread.md"] {
        let src = run_dir.join(file_name);
        if !src.is_file() {
            continue;
        }
        let contents = fs::read(&src)?;
        write_file_atomic(&archive_dir.join(file_name), &contents)?;
    }

    let patches_src = run_dir.join("patches");
    if patches_src.is_dir() {
        for entry in fs::read_dir(&patches_src)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(file_name) = path.file_name() else {
                continue;
            };
            let contents = fs::read(&path)?;
            write_file_atomic(&archive_dir.join("patches").join(file_name), &contents)?;
        }
    }

    prune_saved_run_history(&history_root, MAX_SAVED_RUN_HISTORY_PER_CONTEXT)?;

    Ok(Some(archive_dir))
}

fn verify_gate_status_label(gate: &GateReportGate) -> &'static str {
    if let Some(status) = gate.status.as_deref() {
        if status.eq_ignore_ascii_case("pass")
            || status.eq_ignore_ascii_case("ok")
            || status.eq_ignore_ascii_case("success")
        {
            return "PASS";
        }
        if status.eq_ignore_ascii_case("skip") || status.eq_ignore_ascii_case("skipped") {
            return "SKIP";
        }
        if status.eq_ignore_ascii_case("fail") || status.eq_ignore_ascii_case("failed") {
            return "FAIL";
        }
    }
    if gate.ok {
        "PASS"
    } else {
        "FAIL"
    }
}

fn truncate_for_markdown(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let truncated = text.chars().take(max_chars).collect::<String>();
    format!("{truncated}\n\n... [truncated {} chars]", total - max_chars)
}

fn render_verify_markdown(
    mission_id: &str,
    gate_bundle: Option<&str>,
    gate_selection: &str,
    report: Option<&GateReport>,
    output: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str("# Verify\n\n");
    out.push_str(&format!("Mission: `{mission_id}`\n\n"));
    out.push_str(&format!("Bundle: `{}`\n\n", gate_bundle.unwrap_or("none")));
    out.push_str(&format!("Selection: `{}`\n\n", gate_selection.trim()));

    let status = if let Some(report) = report {
        if report.overall_ok {
            "PASS"
        } else {
            "FAIL"
        }
    } else if gate_bundle.is_some() {
        "PENDING"
    } else {
        "NONE"
    };
    out.push_str(&format!("Status: `{status}`\n\n"));

    if let Some(report) = report {
        out.push_str("## Gates\n\n");
        for gate in report.gates.iter() {
            out.push_str(&format!(
                "- `{}`: `{}`\n",
                gate.name,
                verify_gate_status_label(gate)
            ));
            out.push_str(&format!("  - Command: `{}`\n", gate.command));
            if let Some(notes) = gate
                .notes
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                out.push_str(&format!("  - Notes: {notes}\n"));
            }
        }
        out.push('\n');
    }

    out.push_str("## Files\n\n");
    if report.is_some() {
        out.push_str("- `report.json`\n");
    }
    if output.is_some() {
        out.push_str("- `output.txt`\n");
    }
    if gate_bundle.is_some() || report.is_some() || output.is_some() {
        out.push_str("- `verify.md`\n");
    }
    out.push('\n');

    if let Some(output) = output.map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str("## Output Excerpt\n\n```text\n");
        out.push_str(&truncate_for_markdown(output, VERIFY_OUTPUT_MD_MAX_CHARS));
        out.push_str("\n```\n");
    }

    out
}

fn write_file_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "data".into());
    let tmp = path.with_file_name(format!(".{file_name}.nit.tmp"));
    fs::write(&tmp, contents)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn sanitize_for_filename(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Default)]
struct TerminalState {
    active: bool,
    raw_mode: bool,
    alternate_screen: bool,
    keyboard_flags_pushed: bool,
    mouse_capture: bool,
    bracketed_paste: bool,
    cursor_hidden: bool,
}

impl TerminalState {
    fn restore(&mut self) {
        if !self.active {
            return;
        }
        let mut stdout = io::stdout();
        if self.keyboard_flags_pushed {
            let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        }
        if self.mouse_capture {
            let _ = execute!(stdout, DisableMouseCapture);
        }
        if self.bracketed_paste {
            let _ = execute!(stdout, DisableBracketedPaste);
        }
        let _ = execute!(stdout, SetCursorStyle::DefaultUserShape);
        if self.cursor_hidden {
            let _ = execute!(stdout, Show);
        }
        if self.raw_mode {
            let _ = disable_raw_mode();
        }
        if self.alternate_screen {
            let _ = execute!(stdout, LeaveAlternateScreen);
        }
        self.active = false;
    }
}

struct TerminalGuard {
    state: Arc<Mutex<TerminalState>>,
}

impl TerminalGuard {
    fn activate() -> io::Result<(Self, Stdout)> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(err) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(err);
        }
        let state = TerminalState {
            active: true,
            raw_mode: true,
            alternate_screen: true,
            ..TerminalState::default()
        };
        Ok((
            Self {
                state: Arc::new(Mutex::new(state)),
            },
            stdout,
        ))
    }

    fn weak_state(&self) -> Weak<Mutex<TerminalState>> {
        Arc::downgrade(&self.state)
    }

    fn enable_mouse_capture(&self, stdout: &mut Stdout) -> io::Result<()> {
        execute!(stdout, EnableMouseCapture)?;
        if let Ok(mut state) = self.state.lock() {
            state.mouse_capture = true;
        }
        Ok(())
    }

    fn push_keyboard_flags(&self, stdout: &mut Stdout, flags: KeyboardEnhancementFlags) {
        if execute!(stdout, PushKeyboardEnhancementFlags(flags)).is_ok() {
            if let Ok(mut state) = self.state.lock() {
                state.keyboard_flags_pushed = true;
            }
        }
    }

    fn enable_bracketed_paste(&self, stdout: &mut Stdout) -> io::Result<()> {
        execute!(stdout, EnableBracketedPaste)?;
        if let Ok(mut state) = self.state.lock() {
            state.bracketed_paste = true;
        }
        Ok(())
    }

    fn mark_cursor_hidden(&self, hidden: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.cursor_hidden = hidden;
        }
    }

    fn restore(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.restore();
        }
    }

    fn install_sigint_handler(&self) -> Result<(), CtrlcError> {
        let weak = Arc::downgrade(&self.state);
        ctrlc::set_handler(move || {
            if let Some(state) = weak.upgrade() {
                if let Ok(mut state) = state.lock() {
                    state.restore();
                }
            }
            std::process::exit(130);
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

fn install_terminal_panic_hook(state: Weak<Mutex<TerminalState>>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(state) = state.upgrade() {
            if let Ok(mut state) = state.lock() {
                state.restore();
            }
        }
        previous(info);
    }));
}

#[cfg(test)]
mod tests;
