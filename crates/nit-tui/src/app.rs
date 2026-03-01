use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::{
    codex_runner::{CodexCommand, CodexRunner},
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
        agent_console_view, agent_ops_view, bottom_bar, editor_view, file_tree_view,
        fuzzy_search_popup, games_analysis_popup, games_ca_sim_popup, games_match_history_popup,
        games_replay_popup, games_run_browser_popup, games_strategy_popup, games_tm_sim_popup,
        games_visualizer_view, gate_monitor_view, help_overlay, protocol_picker, rule_picker,
        top_bar, visualizer_view,
    },
};
use arboard::Clipboard;
use crossterm::{
    cursor::SetCursorStyle,
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseEvent,
        MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use nit_core::{
    actions::Action, apply_action, io as core_io, AgentAlert, AgentAlertSeverity, AgentBusEvent,
    AgentChannel, AgentDiagnosticEvent, AgentMessage, AgentOpsTab, AgentStatus, AppKind, AppState,
    McpConnectionState, MissionPhase, MissionRecord, Mode, PaneId, PatchProposal, PatchStatus,
    Prompt, SearchMode, UiSelection, UiSelectionPane, YankKind,
};
use nit_games::config::GamesConfig;
use ratatui::{
    backend::CrosstermBackend,
    style::Style,
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Terminal,
};

const TICK_RATE: Duration = Duration::from_millis(50);
const JOB_TICK: Duration = Duration::from_millis(120);
const BUSY_PULSE_INTERVAL: Duration = Duration::from_millis(550);
const CHORD_TIMEOUT: Duration = Duration::from_millis(300);
const INSPECTOR_JUMP_TIMEOUT: Duration = Duration::from_millis(1500);

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

pub fn run(mut state: AppState, theme: Theme, log_rx: Receiver<String>) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut guard = TerminalGuard {
        active: true,
        keyboard_flags_pushed: false,
        mouse_capture: false,
        bracketed_paste: false,
    };
    if execute!(stdout, EnableMouseCapture).is_ok() {
        guard.mouse_capture = true;
    }
    let keyboard_flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
    if execute!(stdout, PushKeyboardEnhancementFlags(keyboard_flags)).is_ok() {
        guard.keyboard_flags_pushed = true;
    }
    if execute!(stdout, EnableBracketedPaste).is_ok() {
        guard.bracketed_paste = true;
    }
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
    let editor_id = state.active_editor_buffer_id;
    let notes_id = state.notes_buffer_id;
    let warmup_editor = state.editor_buffer().bytes_len() <= 200_000;
    syntax.prime_buffer(editor_id, state.editor_buffer(), warmup_editor);
    syntax.prime_buffer(notes_id, state.notes_buffer(), false);

    let result = run_loop(&mut terminal, &mut state, &theme, &mut syntax, log_rx);

    terminal.show_cursor()?;
    if guard.keyboard_flags_pushed {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        guard.keyboard_flags_pushed = false;
    }
    if guard.mouse_capture {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        guard.mouse_capture = false;
    }
    if guard.bracketed_paste {
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        guard.bracketed_paste = false;
    }
    execute!(io::stdout(), SetCursorStyle::DefaultUserShape)?;
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    guard.active = false;
    let _ = save_notes_on_exit(&state);
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut AppState,
    theme: &Theme,
    syntax: &mut SyntaxRuntime,
    log_rx: Receiver<String>,
) -> io::Result<()> {
    let mut last_tick = Instant::now();
    let mut last_job = Instant::now();
    let mut last_vitals_sample = Instant::now();
    let mut last_busy_pulse = Instant::now();
    let mut needs_redraw = true;
    let mut input_state = InputState::new();
    let mut system_stats = SystemStats::new();
    let mut clipboard = Clipboard::new().ok();
    let mut file_tree_runner = FileTreeRunner::spawn();
    let mut codex_runner = CodexRunner::spawn();
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
                    if is_command_prompt_open_key(&key) {
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
                    if state.rule_picker.open {
                        if rule_picker::handle_key(&key, state) {
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.protocol_picker.open {
                        if protocol_picker::handle_key(&key, state) {
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.show_help {
                        if handle_help_popup_key(&key, state) {
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.games.analysis.open {
                        if handle_analysis_popup_key(&key, state) {
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
                        if handle_replay_popup_key(&key, state, screen) {
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
                        if handle_tm_sim_popup_key(&key, state, screen) {
                            clamp_modal_scroll_offsets(state, screen, theme);
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.app_kind == AppKind::Games && state.games.ca_sim.open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_ca_sim_popup_key(&key, state, screen) {
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
                    if handle_agent_station_key(key, state, &mut vitals, Some(&codex_runner)) {
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
                    if handle_mouse_event(
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
                Event::Resize(_, _) => {
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
            event.apply(state);
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
        if flush_agent_run_provenance(state).is_err() {
            let now = Instant::now();
            vitals.record_diag_event(now, DiagSeverity::Warn);
        }

        // redraw
        if needs_redraw || last_tick.elapsed() >= TICK_RATE {
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
    fuzzy_runtime.shutdown();
    if let Some(runtime) = games_config_preview.as_mut() {
        runtime.shutdown();
    }
    Ok(())
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

fn draw(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut AppState,
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
        let notes_cursor = agent_console_view::render(f, layout.notes, state, theme);
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
            agent_ops_view::render(f, layout.job, state, theme);
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
        } else if petri_visible {
            None
        } else if state.file_tree.open && state.focus == PaneId::Editor {
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
    let cursor_style = match state.mode {
        Mode::Insert => SetCursorStyle::SteadyBar,
        Mode::Normal | Mode::Visual => SetCursorStyle::SteadyBlock,
    };
    execute!(terminal.backend_mut(), cursor_style)?;
    state.metrics.last_render_ms = start.elapsed().as_millis();
    state.metrics.frame_count += 1;
    Ok(())
}

fn handle_agent_station_key(
    key: KeyEvent,
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
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
        PaneId::JobOutput => handle_agent_ops_key(key, state, vitals),
        PaneId::Notes => handle_agent_console_key(key, state, vitals, codex),
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

fn handle_agent_ops_key(key: KeyEvent, state: &mut AppState, vitals: &mut VitalsState) -> bool {
    if state.agents.dock_tab == AgentOpsTab::Scratchpad {
        match key {
            KeyEvent {
                code: KeyCode::Tab,
                modifiers: KeyModifiers::SHIFT,
                ..
            } => {
                state.agents.dock_tab = state.agents.dock_tab.prev();
                state.agents.roster_effort_selected = None;
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
                state.agents.roster_effort_selected = None;
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
                state.agents.roster_effort_selected = None;
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
                state.agents.roster_effort_selected = None;
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
            changed = enter_roster_effort_cursor(state);
        }
        KeyEvent {
            code: KeyCode::Char('h'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            changed = exit_roster_effort_cursor(state);
        }
        KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            changed = reset_roster_context(state);
        }
        KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::SHIFT,
            ..
        } => {
            state.agents.dock_tab = state.agents.dock_tab.prev();
            state.agents.roster_effort_selected = None;
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
            state.agents.roster_effort_selected = None;
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
            state.agents.roster_effort_selected = None;
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
            state.agents.roster_effort_selected = None;
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
            changed = move_agent_ops_selection(state, -1);
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
            changed = move_agent_ops_selection(state, 1);
        }
        KeyEvent {
            code: KeyCode::Char(' '),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster
            && state.agents.roster_effort_selected.is_some() =>
        {
            changed = select_roster_effort(state);
        }
        KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster
            && state.agents.roster_effort_selected.is_some() =>
        {
            changed = select_roster_effort(state);
        }
        KeyEvent {
            code: KeyCode::Enter,
            ..
        } => {
            state.focus = PaneId::Notes;
            state.mode = Mode::Normal;
            state.agents.console_scroll = usize::MAX;
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
            set_mcp_state(state, McpConnectionState::Connecting, Some(19), None);
            set_mcp_state(state, McpConnectionState::Connected, Some(7), None);
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Char('s'),
            modifiers,
            ..
        } if modifiers.is_empty() && state.agents.dock_tab == AgentOpsTab::Mcp => {
            set_mcp_state(state, McpConnectionState::Connected, Some(8), None);
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Char('x'),
            modifiers,
            ..
        } if modifiers.is_empty() && state.agents.dock_tab == AgentOpsTab::Mcp => {
            set_mcp_state(
                state,
                McpConnectionState::Disconnected,
                None,
                Some("MCP link stopped by operator".into()),
            );
            changed = true;
        }
        _ => {}
    }
    if changed {
        state.agents.note_event();
        vitals.record_agent_event(Instant::now());
    }
    changed
}

fn move_agent_ops_selection(state: &mut AppState, delta: i32) -> bool {
    match state.agents.dock_tab {
        AgentOpsTab::Roster => {
            if state.agents.agents.is_empty() {
                return false;
            }
            if let Some(effort_idx) = state.agents.roster_effort_selected {
                let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
                    state.agents.roster_effort_selected = None;
                    return true;
                };
                let efforts = state
                    .agents
                    .codex_supported_reasoning_efforts
                    .get(&agent.id)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                if efforts.is_empty() {
                    state.agents.roster_effort_selected = None;
                    return true;
                }
                let max = efforts.len().saturating_sub(1);
                if delta.is_negative() {
                    if effort_idx > 0 {
                        let next = effort_idx.saturating_sub(1);
                        if next != effort_idx {
                            state.agents.roster_effort_selected = Some(next);
                            return true;
                        }
                    }
                    state.agents.roster_effort_selected = None;
                    return true;
                }
                if delta > 0 {
                    if effort_idx < max {
                        let next = (effort_idx + 1).min(max);
                        if next != effort_idx {
                            state.agents.roster_effort_selected = Some(next);
                            return true;
                        }
                    }

                    // Walk out of the effort list when we hit the end.
                    state.agents.roster_effort_selected = None;
                    let agent_max = state.agents.agents.len().saturating_sub(1) as i32;
                    let next_agent =
                        (state.agents.roster_selected as i32 + 1).clamp(0, agent_max) as usize;
                    if next_agent == state.agents.roster_selected {
                        return true;
                    }
                    state.agents.roster_selected = next_agent;
                    if let Some(agent) = state.agents.agents.get(next_agent) {
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
                    return true;
                }
                return false;
            }

            let max = state.agents.agents.len().saturating_sub(1) as i32;
            let next = (state.agents.roster_selected as i32 + delta).clamp(0, max) as usize;
            if next == state.agents.roster_selected {
                return false;
            }
            state.agents.roster_selected = next;
            state.agents.roster_effort_selected = None;
            if let Some(agent) = state.agents.agents.get(next) {
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
            true
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
        | AgentOpsTab::Evidence
        | AgentOpsTab::Diagnostics
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

fn enter_roster_effort_cursor(state: &mut AppState) -> bool {
    let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
        state.agents.roster_effort_selected = None;
        return false;
    };
    let Some(efforts) = state
        .agents
        .codex_supported_reasoning_efforts
        .get(&agent.id)
        .map(|v| v.as_slice())
    else {
        state.agents.roster_effort_selected = None;
        return false;
    };
    if efforts.is_empty() {
        state.agents.roster_effort_selected = None;
        return false;
    }

    let current = state
        .agents
        .codex_selected_reasoning_effort
        .get(&agent.id)
        .or_else(|| state.agents.codex_default_reasoning_effort.get(&agent.id))
        .map(|s| s.as_str());
    let idx = current
        .and_then(|effort| efforts.iter().position(|e| e == effort))
        .unwrap_or(0)
        .min(efforts.len().saturating_sub(1));

    if state.agents.roster_effort_selected == Some(idx) {
        return false;
    }
    state.agents.roster_effort_selected = Some(idx);
    true
}

fn exit_roster_effort_cursor(state: &mut AppState) -> bool {
    if state.agents.roster_effort_selected.is_some() {
        state.agents.roster_effort_selected = None;
        return true;
    }
    false
}

fn select_roster_effort(state: &mut AppState) -> bool {
    let Some(effort_idx) = state.agents.roster_effort_selected else {
        return false;
    };
    let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
        return false;
    };
    let Some(efforts) = state
        .agents
        .codex_supported_reasoning_efforts
        .get(&agent.id)
    else {
        return false;
    };
    let Some(effort) = efforts.get(effort_idx) else {
        return false;
    };

    let effort = effort.trim();
    if effort.is_empty() {
        return false;
    }

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
    true
}

fn reset_roster_context(state: &mut AppState) -> bool {
    let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
        return false;
    };
    let agent_id = agent.id.clone();
    let agent_label = agent.role.trim();
    let is_codex = agent.is_codex();
    let mission_ctx = state
        .agents
        .selected_context_mission()
        .map(ToString::to_string);

    state.agents.roster_effort_selected = None;
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
        // In non-mission chat, the thread isn't partitioned by agent; reset the whole local thread.
        state.agents.codex_thread_ids.clear();
        state.agents.codex_used_tokens.clear();
        state.agents.messages.retain(|msg| msg.mission_id.is_some());
    }
    let removed = before.saturating_sub(state.agents.messages.len());
    state.agents.console_scroll = usize::MAX;

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
) -> bool {
    let mut changed = false;
    let mut handled = false;
    let mut follow_chat_cursor = false;
    let input_len_chars = state.agents.chat_input.chars().count();
    if state.agents.chat_input_cursor > input_len_chars {
        state.agents.chat_input_cursor = input_len_chars;
    }
    match key {
        KeyEvent {
            code: KeyCode::Enter,
            ..
        } => {
            handled = true;
            let prompt = state.agents.chat_input.clone();
            let mission_id = state
                .agents
                .selected_context_mission()
                .map(ToString::to_string);
            let model = state
                .agents
                .selected_context_agent()
                .map(ToString::to_string);
            changed = push_chat_message(state);
            follow_chat_cursor = changed;
            if changed {
                maybe_dispatch_codex_turn(state, vitals, codex, model, mission_id, prompt);
            }
        }
        KeyEvent {
            code: KeyCode::Char('c'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            handled = true;
            if !state.agents.chat_input.is_empty() {
                state.agents.chat_input.clear();
                state.agents.chat_input_cursor = 0;
                changed = true;
                follow_chat_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('\u{3}'),
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            handled = true;
            if !state.agents.chat_input.is_empty() {
                state.agents.chat_input.clear();
                state.agents.chat_input_cursor = 0;
                changed = true;
                follow_chat_cursor = true;
            }
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
                // In Agent Chat, Esc should clear any active thread selection before touching the
                // compose box contents.
                handled = true;
                state.ui_selection = None;
            }
        }
        KeyEvent {
            code: KeyCode::Backspace,
            ..
        } => {
            handled = true;
            if state.agents.chat_input_cursor > 0 {
                let remove_start = chat_input_byte_index(
                    &state.agents.chat_input,
                    state.agents.chat_input_cursor - 1,
                );
                let remove_end =
                    chat_input_byte_index(&state.agents.chat_input, state.agents.chat_input_cursor);
                state
                    .agents
                    .chat_input
                    .replace_range(remove_start..remove_end, "");
                state.agents.chat_input_cursor = state.agents.chat_input_cursor.saturating_sub(1);
                changed = true;
                follow_chat_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::Delete,
            ..
        } => {
            handled = true;
            if state.agents.chat_input_cursor < state.agents.chat_input.chars().count() {
                let remove_start =
                    chat_input_byte_index(&state.agents.chat_input, state.agents.chat_input_cursor);
                let remove_end = chat_input_byte_index(
                    &state.agents.chat_input,
                    state.agents.chat_input_cursor + 1,
                );
                state
                    .agents
                    .chat_input
                    .replace_range(remove_start..remove_end, "");
                changed = true;
                follow_chat_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::Left,
            ..
        } => {
            handled = true;
            if state.agents.chat_input_cursor > 0 {
                state.agents.chat_input_cursor = state.agents.chat_input_cursor.saturating_sub(1);
                changed = true;
                follow_chat_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::Right,
            ..
        } => {
            handled = true;
            let max = state.agents.chat_input.chars().count();
            if state.agents.chat_input_cursor < max {
                state.agents.chat_input_cursor += 1;
                changed = true;
                follow_chat_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::Home,
            ..
        } => {
            handled = true;
            if state.agents.chat_input_cursor != 0 {
                state.agents.chat_input_cursor = 0;
                changed = true;
                follow_chat_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::End, ..
        } => {
            handled = true;
            let max = state.agents.chat_input.chars().count();
            if state.agents.chat_input_cursor != max {
                state.agents.chat_input_cursor = max;
                changed = true;
                follow_chat_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers,
            ..
        } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
            handled = true;
            let insert_at =
                chat_input_byte_index(&state.agents.chat_input, state.agents.chat_input_cursor);
            state.agents.chat_input.insert(insert_at, c);
            state.agents.chat_input_cursor += 1;
            changed = true;
            follow_chat_cursor = true;
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
            code: KeyCode::Up, ..
        } => {
            handled = true;
            let moved = chat_cursor_move_vertical(
                &state.agents.chat_input,
                state.agents.chat_input_cursor,
                -1,
            );
            if moved != state.agents.chat_input_cursor {
                state.agents.chat_input_cursor = moved;
                changed = true;
                follow_chat_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::Down,
            ..
        } => {
            handled = true;
            let moved = chat_cursor_move_vertical(
                &state.agents.chat_input,
                state.agents.chat_input_cursor,
                1,
            );
            if moved != state.agents.chat_input_cursor {
                state.agents.chat_input_cursor = moved;
                changed = true;
                follow_chat_cursor = true;
            }
        }
        _ => {}
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

fn insert_chat_input_text(state: &mut AppState, text: &str) -> bool {
    let normalized = normalize_chat_input_text(text);
    if normalized.is_empty() {
        return false;
    }
    let insert_at = chat_input_byte_index(&state.agents.chat_input, state.agents.chat_input_cursor);
    state.agents.chat_input.insert_str(insert_at, &normalized);
    state.agents.chat_input_cursor = state
        .agents
        .chat_input_cursor
        .saturating_add(normalized.chars().count());
    state.agents.chat_input_scroll = usize::MAX;
    true
}

fn normalize_chat_input_text(text: &str) -> String {
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

fn insert_text_into_focused_buffer(
    state: &mut AppState,
    syntax: &mut SyntaxRuntime,
    text: &str,
) -> bool {
    if text.is_empty() {
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
        buffer.insert_str(text);
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

fn chat_input_byte_index(input: &str, char_idx: usize) -> usize {
    if char_idx == 0 {
        return 0;
    }
    input
        .char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(input.len())
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

fn maybe_dispatch_codex_turn(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    model: Option<String>,
    mission_id: Option<String>,
    prompt: String,
) {
    let Some(codex) = codex else {
        return;
    };
    let Some(model) = model else {
        return;
    };
    let is_codex = state
        .agents
        .agents
        .iter()
        .find(|lane| lane.id.as_str() == model.as_str())
        .is_some_and(|lane| lane.is_codex());
    if !is_codex {
        return;
    }

    let resume_thread_id = if let Some(mission_id) = mission_id.as_deref() {
        state
            .agents
            .codex_mission_thread_ids
            .get(mission_id)
            .and_then(|threads| threads.get(&model))
            .cloned()
    } else {
        state.agents.codex_thread_ids.get(&model).cloned()
    };
    // Always persist Codex sessions so non-mission chat can resume context across prompts.
    let persist_session = true;

    // Best-effort context remaining percentage for the breather row.
    if let Some(max_tokens) = state
        .agents
        .codex_effective_context_window_tokens
        .get(&model)
        .copied()
    {
        let prompt_tokens_est = estimate_codex_context_tokens(&prompt);
        let baseline_used = if let Some(mission_id) = mission_id.as_deref() {
            state
                .agents
                .codex_mission_used_tokens
                .get(mission_id)
                .and_then(|m| m.get(&model))
                .copied()
        } else {
            state.agents.codex_used_tokens.get(&model).copied()
        };
        let used_tokens = if let Some(baseline) = baseline_used {
            baseline.saturating_add(prompt_tokens_est).min(max_tokens)
        } else if let Some(mission_id) = mission_id.as_deref() {
            estimate_codex_context_tokens_for_mission(state, mission_id).min(max_tokens)
        } else {
            prompt_tokens_est.min(max_tokens)
        };
        if let Some(mission_id) = mission_id.as_deref() {
            state
                .agents
                .codex_mission_used_tokens
                .entry(mission_id.to_string())
                .or_default()
                .insert(model.clone(), used_tokens);
        } else {
            state
                .agents
                .codex_used_tokens
                .insert(model.clone(), used_tokens);
        }
        let remaining = max_tokens.saturating_sub(used_tokens);
        // Round to nearest percent so small prompts on large context windows still show 100%.
        let denom = max_tokens.max(1) as u64;
        let pct =
            (((remaining as u64).saturating_mul(100)).saturating_add(denom / 2) / denom) as u8;
        if let Some(mission_id) = mission_id.as_deref() {
            state
                .agents
                .codex_mission_context_remaining_pct
                .entry(mission_id.to_string())
                .or_default()
                .insert(model.clone(), pct);
        } else {
            state
                .agents
                .codex_context_remaining_pct
                .insert(model.clone(), pct);
        }
    } else {
        if let Some(mission_id) = mission_id.as_deref() {
            if let Some(map) = state
                .agents
                .codex_mission_context_remaining_pct
                .get_mut(mission_id)
            {
                map.remove(&model);
                if map.is_empty() {
                    state
                        .agents
                        .codex_mission_context_remaining_pct
                        .remove(mission_id);
                }
            }
        } else {
            state.agents.codex_context_remaining_pct.remove(&model);
        }
    }

    // Immediate UI feedback: mark the model as running and show the loader/breather row.
    let Some(agent) = state.agents.agents.iter_mut().find(|a| a.id == model) else {
        return;
    };
    agent.status = AgentStatus::Running;
    agent.queue_len = agent.queue_len.saturating_add(1).max(1);
    agent.heartbeat_age_secs = 0;
    agent.last_message = "queued".into();
    // Always reflect the active mission context (including clearing it for non-mission chat).
    agent.current_mission = mission_id.clone();

    let now = Instant::now();
    state.agents.active_turns.insert(
        model.clone(),
        nit_core::state::AgentTurnState {
            started_at: now,
            last_heartbeat_at: now,
            last_output_at: now,
            stage: Some("queued".into()),
        },
    );

    state.agents.mcp.state = McpConnectionState::Connecting;
    state.agents.mcp.last_error = None;
    state.agents.note_event();
    vitals.record_agent_event(now);

    let reasoning_effort = state
        .agents
        .codex_selected_reasoning_effort
        .get(&model)
        .cloned()
        .or_else(|| {
            state
                .agents
                .codex_default_reasoning_effort
                .get(&model)
                .cloned()
        })
        .unwrap_or_else(|| "medium".into());

    codex.send(CodexCommand::RunTurn {
        model,
        cwd: state.workspace_root.clone(),
        mission_id,
        resume_thread_id,
        persist_session,
        reasoning_effort: Some(reasoning_effort),
        prompt,
    });
}

fn estimate_codex_context_tokens(text: &str) -> u32 {
    // Fast heuristic: ~4 bytes per token for typical English/code mixtures.
    // This keeps the UI responsive and avoids bringing in a tokenizer dependency.
    if text.is_empty() {
        return 0;
    }
    let bytes = text.as_bytes().len() as u32;
    (bytes + 3) / 4
}

fn estimate_codex_context_tokens_for_mission(state: &mut AppState, mission_id: &str) -> u32 {
    if let Some(tokens) = state
        .agents
        .codex_estimated_tokens_used_by_mission
        .get(mission_id)
        .copied()
    {
        return tokens;
    }
    let tokens = state
        .agents
        .messages
        .iter()
        .filter(|msg| msg.mission_id.as_deref() == Some(mission_id))
        .fold(0u32, |acc, msg| {
            acc.saturating_add(estimate_codex_context_tokens(&msg.text))
        });
    state
        .agents
        .codex_estimated_tokens_used_by_mission
        .insert(mission_id.to_string(), tokens);
    tokens
}

fn push_chat_message(state: &mut AppState) -> bool {
    let text = state.agents.chat_input.clone();
    if text.trim().is_empty() {
        return false;
    }
    let message = AgentMessage {
        at: timestamp_label(state),
        channel: state.agents.chat_channel,
        agent_id: None,
        mission_id: state
            .agents
            .selected_context_mission()
            .map(ToString::to_string),
        text: text.clone(),
    };
    if let Some(mission_id) = message.mission_id.as_deref() {
        mark_mission_provenance_dirty(state, mission_id);
        let delta = estimate_codex_context_tokens(&text);
        let entry = state
            .agents
            .codex_estimated_tokens_used_by_mission
            .entry(mission_id.to_string())
            .or_insert(0);
        *entry = entry.saturating_add(delta);
    }
    state.agents.messages.push(message);
    state.agents.diag_events.push(AgentDiagnosticEvent {
        severity: AgentAlertSeverity::Info,
        source: "thread".into(),
        message: format!("sent message: {text}"),
        at: timestamp_label(state),
    });
    state.agents.console_scroll = usize::MAX;
    state.agents.chat_input.clear();
    state.agents.chat_input_cursor = 0;
    state.agents.chat_input_scroll = usize::MAX;
    true
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
        id: format!("patch-{:03}", patch_base),
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

fn set_mcp_state(
    state: &mut AppState,
    connection_state: McpConnectionState,
    latency_ms: Option<u64>,
    last_error: Option<String>,
) {
    state.agents.mcp.state = connection_state;
    state.agents.mcp.latency_ms = latency_ms;
    state.agents.mcp.last_error = last_error.clone();
    let message = match last_error {
        Some(err) => format!("MCP {} ({err})", connection_state.label()),
        None => format!("MCP {}", connection_state.label()),
    };
    let severity = if matches!(connection_state, McpConnectionState::Error) {
        AgentAlertSeverity::Error
    } else {
        AgentAlertSeverity::Info
    };
    state.agents.alerts.push(AgentAlert {
        severity,
        source: "mcp".into(),
        message: message.clone(),
        at: timestamp_label(state),
    });
    state.agents.diag_events.push(AgentDiagnosticEvent {
        severity,
        source: "mcp".into(),
        message,
        at: timestamp_label(state),
    });
}

fn mark_mission_provenance_dirty(state: &mut AppState, mission_id: &str) {
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

fn timestamp_label(state: &AppState) -> String {
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

    if is_command_prompt_open_key(&key) {
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
    let editor_id = state.active_editor_buffer_id;
    let notes_id = state.notes_buffer_id;
    let editor_version = state.editor_buffer().version();
    let notes_version = state.notes_buffer().version();
    let outcome = apply_action(state, action.clone());

    log_action(state, &action, before_focus, before_mode, before_debug);

    if state.editor_buffer().version() != editor_version {
        let buf = state.editor_buffer_mut();
        syntax.note_buffer_change(editor_id, buf);
    }
    if state.notes_buffer().version() != notes_version {
        let buf = state.notes_buffer_mut();
        syntax.note_buffer_change(notes_id, buf);
    }

    if matches!(action, Action::ToggleSyntax) {
        syntax.update_config(state.settings.highlight.clone());
        syntax.prime_buffer(editor_id, state.editor_buffer(), true);
        syntax.prime_buffer(notes_id, state.notes_buffer(), false);
    }
    if matches!(action, Action::OpenFile(_)) {
        // Avoid blocking highlight warmup when hopping files from NITTree.
        syntax.prime_buffer(editor_id, state.editor_buffer(), false);
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
                state.yank_kind = if state.yank.as_ref().map_or(false, |t| t.contains('\n')) {
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
    match key {
        KeyEvent {
            code: KeyCode::Enter | KeyCode::Char('\n') | KeyCode::Char('\r'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => true,
        _ => false,
    }
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
    match key {
        KeyEvent {
            code: KeyCode::Char('*') | KeyCode::Char('8'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => true,
        _ => false,
    }
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
            | KeyCode::Char('h')
            | KeyCode::Char('H')
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

fn handle_mouse_event(
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
                                .saturating_sub(delta.abs() as usize);
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

            if state.show_help {
                let area = dynamic_popup_rect(screen, help_overlay::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    bump_scroll(&mut state.help_scroll, delta);
                }
                return true;
            }

            if state.app_kind == AppKind::Games && state.games.analysis.open {
                let area = dynamic_popup_rect(screen, games_analysis_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    bump_scroll(&mut state.games.analysis.scroll_offset, delta);
                }
                return true;
            }

            if state.app_kind == AppKind::Games && state.games.run_browser.open {
                let area =
                    dynamic_popup_rect(screen, games_run_browser_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    bump_scroll(&mut state.games.run_browser.scroll_offset, delta);
                }
                return true;
            }

            if state.app_kind == AppKind::Games && state.games.replay.open {
                let area = dynamic_popup_rect(screen, games_replay_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    bump_scroll(&mut state.games.replay.scroll_offset, delta);
                }
                return true;
            }

            if state.app_kind == AppKind::Games && state.games.strategy_inspect.open {
                let area = dynamic_popup_rect(screen, games_strategy_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    bump_scroll(&mut state.games.strategy_inspect.scroll_offset, delta);
                }
                return true;
            }
            if state.app_kind == AppKind::Games && state.games.tm_sim.open {
                let area = dynamic_popup_rect(screen, games_tm_sim_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    bump_scroll(&mut state.games.tm_sim.scroll_offset, delta);
                }
                return true;
            }
            if state.app_kind == AppKind::Games && state.games.ca_sim.open {
                let area = dynamic_popup_rect(screen, games_ca_sim_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    bump_scroll(&mut state.games.ca_sim.scroll_offset, delta);
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
                    let lines = agent_console_view::thread_lines_for_selection(
                        state,
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
                    let lines = agent_ops_view::current_lines_for_width(state, text_width);
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
            handle_mouse_down(mouse, screen, state, input_state, clipboard, theme)
        }
        MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
            handle_mouse_drag(mouse, screen, state, input_state, clipboard, theme)
        }
        MouseEventKind::Up(crossterm::event::MouseButton::Left) => {
            input_state.mouse_select_anchor = None;
            true
        }
        _ => false,
    }
}

const SCROLL_LINES: usize = 1;
const SCROLL_LINES_FAST: usize = 5;

fn bump_scroll(value: &mut usize, delta: i32) {
    if delta.is_negative() {
        *value = value.saturating_sub(delta.abs() as usize);
    } else if delta > 0 {
        *value = value.saturating_add(delta as usize);
    }
}

fn popup_max_scroll(line_count: usize, text_area: ratatui::layout::Rect) -> usize {
    line_count.saturating_sub(text_area.height as usize)
}

fn max_scroll_for_height(line_count: usize, height: usize) -> usize {
    line_count.saturating_sub(height)
}

fn help_popup_max_scroll(screen: ratatui::layout::Rect, theme: &Theme) -> usize {
    let area = dynamic_popup_rect(screen, help_overlay::preferred_size(screen));
    let text_area = popup_text_area(area);
    let lines = help_overlay::build_lines(theme);
    popup_max_scroll(lines.len(), text_area)
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

fn map_agent_console_mouse(
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
    let lines = agent_console_view::thread_lines_for_selection(state, text_area.width as usize);
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

fn map_job_output_mouse(
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
    let lines = agent_ops_view::current_lines_for_width(state, text_width);
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
        config_result.and_then(|result| result.as_ref().ok()),
    );
    let Some(side_area) = layout_info.side else {
        return None;
    };
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
        // Keep in sync with Agent Ops layout: tabs + body + footer hints.
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    chunks[1]
}

fn agent_ops_scratchpad_editor_area(area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::widgets::{Block, Borders};
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        // Keep in sync with Agent Ops layout: tabs + body + footer hints.
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
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
    for line_idx in start_line..=end_line {
        let line = &lines[line_idx];
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

fn slice_by_char(input: &str, start: usize, end: usize) -> String {
    if start >= end {
        return String::new();
    }
    let mut start_byte = None;
    let mut end_byte = None;
    let mut count = 0usize;
    for (idx, _) in input.char_indices() {
        if count == start {
            start_byte = Some(idx);
        }
        if count == end {
            end_byte = Some(idx);
            break;
        }
        count += 1;
    }
    let start_byte = start_byte.unwrap_or_else(|| input.len());
    let end_byte = end_byte.unwrap_or_else(|| input.len());
    input[start_byte..end_byte].to_string()
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

fn handle_mouse_down(
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
        state.agents.chat_input_cursor =
            cursor_char_idx.min(state.agents.chat_input.chars().count());
        input_state.mouse_select_anchor = None;
        return true;
    }
    if let Some((line_idx, col, lines)) = map_agent_console_mouse(mouse, screen, state, false) {
        state.focus = PaneId::Notes;
        state.mode = Mode::Normal;
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
        map_job_output_mouse(mouse, screen, state, false)
    {
        state.focus = PaneId::JobOutput;
        apply_agent_ops_click_selection(state, line_idx, text_width);
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

fn apply_agent_ops_click_selection(state: &mut AppState, line_idx: usize, text_width: usize) {
    if line_idx < 2 {
        return;
    }
    let data_line = line_idx.saturating_sub(2);
    match state.agents.dock_tab {
        AgentOpsTab::Roster => {
            let Some(meta) = agent_ops_view::roster_meta_for_body_line(state, data_line) else {
                return;
            };
            state.agents.roster_selected = meta.agent_idx;
            state.agents.roster_effort_selected = meta.effort_idx;
            if let Some(agent) = state.agents.agents.get(meta.agent_idx) {
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

                if let Some(effort_idx) = meta.effort_idx {
                    if let Some(efforts) = state
                        .agents
                        .codex_supported_reasoning_efforts
                        .get(&agent.id)
                    {
                        if let Some(effort) = efforts.get(effort_idx).cloned() {
                            state
                                .agents
                                .codex_selected_reasoning_effort
                                .insert(agent.id.clone(), effort);
                        }
                    }
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
        AgentOpsTab::Patch
        | AgentOpsTab::Evidence
        | AgentOpsTab::Diagnostics
        | AgentOpsTab::Scratchpad => {}
        AgentOpsTab::Mcp => {}
    }
}

fn handle_mouse_drag(
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
        MouseSelectTarget::Ui(pane) => {
            let result = match pane {
                UiSelectionPane::JobOutput => map_job_output_mouse(mouse, screen, state, true)
                    .map(|(line_idx, col, _text_width, lines)| (line_idx, col, lines)),
                UiSelectionPane::AgentConsole => {
                    map_agent_console_mouse(mouse, screen, state, true)
                        .map(|(line_idx, col, lines)| (line_idx, col, lines))
                }
                UiSelectionPane::GamesPetriDish => {
                    map_games_petri_mouse(mouse, screen, state, true)
                        .map(|(line_idx, col, lines)| (line_idx, col, lines))
                }
                UiSelectionPane::VisualizerMain => {
                    map_visualizer_main_mouse(mouse, screen, state, theme, true)
                        .map(|(line_idx, col, lines)| (line_idx, col, lines))
                }
                UiSelectionPane::VisualizerSide => {
                    map_visualizer_side_mouse(mouse, screen, state, theme, true)
                        .map(|(line_idx, col, lines)| (line_idx, col, lines))
                }
                UiSelectionPane::GateMonitor => {
                    map_gate_monitor_mouse(mouse, screen, state, theme, true)
                        .map(|(line_idx, col, lines)| (line_idx, col, lines))
                }
                UiSelectionPane::HelpPopup => {
                    map_help_popup_mouse(mouse, screen, state, theme, true)
                        .map(|(line_idx, col, lines)| (line_idx, col, lines))
                }
                UiSelectionPane::GamesAnalysisPopup => {
                    map_analysis_popup_mouse(mouse, screen, state, theme, true)
                        .map(|(line_idx, col, lines)| (line_idx, col, lines))
                }
                UiSelectionPane::GamesRunBrowserPopup => {
                    map_run_browser_popup_mouse(mouse, screen, state, theme, true)
                        .map(|(line_idx, col, lines)| (line_idx, col, lines))
                }
                UiSelectionPane::GamesReplayPopup => {
                    map_replay_popup_mouse(mouse, screen, state, theme, true)
                        .map(|(line_idx, col, lines)| (line_idx, col, lines))
                }
                UiSelectionPane::GamesStrategyPopup => {
                    map_strategy_popup_mouse(mouse, screen, state, theme, true)
                        .map(|(line_idx, col, lines)| (line_idx, col, lines))
                }
                UiSelectionPane::GamesTmSimPopupLeft | UiSelectionPane::GamesTmSimPopupRight => {
                    map_tm_sim_popup_mouse_for_pane(mouse, screen, state, theme, true, pane)
                        .map(|(line_idx, col, lines)| (line_idx, col, lines))
                }
                UiSelectionPane::GamesCaSimPopupLeft | UiSelectionPane::GamesCaSimPopupRight => {
                    map_ca_sim_popup_mouse_for_pane(mouse, screen, state, theme, true, pane)
                        .map(|(line_idx, col, lines)| (line_idx, col, lines))
                }
                UiSelectionPane::GamesMatchHistoryPopup => {
                    map_match_history_popup_mouse(mouse, screen, state, theme, true)
                        .map(|(line_idx, col, lines)| (line_idx, col, lines))
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
    let mut count = 0;
    for ch in line.chars() {
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
        count += 1;
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

fn handle_analysis_popup_key(key: &KeyEvent, state: &mut AppState) -> bool {
    if !state.games.analysis.open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
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
                    if state.editor_buffer().is_dirty() {
                        state.status =
                            Some("Unsaved changes - save (Ctrl+S) before opening a file".into());
                        return true;
                    }
                    let path = row.path.clone();
                    let _ = apply_action_with_syntax(state, syntax, Action::OpenFile(path));
                    state.file_tree.open = false;
                    true
                }
                nit_core::FileTreeKind::Dir => true,
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
            let old_anchor = file_tree::anchor_dir(state);
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
            let preserve = file_tree::selected_path(state);
            let new_anchor = file_tree::anchor_dir(state);
            if new_anchor != old_anchor {
                file_tree::rebuild_view(state, Some(preserve));
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
            code: KeyCode::Enter,
            ..
        } => match state.fuzzy_search.mode {
            SearchMode::Files => {
                let Some(item) = state
                    .fuzzy_search
                    .file_results
                    .get(state.fuzzy_search.selected)
                else {
                    return true;
                };
                if state.editor_buffer().is_dirty() {
                    state.status =
                        Some("Unsaved changes - save (Ctrl+S) before opening a file".into());
                    return true;
                }
                let path = item.abs_path.clone();
                let _ = apply_action_with_syntax(state, syntax, Action::OpenFile(path));
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
                if state.editor_buffer().is_dirty() {
                    state.status =
                        Some("Unsaved changes - save (Ctrl+S) before opening a file".into());
                    return true;
                }
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
            code: KeyCode::Char('\n'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            // Some terminals report Ctrl+J as a raw control character.
            let len = fuzzy_results_len(state);
            if len > 0 {
                state.fuzzy_search.selected = (state.fuzzy_search.selected + 1).min(len - 1);
            }
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
            state.games.run_browser.selected = state.games.run_browser.selected.saturating_sub(10);
            adjust_run_browser_scroll(state, screen);
            true
        }
        KeyCode::PageDown => {
            let max = state.games.run_browser.entries.len().saturating_sub(1);
            state.games.run_browser.selected = (state.games.run_browser.selected + 10).min(max);
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
) -> bool {
    if state.app_kind != AppKind::Games || !state.games.replay.open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
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
                bump_scroll(&mut state.games.replay.scroll_offset, -1);
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
                bump_scroll(&mut state.games.replay.scroll_offset, 1);
            }
            true
        }
        KeyCode::PageUp => {
            if state.games.replay.lines.is_empty() {
                state.games.replay.selected_index =
                    state.games.replay.selected_index.saturating_sub(10);
                adjust_replay_scroll(state, screen);
            } else {
                bump_scroll(&mut state.games.replay.scroll_offset, -10);
            }
            true
        }
        KeyCode::PageDown => {
            if state.games.replay.lines.is_empty() {
                let max = games_replay_popup::pair_list(state).len().saturating_sub(1);
                state.games.replay.selected_index =
                    (state.games.replay.selected_index + 10).min(max);
                adjust_replay_scroll(state, screen);
            } else {
                bump_scroll(&mut state.games.replay.scroll_offset, 10);
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
                bump_scroll(&mut state.games.strategy_inspect.scroll_offset, -1);
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
                bump_scroll(&mut state.games.strategy_inspect.scroll_offset, 1);
            }
            true
        }
        KeyCode::PageUp => {
            if state.games.strategy_inspect.lines.is_empty() {
                state.games.strategy_inspect.selected_index = state
                    .games
                    .strategy_inspect
                    .selected_index
                    .saturating_sub(10);
                adjust_strategy_scroll(state, screen);
            } else {
                bump_scroll(&mut state.games.strategy_inspect.scroll_offset, -10);
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
                    (state.games.strategy_inspect.selected_index + 10).min(max);
                adjust_strategy_scroll(state, screen);
            } else {
                bump_scroll(&mut state.games.strategy_inspect.scroll_offset, 10);
            }
            true
        }
        _ => true,
    }
}

fn handle_tm_sim_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    _screen: ratatui::layout::Rect,
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
            bump_scroll(&mut state.games.tm_sim.scroll_offset, -1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            bump_scroll(&mut state.games.tm_sim.scroll_offset, 1);
            true
        }
        KeyCode::PageUp => {
            bump_scroll(&mut state.games.tm_sim.scroll_offset, -10);
            true
        }
        KeyCode::PageDown => {
            bump_scroll(&mut state.games.tm_sim.scroll_offset, 10);
            true
        }
        _ => true,
    }
}

fn handle_ca_sim_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    _screen: ratatui::layout::Rect,
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
            bump_scroll(&mut state.games.ca_sim.scroll_offset, -1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            bump_scroll(&mut state.games.ca_sim.scroll_offset, 1);
            true
        }
        KeyCode::PageUp => {
            bump_scroll(&mut state.games.ca_sim.scroll_offset, -10);
            true
        }
        KeyCode::PageDown => {
            bump_scroll(&mut state.games.ca_sim.scroll_offset, 10);
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

fn handle_help_popup_key(key: &KeyEvent, state: &mut AppState) -> bool {
    if !state.show_help {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }
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
        true
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

fn flush_agent_run_provenance(state: &mut AppState) -> io::Result<()> {
    let pending = std::mem::take(&mut state.agents.pending_provenance_mission_ids);
    if pending.is_empty() {
        return Ok(());
    }
    let unique = pending.into_iter().collect::<BTreeSet<_>>();
    for mission_id in unique {
        write_agent_run_provenance(state, &mission_id)?;
    }
    Ok(())
}

fn write_agent_run_provenance(state: &AppState, mission_id: &str) -> io::Result<()> {
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

    let patches = state
        .agents
        .patches
        .iter()
        .filter(|patch| patch.mission_id.as_deref() == Some(mission_id))
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
        "patches": patches
            .iter()
            .map(|patch| {
                serde_json::json!({
                    "id": patch.id,
                    "agent_id": patch.agent_id,
                    "title": patch.title,
                    "status": patch.status.label(),
                })
            })
            .collect::<Vec<_>>(),
    });
    let run_json = serde_json::to_string_pretty(&run_payload)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, format!("serde run.json: {err}")))?;
    fs::write(run_dir.join("run.json"), run_json)?;

    let mut thread_md = String::new();
    thread_md.push_str(&format!("# Mission {}\n\n", mission.id));
    thread_md.push_str(&format!("Title: {}\n\n", mission.title));
    thread_md.push_str("## Thread\n\n");
    for msg in state.agents.messages.iter().filter(|msg| {
        msg.mission_id.as_deref() == Some(mission_id)
            || (msg.mission_id.is_none() && matches!(msg.channel, AgentChannel::Broadcast))
    }) {
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

    for patch in patches {
        let filename = format!("{}.diff", sanitize_for_filename(&patch.id));
        fs::write(patches_dir.join(filename), &patch.diff)?;
    }
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

struct TerminalGuard {
    active: bool,
    keyboard_flags_pushed: bool,
    mouse_capture: bool,
    bracketed_paste: bool,
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            if self.keyboard_flags_pushed {
                let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
            }
            if self.mouse_capture {
                let _ = execute!(io::stdout(), DisableMouseCapture);
            }
            if self.bracketed_paste {
                let _ = execute!(io::stdout(), DisableBracketedPaste);
            }
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), SetCursorStyle::DefaultUserShape);
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::{agent_console_view, agent_ops_view};
    use nit_core::AgentBusEvent;

    fn state_for_test() -> AppState {
        let editor = nit_core::Buffer::from_str("editor", "", None);
        let notes = nit_core::Buffer::from_str("notes", "", None);
        AppState::new(std::path::PathBuf::from("."), editor, notes)
    }

    #[test]
    fn fuzzy_popup_size_matches_preferred_when_tree_closed() {
        let state = state_for_test();
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 60,
        };
        let expected = fuzzy_search_popup::preferred_size(screen);
        assert_eq!(fuzzy_popup_size(screen, &state), expected);
    }

    #[test]
    fn fuzzy_popup_size_matches_preferred_when_tree_open() {
        let mut state = state_for_test();
        state.file_tree.open = true;
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 60,
        };
        let expected = fuzzy_search_popup::preferred_size(screen);
        assert_eq!(fuzzy_popup_size(screen, &state), expected);
    }

    #[test]
    fn agent_ops_space_does_not_toggle_pause() {
        let mut state = state_for_test();
        state.focus = PaneId::JobOutput;
        let mut input = InputState::new();
        let key = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::empty());
        let action = map_key_to_action(key, &state, &mut input);
        assert!(action.is_none());
    }

    #[test]
    fn codex_like_event_stream_updates_agent_panes() {
        let mut state = state_for_test();

        // Simulate an external runtime (Codex/Claude/etc.) driving NIT via NDJSON events.
        let events = [
            r#"{"type":"mcp_status","status":{"state":"Connected","endpoint":"stdio://nit-agentd","latency_ms":7,"last_error":null}}"#,
            r#"{"type":"mission_upsert","mission":{"id":"mis-001","title":"Wire up Codex runtime","phase":"Execute","swarm":false,"assigned_agents":["codex"],"status":"RUNNING","updated_at":"t+1"}}"#,
            r#"{"type":"agent_upsert","agent":{"id":"codex","role":"Coder","lane":"Lane A","status":"Running","heartbeat_age_secs":1,"queue_len":1,"current_mission":"mis-001","last_message":"boot"}}"#,
            r#"{"type":"message_append","message":{"at":"t+2","channel":"Agent","agent_id":null,"mission_id":"mis-001","text":"Please integrate Codex."}}"#,
            r#"{"type":"message_append","message":{"at":"t+3","channel":"Agent","agent_id":"codex","mission_id":"mis-001","text":"Acknowledged. Streaming events into AgentsState now."}}"#,
            r#"{"type":"alert_append","alert":{"severity":"Warn","source":"codex","message":"This is a long alert message that should wrap into multiple lines in the Agent Ops Alerts table for smaller widths.","at":"t+4"}}"#,
        ];

        let start_epoch = state.agents.event_epoch;
        for json in events {
            let ev: AgentBusEvent = serde_json::from_str(json).expect("parse AgentBusEvent");
            ev.apply(&mut state);
        }
        assert!(state.agents.event_epoch > start_epoch);

        // Roster tab should show the Codex agent.
        state.agents.dock_tab = AgentOpsTab::Roster;
        let roster = agent_ops_view::current_lines_for_width(&state, 72);
        assert!(roster.iter().any(|line| line.contains("Coder")));

        // Missions tab should show the mission + agent list in a vertical column.
        state.agents.dock_tab = AgentOpsTab::Missions;
        let missions = agent_ops_view::current_lines_for_width(&state, 72);
        assert!(missions.iter().any(|line| line.contains("mis-001")));

        // Alerts tab should wrap long messages and keep click mapping stable across wrapped rows.
        let alert_width = 48usize;
        state.agents.dock_tab = AgentOpsTab::Alerts;
        let alerts = agent_ops_view::current_lines_for_width(&state, alert_width);
        assert!(alerts.len() >= 5); // header + separator + at least two wrapped rows
        assert!(alerts[2].contains("WARN"));
        assert_eq!(
            agent_ops_view::alert_index_for_body_line(&state, alert_width, 0),
            Some(0)
        );
        assert_eq!(
            agent_ops_view::alert_index_for_body_line(&state, alert_width, 1),
            Some(0)
        );

        // Thread selection/export should include both user and agent message content.
        let thread = agent_console_view::thread_lines_for_selection(&state, 80).join("\n");
        assert!(thread.contains("Please integrate Codex."));
        assert!(thread.contains("Streaming events into AgentsState"));
    }

    #[test]
    fn codex_turn_completed_stores_mission_thread_id_and_marks_live() {
        let mut state = state_for_test();
        state.agents.missions.push(MissionRecord {
            id: "mis-001".into(),
            title: "Test mission".into(),
            phase: MissionPhase::Execute,
            swarm: false,
            assigned_agents: vec!["gpt-5.1-codex-mini".into()],
            status: "QUEUED".into(),
            updated_at: "t+0".into(),
        });
        state.agents.selected_mission = Some("mis-001".into());
        state.agents.mission_selected = 0;
        state.agents.agents.push(nit_core::AgentLane {
            id: "gpt-5.1-codex-mini".into(),
            role: "gpt-5.1-codex-mini".into(),
            lane: "Codex".into(),
            kind: nit_core::AgentLaneKind::Codex,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
        });

        AgentBusEvent::TurnCompleted {
            agent_id: "gpt-5.1-codex-mini".into(),
            mission_id: Some("mis-001".into()),
            thread_id: Some("thread-123".into()),
            token_count: None,
            message: "ok".into(),
        }
        .apply(&mut state);

        assert_eq!(
            state
                .agents
                .codex_mission_thread_ids
                .get("mis-001")
                .and_then(|threads| threads.get("gpt-5.1-codex-mini"))
                .map(|s| s.as_str()),
            Some("thread-123")
        );
        assert_eq!(state.agents.missions[0].status.to_ascii_uppercase(), "LIVE");
        assert!(state
            .agents
            .messages
            .iter()
            .any(|msg| msg.mission_id.as_deref() == Some("mis-001")
                && msg.agent_id.as_deref() == Some("gpt-5.1-codex-mini")
                && msg.text == "ok"));
    }

    #[test]
    fn reset_context_in_mission_forgets_codex_thread_id_and_clears_mission_thread() {
        let mut state = state_for_test();
        state.agents.missions.push(MissionRecord {
            id: "mis-001".into(),
            title: "Test mission".into(),
            phase: MissionPhase::Execute,
            swarm: true,
            assigned_agents: vec!["gpt-5.1-codex-mini".into(), "gpt-5.3-codex".into()],
            status: "LIVE".into(),
            updated_at: "t+0".into(),
        });
        state.agents.selected_mission = Some("mis-001".into());
        state.agents.mission_selected = 0;
        state.agents.agents.push(nit_core::AgentLane {
            id: "gpt-5.1-codex-mini".into(),
            role: "gpt-5.1-codex-mini".into(),
            lane: "Codex".into(),
            kind: nit_core::AgentLaneKind::Codex,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: Some("mis-001".into()),
            last_message: String::new(),
        });
        state.agents.selected_agent = Some("gpt-5.1-codex-mini".into());
        state.agents.roster_selected = 0;

        state
            .agents
            .codex_mission_thread_ids
            .entry("mis-001".into())
            .or_default()
            .insert("gpt-5.1-codex-mini".into(), "thread-123".into());
        state.agents.messages.push(AgentMessage {
            at: "t+1".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: Some("mis-001".into()),
            text: "hello".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "t+2".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("gpt-5.1-codex-mini".into()),
            mission_id: Some("mis-001".into()),
            text: "world".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "t+3".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: Some("mis-999".into()),
            text: "other mission".into(),
        });

        assert!(reset_roster_context(&mut state));
        assert!(state
            .agents
            .codex_mission_thread_ids
            .get("mis-001")
            .is_none());
        assert!(state
            .agents
            .messages
            .iter()
            .all(|msg| msg.mission_id.as_deref() != Some("mis-001")));
        assert!(state
            .agents
            .messages
            .iter()
            .any(|msg| msg.mission_id.as_deref() == Some("mis-999")));
    }

    #[test]
    fn ctrl_focus_hotkeys_target_expected_panes() {
        let mut input = InputState::new();
        let state = state_for_test();
        let editor = map_key_to_action(
            KeyEvent::new(KeyCode::Char('1'), KeyModifiers::CONTROL),
            &state,
            &mut input,
        );
        assert_eq!(editor, Some(Action::FocusPane(PaneId::Editor)));
        let ops = map_key_to_action(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::CONTROL),
            &state,
            &mut input,
        );
        assert_eq!(ops, Some(Action::FocusPane(PaneId::JobOutput)));
        let console = map_key_to_action(
            KeyEvent::new(KeyCode::Char('3'), KeyModifiers::CONTROL),
            &state,
            &mut input,
        );
        assert_eq!(console, Some(Action::FocusPane(PaneId::Notes)));
    }

    #[test]
    fn ctrl_q_quits_but_ctrl_c_does_not() {
        let mut input = InputState::new();
        let state = state_for_test();
        let quit = map_key_to_action(
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
            &state,
            &mut input,
        );
        assert_eq!(quit, Some(Action::Quit));
        let no_quit = map_key_to_action(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &state,
            &mut input,
        );
        assert_eq!(no_quit, None);
    }

    #[test]
    fn agent_chat_accepts_input_and_sends_on_enter() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.agents.console_scroll = 9;
        let mut vitals = VitalsState::default();

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
            &mut state,
            &mut vitals,
            None
        ));
        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE),
            &mut state,
            &mut vitals,
            None
        ));
        assert_eq!(state.agents.chat_input, "hi");
        assert_eq!(state.agents.chat_input_cursor, 2);

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
            &mut vitals,
            None
        ));
        assert_eq!(state.agents.chat_input, "");
        assert_eq!(state.agents.chat_input_cursor, 0);
        assert_eq!(state.agents.console_scroll, usize::MAX);
        assert_eq!(state.agents.messages.len(), 1);
        assert_eq!(state.agents.messages[0].text, "hi");
    }

    #[test]
    fn ctrl_c_clears_chat_input_when_chat_focused() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.agents.chat_input = "clear me".into();
        state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
        let mut vitals = VitalsState::default();

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &mut state,
            &mut vitals,
            None
        ));
        assert_eq!(state.agents.chat_input, "");
        assert_eq!(state.agents.chat_input_cursor, 0);
    }

    #[test]
    fn agent_chat_left_right_moves_cursor_and_inserts_at_cursor() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.agents.chat_input = "helo".into();
        state.agents.chat_input_cursor = 4;
        let mut vitals = VitalsState::default();

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Left, KeyModifiers::empty()),
            &mut state,
            &mut vitals,
            None
        ));
        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::empty()),
            &mut state,
            &mut vitals,
            None
        ));
        assert_eq!(state.agents.chat_input, "hello");

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Right, KeyModifiers::empty()),
            &mut state,
            &mut vitals,
            None
        ));
        assert_eq!(state.agents.chat_input_cursor, 5);
    }

    #[test]
    fn agent_chat_up_down_moves_cursor_between_lines() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.agents.chat_input = "abcd\nxy\nlast".into();
        state.agents.chat_input_cursor = 3;
        let mut vitals = VitalsState::default();

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
            &mut state,
            &mut vitals,
            None
        ));
        assert_eq!(state.agents.chat_input_cursor, 7);

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
            &mut state,
            &mut vitals,
            None
        ));
        assert_eq!(state.agents.chat_input_cursor, 10);

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Up, KeyModifiers::empty()),
            &mut state,
            &mut vitals,
            None
        ));
        assert_eq!(state.agents.chat_input_cursor, 7);
    }

    #[test]
    fn agent_chat_arrow_keys_move_cursor_not_thread_scroll() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.agents.chat_input = "one\ntwo".into();
        state.agents.chat_input_cursor = 0;
        state.agents.console_scroll = 3;
        let mut vitals = VitalsState::default();

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
            &mut state,
            &mut vitals,
            None
        ));
        assert_eq!(state.agents.chat_input_cursor, 4);
        assert_eq!(state.agents.console_scroll, 3);

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Up, KeyModifiers::empty()),
            &mut state,
            &mut vitals,
            None
        ));
        assert_eq!(state.agents.chat_input_cursor, 0);
        assert_eq!(state.agents.console_scroll, 3);
    }

    #[test]
    fn chat_paste_inserts_raw_text_without_sending_or_opening_command_prompt() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        let mut vitals = VitalsState::default();
        let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
        let mut fuzzy_runtime =
            FuzzySearchRuntime::new(&Theme::default(), state.settings.highlight.clone());
        let pasted = ":run now\n  keep this exactly\n@all is plain text";

        assert!(handle_paste_event(
            pasted,
            &mut state,
            &mut syntax,
            &mut fuzzy_runtime,
            &mut vitals
        ));
        assert_eq!(state.agents.chat_input, pasted);
        assert_eq!(state.agents.chat_input_cursor, pasted.chars().count());
        assert!(state.agents.messages.is_empty());
        assert!(state.command_line.is_none());
    }

    #[test]
    fn chat_paste_normalizes_crlf_markdown_for_chat_box() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        let mut vitals = VitalsState::default();
        let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
        let mut fuzzy_runtime =
            FuzzySearchRuntime::new(&Theme::default(), state.settings.highlight.clone());
        let pasted = "# Plan\r\n- item 1\r\n```rust\r\nlet x = 1;\r\n```\r\n";

        assert!(handle_paste_event(
            pasted,
            &mut state,
            &mut syntax,
            &mut fuzzy_runtime,
            &mut vitals
        ));
        assert_eq!(
            state.agents.chat_input,
            "# Plan\n- item 1\n```rust\nlet x = 1;\n```\n"
        );
        assert!(!state.agents.chat_input.contains('\r'));
        assert_eq!(
            state.agents.chat_input_cursor,
            state.agents.chat_input.chars().count()
        );
        assert!(state.agents.messages.is_empty());
        assert!(state.command_line.is_none());
    }

    #[test]
    fn agent_chat_send_preserves_pasted_formatting() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.agents.chat_input = "  code block:\n    let x = 1;\n".into();
        state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
        let mut vitals = VitalsState::default();

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
            &mut vitals,
            None
        ));
        assert_eq!(state.agents.messages.len(), 1);
        assert_eq!(
            state.agents.messages[0].text,
            "  code block:\n    let x = 1;\n"
        );
    }

    #[test]
    fn agent_chat_send_preserves_markdown_text() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        let markdown = "# Plan\n- item 1\n- item 2\n```rust\nlet x = 1;\n```\n";
        state.agents.chat_input = markdown.into();
        state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
        let mut vitals = VitalsState::default();

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
            &mut vitals,
            None
        ));
        assert_eq!(state.agents.messages.len(), 1);
        assert_eq!(state.agents.messages[0].text, markdown);
    }

    #[test]
    fn map_agent_console_mouse_maps_chat_thread_lines() {
        let mut state = state_for_test();
        state.agents.selected_mission = None;
        state.agents.selected_agent = None;
        // In normal usage the roster provides a selected agent context; include a lane so the
        // thread renders in single-agent mode (no repeating badge) and preserves content width.
        state.agents.agents.push(nit_core::AgentLane {
            id: "planner".into(),
            role: "Planner".into(),
            lane: "Lane A".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: nit_core::AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: "idle".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "10:00:00".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("planner".into()),
            mission_id: None,
            text: "hello world".into(),
        });
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 160,
            height: 48,
        };
        let layout = layout::split(screen);
        let text_area = agent_console_view::thread_text_area(layout.notes, &state)
            .expect("thread area should be available");
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: text_area.x,
            row: text_area.y,
            modifiers: KeyModifiers::NONE,
        };

        let (line_idx, col, lines) = map_agent_console_mouse(mouse, screen, &state, false)
            .expect("mouse should map into chat thread");
        assert_eq!(line_idx, 0);
        assert_eq!(col, 0);
        let flattened = lines.concat();
        assert!(flattened.contains("hello world"));
    }

    #[test]
    fn click_in_chat_input_box_moves_chat_cursor() {
        let mut state = state_for_test();
        state.agents.chat_input = "hello\nworld".into();
        state.agents.chat_input_cursor = 0;
        let mut input_state = InputState::new();
        let mut clipboard = None;
        let theme = Theme::default();
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 220,
            height: 42,
        };
        let layout = layout::split(screen);
        let input_area = agent_console_view::chat_input_text_area(layout.notes, &state)
            .expect("chat input area should be available");
        let down = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: input_area.x.saturating_add(2),
            row: input_area.y.saturating_add(1),
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse_down(
            down,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        assert_eq!(state.focus, PaneId::Notes);
        assert_eq!(state.agents.chat_input_cursor, 8);
    }

    #[test]
    fn click_in_chat_header_does_not_start_thread_selection() {
        let mut state = state_for_test();
        state.agents.selected_mission = None;
        state.agents.selected_agent = None;
        state.agents.messages.push(AgentMessage {
            at: "10:00:00".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("planner".into()),
            mission_id: None,
            text: "select me".into(),
        });
        let mut input_state = InputState::new();
        let mut clipboard = None;
        let theme = Theme::default();
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 140,
            height: 42,
        };
        let layout = layout::split(screen);
        let context_click = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: layout.notes.x.saturating_add(3),
            row: layout.notes.y.saturating_add(1),
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse_down(
            context_click,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        assert_eq!(state.focus, PaneId::Notes);
        assert!(state.ui_selection.is_none());
    }

    #[test]
    fn chat_thread_selection_starts_at_clicked_column() {
        let mut state = state_for_test();
        state.agents.selected_mission = None;
        state.agents.selected_agent = None;
        state.agents.console_scroll = 0;
        // Include the agent lane so the transcript renders in single-agent context and doesn't
        // waste horizontal space on redundant agent badges.
        state.agents.agents.push(nit_core::AgentLane {
            id: "planner".into(),
            role: "Planner".into(),
            lane: "Lane A".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: nit_core::AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: "idle".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "10:00:00".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("planner".into()),
            mission_id: None,
            text: "selection precision".into(),
        });
        let mut input_state = InputState::new();
        let mut clipboard = None;
        let theme = Theme::default();
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 140,
            height: 42,
        };
        let layout = layout::split(screen);
        let text_area = agent_console_view::thread_text_area(layout.notes, &state)
            .expect("thread area should be available");
        let lines =
            agent_console_view::thread_lines_for_selection(&state, text_area.width as usize);
        let line_idx = lines
            .iter()
            .position(|line| line.contains("precision"))
            .expect("precision line");
        let line = &lines[line_idx];
        let marker_byte = line.find("precision").expect("precision marker");
        let target_col = line[..marker_byte].chars().count();
        let click = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: text_area.x.saturating_add(target_col as u16),
            row: text_area.y.saturating_add(line_idx as u16),
            modifiers: KeyModifiers::NONE,
        };

        assert!(handle_mouse_down(
            click,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        let selection = state.ui_selection.expect("selection should exist");
        assert_eq!(selection.pane, UiSelectionPane::AgentConsole);
        assert_eq!(selection.start_line, line_idx);
        assert_eq!(selection.start_col, target_col);
        let selected_char = line
            .chars()
            .nth(selection.start_col)
            .expect("selected char at cursor");
        assert_eq!(selected_char, 'p');
    }

    #[test]
    fn mouse_wheel_in_chat_input_scrolls_input_not_thread() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.agents.chat_input = (0..40).map(|i| format!("line-{i}\n")).collect();
        state.agents.chat_input_cursor = 0;
        state.agents.chat_input_scroll = 0;
        state.agents.console_scroll = 6;
        let mut fuzzy_runtime =
            FuzzySearchRuntime::new(&Theme::default(), state.settings.highlight.clone());
        let mut input_state = InputState::new();
        let mut clipboard = None;
        let theme = Theme::default();
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 140,
            height: 42,
        };
        let layout = layout::split(screen);
        let input_area = agent_console_view::chat_input_text_area(layout.notes, &state)
            .expect("chat input area should be available");
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: input_area.x.saturating_add(1),
            row: input_area.y.saturating_add(1),
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse_event(
            wheel,
            screen,
            &mut state,
            &mut fuzzy_runtime,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        assert!(state.agents.chat_input_scroll > 0);
        assert_eq!(state.agents.console_scroll, 6);
    }

    #[test]
    fn mouse_wheel_in_chat_thread_clamps_at_bottom_without_wrap() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.agents.selected_mission = None;
        state.agents.selected_agent = None;
        state.agents.missions.clear();
        state.agents.console_scroll = 0;
        state.agents.messages.clear();
        for i in 0..120 {
            state.agents.messages.push(AgentMessage {
                at: format!("10:00:{:02}", i % 60),
                channel: AgentChannel::Agent,
                agent_id: Some("planner".into()),
                mission_id: None,
                text: format!("message-{i}"),
            });
        }

        let mut fuzzy_runtime =
            FuzzySearchRuntime::new(&Theme::default(), state.settings.highlight.clone());
        let mut input_state = InputState::new();
        let mut clipboard = None;
        let theme = Theme::default();
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 36,
        };
        let layout = layout::split(screen);
        let thread_area = agent_console_view::thread_text_area(layout.notes, &state)
            .expect("chat thread area should be available");
        let lines =
            agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);
        let max_scroll = lines.len().saturating_sub(thread_area.height as usize);
        let wheel_down = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: thread_area.x.saturating_add(1),
            row: thread_area.y.saturating_add(1),
            modifiers: KeyModifiers::NONE,
        };

        for _ in 0..(max_scroll + 50) {
            assert!(handle_mouse_event(
                wheel_down,
                screen,
                &mut state,
                &mut fuzzy_runtime,
                &mut input_state,
                &mut clipboard,
                &theme
            ));
        }
        assert_eq!(state.agents.console_scroll, max_scroll);

        assert!(handle_mouse_event(
            wheel_down,
            screen,
            &mut state,
            &mut fuzzy_runtime,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        assert_eq!(state.agents.console_scroll, max_scroll);
    }

    #[test]
    fn scrolled_chat_selection_maps_to_visible_line_not_top() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.agents.selected_mission = None;
        state.agents.selected_agent = None;
        state.agents.messages.clear();
        for i in 0..120 {
            state.agents.messages.push(AgentMessage {
                at: format!("10:00:{:02}", i % 60),
                channel: AgentChannel::Agent,
                agent_id: Some("planner".into()),
                mission_id: None,
                text: format!("payload-{i:03}"),
            });
        }
        let mut fuzzy_runtime =
            FuzzySearchRuntime::new(&Theme::default(), state.settings.highlight.clone());
        let mut input_state = InputState::new();
        let mut clipboard = None;
        let theme = Theme::default();
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 140,
            height: 42,
        };
        let layout = layout::split(screen);
        let thread_area = agent_console_view::thread_text_area(layout.notes, &state)
            .expect("chat thread area should be available");
        let wheel_down = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: thread_area.x.saturating_add(1),
            row: thread_area.y.saturating_add(1),
            modifiers: KeyModifiers::NONE,
        };
        for _ in 0..18 {
            assert!(handle_mouse_event(
                wheel_down,
                screen,
                &mut state,
                &mut fuzzy_runtime,
                &mut input_state,
                &mut clipboard,
                &theme
            ));
        }
        assert!(state.agents.console_scroll > 0);

        let lines =
            agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);
        let visible_height = thread_area.height as usize;
        let visible_start = state.agents.console_scroll;
        let maybe_target = (0..visible_height)
            .filter_map(|row| {
                let idx = visible_start.saturating_add(row);
                lines.get(idx).map(|line| (row, idx, line))
            })
            .find(|(_, _, line)| line.contains("payl"));
        let (row_rel, expected_line_idx, line) = if let Some(target) = maybe_target {
            target
        } else {
            let visible = (0..visible_height)
                .filter_map(|row| {
                    let idx = visible_start.saturating_add(row);
                    lines.get(idx).map(|line| line.clone())
                })
                .collect::<Vec<_>>();
            panic!("payl line visible after scroll; visible={visible:?}");
        };
        let marker_byte = line.find("payl").expect("marker in visible line");
        let marker_col = line[..marker_byte].chars().count();
        let select_col = marker_col + 1;
        let expected_char = line
            .chars()
            .nth(select_col)
            .expect("character at selection point")
            .to_string();

        let down = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: thread_area.x.saturating_add(select_col as u16),
            row: thread_area.y.saturating_add(row_rel as u16),
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse_down(
            down,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        let selection = state.ui_selection.expect("selection exists");
        assert_eq!(selection.start_line, expected_line_idx);
        assert_eq!(selection.start_col, select_col);

        let drag = MouseEvent {
            kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
            column: down.column.saturating_add(1),
            row: down.row,
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse_drag(
            drag,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        assert_eq!(state.yank.as_deref(), Some(expected_char.as_str()));
    }

    #[test]
    fn user_bubble_selection_can_span_multiple_messages() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.agents.selected_mission = None;
        state.agents.selected_agent = None;
        state.agents.missions.clear();
        state.agents.console_scroll = 0;
        state.agents.messages.clear();
        state.agents.messages.push(AgentMessage {
            at: "10:00:00".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: "alpha prompt".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "10:00:01".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: "omega prompt".into(),
        });
        let mut input_state = InputState::new();
        let mut clipboard = None;
        let theme = Theme::default();
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 180,
            height: 42,
        };
        let layout = layout::split(screen);
        let thread_area = agent_console_view::thread_text_area(layout.notes, &state)
            .expect("chat thread area should be available");
        let lines =
            agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);
        let (start_row, start_col) = lines
            .iter()
            .enumerate()
            .find_map(|(idx, line)| {
                line.find("alpha")
                    .map(|byte| (idx, line[..byte].chars().count()))
            })
            .expect("alpha row");
        let (end_row, end_col) = lines
            .iter()
            .enumerate()
            .find_map(|(idx, line)| {
                line.find("omega")
                    .map(|byte| (idx, line[..byte].chars().count() + "omega".chars().count()))
            })
            .expect("omega row");
        let down = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: thread_area.x.saturating_add(start_col as u16),
            row: thread_area.y.saturating_add(start_row as u16),
            modifiers: KeyModifiers::NONE,
        };
        let drag = MouseEvent {
            kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
            column: thread_area.x.saturating_add(end_col as u16),
            row: thread_area.y.saturating_add(end_row as u16),
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse_down(
            down,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        assert!(handle_mouse_drag(
            drag,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        let yank = state.yank.clone().unwrap_or_default();
        assert!(yank.contains("alpha"));
        assert!(yank.contains("omega"));
    }

    #[test]
    fn scrolled_user_bubble_selection_can_span_multiple_messages() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.agents.selected_mission = None;
        state.agents.selected_agent = None;
        state.agents.missions.clear();
        state.agents.console_scroll = 0;
        state.agents.messages.clear();
        for i in 0..60 {
            state.agents.messages.push(AgentMessage {
                at: format!("10:00:{:02}", i % 60),
                channel: AgentChannel::Agent,
                agent_id: None,
                mission_id: None,
                text: format!("user-prompt-{i:02}"),
            });
        }
        let mut fuzzy_runtime =
            FuzzySearchRuntime::new(&Theme::default(), state.settings.highlight.clone());
        let mut input_state = InputState::new();
        let mut clipboard = None;
        let theme = Theme::default();
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 180,
            height: 42,
        };
        let layout = layout::split(screen);
        let thread_area = agent_console_view::thread_text_area(layout.notes, &state)
            .expect("chat thread area should be available");
        let wheel_down = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: thread_area.x.saturating_add(1),
            row: thread_area.y.saturating_add(1),
            modifiers: KeyModifiers::NONE,
        };
        for _ in 0..12 {
            assert!(handle_mouse_event(
                wheel_down,
                screen,
                &mut state,
                &mut fuzzy_runtime,
                &mut input_state,
                &mut clipboard,
                &theme
            ));
        }
        let lines =
            agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);
        let visible_start = state.agents.console_scroll;
        let visible_end = visible_start.saturating_add(thread_area.height as usize);
        let visible = lines
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx >= visible_start && *idx < visible_end)
            .collect::<Vec<_>>();
        let (start_row_rel, start_col, _) = visible
            .iter()
            .find_map(|(idx, line)| {
                line.find("user-prompt-").map(|byte| {
                    (
                        idx.saturating_sub(visible_start),
                        line[..byte].chars().count(),
                        *idx,
                    )
                })
            })
            .expect("visible start prompt");
        let (end_row_rel, end_col, _) = visible
            .iter()
            .rev()
            .find_map(|(idx, line)| {
                line.find("user-prompt-").map(|byte| {
                    (
                        idx.saturating_sub(visible_start),
                        line[..byte].chars().count() + 8,
                        *idx,
                    )
                })
            })
            .expect("visible end prompt");
        let down = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: thread_area.x.saturating_add(start_col as u16),
            row: thread_area.y.saturating_add(start_row_rel as u16),
            modifiers: KeyModifiers::NONE,
        };
        let drag = MouseEvent {
            kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
            column: thread_area.x.saturating_add(end_col as u16),
            row: thread_area.y.saturating_add(end_row_rel as u16),
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse_down(
            down,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        assert!(handle_mouse_drag(
            drag,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        let yank = state.yank.clone().unwrap_or_default();
        assert!(yank.contains("user-prompt-"));
        let unique_hits = yank.matches("user-prompt-").count();
        assert!(
            unique_hits >= 2,
            "expected >= 2 prompts in yank, got {unique_hits} from {yank:?}"
        );
    }

    #[test]
    fn vertical_drag_across_user_bubbles_includes_end_message_text() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.agents.selected_mission = None;
        state.agents.selected_agent = None;
        state.agents.missions.clear();
        state.agents.console_scroll = 0;
        state.agents.messages.clear();
        state.agents.messages.push(AgentMessage {
            at: "10:00:00".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: "first message has a wider bubble".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "10:00:01".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: "short".into(),
        });

        let mut input_state = InputState::new();
        let mut clipboard = None;
        let theme = Theme::default();
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 180,
            height: 42,
        };
        let layout = layout::split(screen);
        let thread_area = agent_console_view::thread_text_area(layout.notes, &state)
            .expect("chat thread area should be available");
        let lines =
            agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);

        let (start_row, start_col) = lines
            .iter()
            .enumerate()
            .find_map(|(idx, line)| {
                line.find("first")
                    .map(|byte| (idx, line[..byte].chars().count() + 2))
            })
            .expect("first row");
        let end_row = lines
            .iter()
            .enumerate()
            .find_map(|(idx, line)| line.contains("short").then_some(idx))
            .expect("short row");
        let end_col = lines
            .get(end_row)
            .and_then(|line| {
                line.find("short")
                    .map(|byte| line[..byte].chars().count() + "short".chars().count())
            })
            .expect("short col");

        let down = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: thread_area.x.saturating_add(start_col as u16),
            row: thread_area.y.saturating_add(start_row as u16),
            modifiers: KeyModifiers::NONE,
        };
        let drag = MouseEvent {
            kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
            column: thread_area.x.saturating_add(end_col as u16),
            row: thread_area.y.saturating_add(end_row as u16),
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse_down(
            down,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        assert!(handle_mouse_drag(
            drag,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        let yank = state.yank.clone().unwrap_or_default();
        assert!(
            yank.contains("short"),
            "end message text missing from yank: {yank:?}"
        );
    }

    #[test]
    fn reverse_vertical_drag_across_user_bubbles_keeps_both_messages() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.agents.selected_mission = None;
        state.agents.selected_agent = None;
        state.agents.missions.clear();
        state.agents.console_scroll = 0;
        state.agents.messages.clear();
        state.agents.messages.push(AgentMessage {
            at: "10:00:00".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: "first message has a wider bubble".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "10:00:01".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: "short".into(),
        });

        let mut input_state = InputState::new();
        let mut clipboard = None;
        let theme = Theme::default();
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 180,
            height: 42,
        };
        let layout = layout::split(screen);
        let thread_area = agent_console_view::thread_text_area(layout.notes, &state)
            .expect("chat thread area should be available");
        let lines =
            agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);

        let (start_row, start_col) = lines
            .iter()
            .enumerate()
            .find_map(|(idx, line)| {
                line.find("short")
                    .map(|byte| (idx, line[..byte].chars().count() + "short".chars().count()))
            })
            .expect("short row");
        let end_row = lines
            .iter()
            .enumerate()
            .find_map(|(idx, line)| line.contains("first").then_some(idx))
            .expect("first row");
        let end_payload_start = user_bubble_payload_start_col(
            lines
                .get(end_row)
                .expect("line for first message payload should exist"),
        )
        .expect("payload start for first message");
        let end_col = end_payload_start.saturating_sub(1);

        let down = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: thread_area.x.saturating_add(start_col as u16),
            row: thread_area.y.saturating_add(start_row as u16),
            modifiers: KeyModifiers::NONE,
        };
        let drag = MouseEvent {
            kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
            column: thread_area.x.saturating_add(end_col as u16),
            row: thread_area.y.saturating_add(end_row as u16),
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse_down(
            down,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        assert!(handle_mouse_drag(
            drag,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        let yank = state.yank.clone().unwrap_or_default();
        assert!(
            yank.contains("first"),
            "first message missing from yank: {yank:?}"
        );
        assert!(
            yank.contains("short"),
            "second message missing from yank: {yank:?}"
        );
    }

    #[test]
    fn scrolled_reverse_drag_across_user_bubbles_keeps_visible_messages() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.agents.selected_mission = None;
        state.agents.selected_agent = None;
        state.agents.missions.clear();
        state.agents.console_scroll = 0;
        state.agents.messages.clear();
        for i in 0..80 {
            state.agents.messages.push(AgentMessage {
                at: format!("10:00:{:02}", i % 60),
                channel: AgentChannel::Agent,
                agent_id: None,
                mission_id: None,
                text: format!("user-prompt-{i:02}"),
            });
        }

        let mut fuzzy_runtime =
            FuzzySearchRuntime::new(&Theme::default(), state.settings.highlight.clone());
        let mut input_state = InputState::new();
        let mut clipboard = None;
        let theme = Theme::default();
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 180,
            height: 42,
        };
        let layout = layout::split(screen);
        let thread_area = agent_console_view::thread_text_area(layout.notes, &state)
            .expect("chat thread area should be available");
        let wheel_down = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: thread_area.x.saturating_add(1),
            row: thread_area.y.saturating_add(1),
            modifiers: KeyModifiers::NONE,
        };
        for _ in 0..16 {
            assert!(handle_mouse_event(
                wheel_down,
                screen,
                &mut state,
                &mut fuzzy_runtime,
                &mut input_state,
                &mut clipboard,
                &theme
            ));
        }

        let lines =
            agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);
        let visible_start = state.agents.console_scroll;
        let visible_end = visible_start.saturating_add(thread_area.height as usize);
        let visible_prompt_rows = lines
            .iter()
            .enumerate()
            .filter(|(idx, line)| {
                *idx >= visible_start && *idx < visible_end && line.contains("user-prompt-")
            })
            .collect::<Vec<_>>();
        assert!(
            visible_prompt_rows.len() >= 2,
            "need at least two visible prompt rows after scroll"
        );
        let (start_abs_row, start_line) =
            *visible_prompt_rows.last().expect("last visible prompt row");
        let (end_abs_row, end_line) = *visible_prompt_rows
            .first()
            .expect("first visible prompt row");
        let start_row_rel = start_abs_row.saturating_sub(visible_start);
        let end_row_rel = end_abs_row.saturating_sub(visible_start);
        let start_col = start_line
            .find("user-prompt-")
            .map(|byte| start_line[..byte].chars().count() + "user-prompt-".chars().count())
            .expect("start prompt marker");
        let end_payload_start =
            user_bubble_payload_start_col(end_line).expect("payload start in end prompt row");
        let end_col = end_payload_start.saturating_sub(1);

        let down = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: thread_area.x.saturating_add(start_col as u16),
            row: thread_area.y.saturating_add(start_row_rel as u16),
            modifiers: KeyModifiers::NONE,
        };
        let drag = MouseEvent {
            kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
            column: thread_area.x.saturating_add(end_col as u16),
            row: thread_area.y.saturating_add(end_row_rel as u16),
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse_down(
            down,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        assert!(handle_mouse_drag(
            drag,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        let yank = state.yank.clone().unwrap_or_default();
        let hits = yank.matches("user-prompt-").count();
        assert!(
            hits >= 2,
            "expected >=2 prompt hits in yank after scrolled reverse drag, got {hits}: {yank:?}"
        );
    }

    #[test]
    fn agent_console_mouse_drag_copies_selected_chat_text() {
        let mut state = state_for_test();
        state.agents.messages.push(AgentMessage {
            at: "10:00:00".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("planner".into()),
            mission_id: None,
            text: "selection copy works".into(),
        });
        let mut input_state = InputState::new();
        let mut clipboard = None;
        let theme = Theme::default();
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 160,
            height: 48,
        };
        let layout = layout::split(screen);
        let text_area = agent_console_view::thread_text_area(layout.notes, &state)
            .expect("thread area should be available");
        let down = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: text_area.x,
            // Agent messages include a top padding row, then ECG header, then message text.
            row: text_area.y.saturating_add(2),
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse_down(
            down,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        let drag = MouseEvent {
            kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
            column: text_area.x.saturating_add(24),
            row: text_area.y.saturating_add(2),
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse_drag(
            drag,
            screen,
            &mut state,
            &mut input_state,
            &mut clipboard,
            &theme
        ));
        assert_eq!(state.focus, PaneId::Notes);
        assert!(matches!(
            state.ui_selection.map(|s| s.pane),
            Some(UiSelectionPane::AgentConsole)
        ));
        assert!(state
            .yank
            .as_deref()
            .unwrap_or_default()
            .contains("selection copy works"));
    }

    #[test]
    fn esc_in_agent_chat_clears_thread_selection_before_chat_input() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.ui_selection = Some(nit_core::UiSelection {
            pane: UiSelectionPane::AgentConsole,
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 1,
        });
        state.agents.chat_input = "draft message".into();
        state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
        let mut vitals = VitalsState::default();

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut state,
            &mut vitals,
            None
        ));
        assert!(state.ui_selection.is_none());
        assert_eq!(state.agents.chat_input, "draft message");
    }

    #[test]
    fn esc_in_agent_chat_does_not_clear_chat_input_when_no_selection() {
        let mut state = state_for_test();
        state.focus = PaneId::Notes;
        state.ui_selection = None;
        state.agents.chat_input = "draft message".into();
        state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
        let mut vitals = VitalsState::default();

        assert!(!handle_agent_station_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut state,
            &mut vitals,
            None
        ));
        assert!(state.ui_selection.is_none());
        assert_eq!(state.agents.chat_input, "draft message");
    }

    #[test]
    fn agent_console_selection_strips_user_bubble_edges_from_clipboard() {
        let mut state = state_for_test();
        state.agents.selected_mission = None;
        state.agents.selected_agent = None;
        state.agents.console_scroll = 0;
        state.agents.messages.clear();
        state.agents.messages.push(AgentMessage {
            at: "10:00:00".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: "hello".into(),
        });
        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 36,
        };
        let layout = layout::split(screen);
        let thread_area =
            agent_console_view::thread_text_area(layout.notes, &state).expect("thread area");
        let lines =
            agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);
        let (line_idx, line) = lines
            .iter()
            .enumerate()
            .find(|(_, line)| line.contains("hello"))
            .expect("hello bubble line");
        let line_len = line.chars().count();
        let selection = UiSelection {
            pane: UiSelectionPane::AgentConsole,
            start_line: line_idx,
            start_col: 0,
            end_line: line_idx,
            end_col: line_len,
        };
        let text = selection_text_agent_console(&lines, selection);
        assert_eq!(text, "hello");
    }

    #[test]
    fn agent_console_selection_does_not_strip_markdown_table_pipes_in_agent_output() {
        let mut state = state_for_test();
        state.agents.selected_mission = None;
        state.agents.selected_agent = None;
        state.agents.console_scroll = 0;
        state.agents.messages.clear();
        state.agents.agents.push(nit_core::AgentLane {
            id: "coder".into(),
            role: "Coder".into(),
            lane: "Lane B".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: AgentStatus::Running,
            heartbeat_age_secs: 1,
            queue_len: 1,
            current_mission: None,
            last_message: "active".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "10:00:00".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("coder".into()),
            mission_id: None,
            text: "intro\n| table row |".into(),
        });

        let screen = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 140,
            height: 42,
        };
        let layout = layout::split(screen);
        let thread_area =
            agent_console_view::thread_text_area(layout.notes, &state).expect("thread area");
        let lines =
            agent_console_view::thread_lines_for_selection(&state, thread_area.width as usize);
        let (line_idx, line) = lines
            .iter()
            .enumerate()
            .find(|(_, line)| line.contains("| table row |"))
            .expect("table row line");
        let line_len = line.chars().count();
        let selection = UiSelection {
            pane: UiSelectionPane::AgentConsole,
            start_line: line_idx,
            start_col: 0,
            end_line: line_idx,
            end_col: line_len,
        };
        let text = selection_text_agent_console(&lines, selection);
        assert!(
            text.contains("| table row |"),
            "unexpected stripped text: {text:?}"
        );
        assert_eq!(text, line.as_str());
    }

    #[test]
    fn scratchpad_in_agent_ops_accepts_insert_input() {
        let mut state = state_for_test();
        state.focus = PaneId::JobOutput;
        state.agents.dock_tab = AgentOpsTab::Scratchpad;
        state.mode = Mode::Insert;
        let mut input = InputState::new();
        let mut vitals = VitalsState::default();

        // Scratchpad editing should flow through the normal action keymap.
        assert!(!handle_agent_station_key(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            &mut state,
            &mut vitals,
            None
        ));
        let action = map_key_to_action(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            &state,
            &mut input,
        );
        assert_eq!(action, Some(Action::InsertChar('x')));
        let _ = apply_action(&mut state, Action::InsertChar('x'));
        assert!(state.notes_buffer().content_as_string().contains('x'));
    }

    #[test]
    fn scratchpad_tab_cycles_ops_tabs_without_escaping_insert_mode() {
        let mut state = state_for_test();
        state.focus = PaneId::JobOutput;
        state.agents.dock_tab = AgentOpsTab::Scratchpad;
        state.mode = Mode::Insert;
        let mut vitals = VitalsState::default();

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()),
            &mut state,
            &mut vitals,
            None
        ));
        assert_eq!(state.agents.dock_tab, AgentOpsTab::Roster);
        assert_eq!(state.mode, Mode::Normal);
    }

    #[test]
    fn agent_ops_left_right_arrows_switch_tabs() {
        let mut state = state_for_test();
        state.focus = PaneId::JobOutput;
        state.agents.dock_tab = AgentOpsTab::Roster;
        let mut vitals = VitalsState::default();

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Right, KeyModifiers::empty()),
            &mut state,
            &mut vitals,
            None
        ));
        assert_eq!(state.agents.dock_tab, AgentOpsTab::Missions);

        assert!(handle_agent_station_key(
            KeyEvent::new(KeyCode::Left, KeyModifiers::empty()),
            &mut state,
            &mut vitals,
            None
        ));
        assert_eq!(state.agents.dock_tab, AgentOpsTab::Roster);
    }

    #[test]
    fn job_pause_key_matches_ctrl_space_and_f6() {
        let ctrl_space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL);
        assert!(is_job_pause_key(&ctrl_space));

        let f6 = KeyEvent::new(KeyCode::F(6), KeyModifiers::empty());
        assert!(is_job_pause_key(&f6));

        // Depending on terminal + crossterm backend, Ctrl+Space can also arrive as NULL.
        let nul_code = KeyEvent::new(KeyCode::Char('\u{0}'), KeyModifiers::empty());
        assert!(is_job_pause_key(&nul_code));

        let null_code = KeyEvent::new(KeyCode::Null, KeyModifiers::empty());
        assert!(is_job_pause_key(&null_code));
    }

    #[test]
    fn command_prompt_open_key_matches_colon_and_shift_semicolon() {
        let colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty());
        assert!(is_command_prompt_open_key(&colon));

        let semicolon_shift = KeyEvent::new(KeyCode::Char(';'), KeyModifiers::SHIFT);
        assert!(is_command_prompt_open_key(&semicolon_shift));
    }

    #[test]
    fn petri_show_key_matches_ctrl_caret_terminal_variants() {
        let mut state = state_for_test();
        state.app_kind = AppKind::Games;
        state.games.running = true;
        state.games.petri_hidden = true;

        let ctrl_six = KeyEvent::new(KeyCode::Char('6'), KeyModifiers::CONTROL);
        assert!(is_petri_show_key(&ctrl_six, &state));

        let ctrl_caret = KeyEvent::new(KeyCode::Char('^'), KeyModifiers::CONTROL);
        assert!(is_petri_show_key(&ctrl_caret, &state));

        let rs_control_char = KeyEvent::new(KeyCode::Char('\u{1e}'), KeyModifiers::empty());
        assert!(is_petri_show_key(&rs_control_char, &state));
    }

    #[test]
    fn petri_show_key_allows_done_games_session() {
        let mut state = state_for_test();
        state.app_kind = AppKind::Games;
        state.games.running = false;
        state.games.status = nit_core::GamesStatus::Done;
        state.games.petri_hidden = true;
        state.games.petri_lines = vec!["Status: Done".into()];

        let ctrl_six = KeyEvent::new(KeyCode::Char('6'), KeyModifiers::CONTROL);
        assert!(is_petri_show_key(&ctrl_six, &state));
        assert!(games_petri_active(&state));
        state.games.petri_hidden = false;
        assert!(games_petri_visible(&state));
    }

    #[test]
    fn games_history_open_key_matches_ctrl_star_variants() {
        let mut state = state_for_test();
        state.app_kind = AppKind::Games;
        state.games.running = true;

        let ctrl_star = KeyEvent::new(KeyCode::Char('*'), KeyModifiers::CONTROL);
        assert!(is_games_history_open_key(&ctrl_star, &state));

        let ctrl_shift_star = KeyEvent::new(
            KeyCode::Char('*'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert!(is_games_history_open_key(&ctrl_shift_star, &state));

        let ctrl_eight = KeyEvent::new(KeyCode::Char('8'), KeyModifiers::CONTROL);
        assert!(is_games_history_open_key(&ctrl_eight, &state));
    }

    #[test]
    fn file_tree_does_not_consume_command_or_help_toggle_keys() {
        let mut state = state_for_test();
        state.file_tree.open = true;
        let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
        let area = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };

        let colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty());
        assert!(!handle_file_tree_key(&colon, &mut state, &mut syntax, area));

        let help = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT);
        assert!(!handle_file_tree_key(&help, &mut state, &mut syntax, area));

        state.app_kind = AppKind::Games;
        state.games.running = true;
        state.games.petri_hidden = true;
        let show_hidden = KeyEvent::new(KeyCode::Char('\u{1e}'), KeyModifiers::empty());
        assert!(!handle_file_tree_key(
            &show_hidden,
            &mut state,
            &mut syntax,
            area
        ));

        let history = KeyEvent::new(KeyCode::Char('8'), KeyModifiers::CONTROL);
        assert!(!handle_file_tree_key(
            &history,
            &mut state,
            &mut syntax,
            area
        ));
    }

    #[test]
    fn background_work_active_when_games_running_or_loading() {
        let mut state = state_for_test();
        state.app_kind = AppKind::Games;
        assert!(!is_background_work_active(&state));

        state.games.running = true;
        assert!(is_background_work_active(&state));

        state.games.running = false;
        state.games.run_browser.loading = true;
        assert!(is_background_work_active(&state));
    }

    #[test]
    fn background_work_active_when_status_text_is_busy() {
        let mut state = state_for_test();
        state.app_kind = AppKind::Games;
        state.status = Some("Games analysis started".into());
        assert!(is_background_work_active(&state));

        state.status = Some("Games tournament completed".into());
        assert!(!is_background_work_active(&state));
    }

    #[test]
    fn log_line_vitals_do_not_refresh_job_heartbeat_without_real_activity() {
        let mut vitals = VitalsState::default();
        let start = Instant::now();
        vitals.record_job_event(start);

        let later = start + Duration::from_secs(3);
        record_log_line_vitals(&mut vitals, later, "INFO just a message");

        let age = vitals.job_hb.age(later).unwrap_or_default();
        assert!(age >= Duration::from_secs(3));
    }

    #[test]
    fn status_looks_busy_matches_expected_keywords() {
        assert!(status_looks_busy("Games analysis started"));
        assert!(status_looks_busy("Preparing run config..."));
        assert!(status_looks_busy("Loading replay..."));
        assert!(!status_looks_busy("Games tournament completed"));
        assert!(!status_looks_busy("Saved"));
    }

    #[test]
    fn vitals_smoke_games_busy_phase_keeps_ecg_alive_before_run() {
        let mut state = state_for_test();
        state.app_kind = AppKind::Games;
        state.games.pending_run = true;
        state.status = Some("Preparing run config...".into());

        let mut vitals = VitalsState::default();
        let mut now = Instant::now();
        let dt = Duration::from_millis(100);
        let mut last_busy_pulse = now;
        let mut max_sample = 0u64;
        let mut last_snapshot = vitals.tick(
            now,
            dt,
            is_lab_job_running(&state),
            current_agent_state(&state),
        );

        for _ in 0..40 {
            now += dt;
            if is_background_work_active(&state)
                && !is_lab_job_running(&state)
                && now.saturating_duration_since(last_busy_pulse) >= BUSY_PULSE_INTERVAL
            {
                vitals.record_job_event(now);
                last_busy_pulse = now;
            }
            last_snapshot = vitals.tick(
                now,
                dt,
                is_lab_job_running(&state),
                current_agent_state(&state),
            );
            max_sample = max_sample.max(*last_snapshot.ecg_samples.last().unwrap_or(&0));
        }

        assert!(
            max_sample >= 30,
            "expected busy pulses to animate ECG before run, got {max_sample}"
        );
        assert_eq!(
            last_snapshot.criticality,
            crate::vitals::LabCriticality::Idle
        );
    }

    #[test]
    fn vitals_smoke_games_run_then_stall_hits_crit_boundary() {
        let mut state = state_for_test();
        state.app_kind = AppKind::Games;
        state.games.running = true;
        state.games.paused = false;

        let mut vitals = VitalsState::default();
        let mut now = Instant::now();
        let dt = Duration::from_millis(100);

        let mut snapshot = vitals.tick(
            now,
            dt,
            is_lab_job_running(&state),
            current_agent_state(&state),
        );
        for _ in 0..30 {
            now += dt;
            vitals.record_job_event(now);
            snapshot = vitals.tick(
                now,
                dt,
                is_lab_job_running(&state),
                current_agent_state(&state),
            );
        }

        assert!(snapshot.hb_age.unwrap_or(Duration::MAX) < Duration::from_secs(1));
        assert_ne!(snapshot.criticality, crate::vitals::LabCriticality::Crit);

        for _ in 0..120 {
            now += dt;
            snapshot = vitals.tick(
                now,
                dt,
                is_lab_job_running(&state),
                current_agent_state(&state),
            );
        }

        assert!(snapshot.hb_age.unwrap_or_default() >= Duration::from_secs(10));
        assert_eq!(snapshot.criticality, crate::vitals::LabCriticality::Crit);
    }
}
