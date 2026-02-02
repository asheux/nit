use std::io::{self, Stdout};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use arboard::Clipboard;
use crossterm::{
    cursor::SetCursorStyle,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, MouseEvent, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use nit_core::{
    actions::Action, apply_action, io as core_io, AppKind, AppState, Mode, PaneId, Prompt,
    UiSelection, UiSelectionPane, YankKind,
};
use nit_games::config::GamesConfig;
use ratatui::{
    backend::CrosstermBackend,
    style::Style,
    widgets::{Block, Borders, Clear, Paragraph},
    Terminal,
};
use tracing::info;

use crate::{
    file_tree,
    file_tree_runner::{FileTreeCommand, FileTreeEvent, FileTreeRunner},
    games_petri_dish::GamesPetriDishRuntime,
    layout,
    petri_dish::PetriDishRuntime,
    seed_runtime::SeedRuntime,
    syntax::SyntaxRuntime,
    system_stats::SystemStats,
    theme::Theme,
    widgets::{
        bottom_bar, editor_view, file_tree_view, games_analysis_popup, games_replay_popup,
        games_run_browser_popup, games_strategy_popup, games_tm_sim_popup, games_visualizer_view,
        gate_monitor_view, help_overlay, job_output_view, notes_view, protocol_picker, rule_picker,
        top_bar, visualizer_view,
    },
};

const TICK_RATE: Duration = Duration::from_millis(50);
const JOB_TICK: Duration = Duration::from_millis(120);
const LOG_TICK: Duration = Duration::from_millis(900);
const CHORD_TIMEOUT: Duration = Duration::from_millis(300);
const INSPECTOR_JUMP_TIMEOUT: Duration = Duration::from_millis(1500);

pub fn run(mut state: AppState, theme: Theme, log_rx: Receiver<String>) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut guard = TerminalGuard {
        active: true,
        keyboard_flags_pushed: false,
        mouse_capture: false,
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
    let mut last_log = Instant::now();
    let mut needs_redraw = true;
    let mut input_state = InputState::new();
    let mut system_stats = SystemStats::new();
    let mut clipboard = Clipboard::new().ok();
    let mut file_tree_runner = FileTreeRunner::spawn();
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
    tracing::info!("SECURITY: no plugins, no network, no shell execution");
    loop {
        if let Some(deferred) = input_state.take_deferred() {
            if let Some(action) = map_key_to_action(deferred, state, &mut input_state) {
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
                    if state.command_line.is_some() || state.prompt.is_some() {
                        if let Some(action) = map_key_to_action(key, state, &mut input_state) {
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
                        continue;
                    }
                    if matches!(key.code, KeyCode::Char(':')) {
                        if let Some(action) = map_key_to_action(key, state, &mut input_state) {
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
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.app_kind == AppKind::Games && state.games.replay.open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_replay_popup_key(&key, state, screen) {
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.app_kind == AppKind::Games && state.games.strategy_inspect.open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_strategy_popup_key(&key, state, screen) {
                            needs_redraw = true;
                            continue;
                        }
                    }
                    if state.app_kind == AppKind::Games && state.games.tm_sim.open {
                        let screen = terminal.size().unwrap_or_default();
                        if handle_tm_sim_popup_key(&key, state, screen) {
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
                    if let Some(action) = map_key_to_action(key, state, &mut input_state) {
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
                Event::Mouse(mouse) => {
                    handled_input = true;
                    let screen = terminal.size().unwrap_or_default();
                    if handle_mouse_event(
                        mouse,
                        screen,
                        state,
                        &mut input_state,
                        &mut clipboard,
                        theme,
                    ) {
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
            if !state.job.paused {
                info!("job {:.0}% complete", state.job.progress * 100.0);
            }
            last_job = Instant::now();
            needs_redraw = true;
        }

        // periodic log injection (in addition to tracing)
        if last_log.elapsed() >= LOG_TICK {
            info!("heartbeat frame={}", state.metrics.frame_count);
            last_log = Instant::now();
        }

        // drain logs
        while let Ok(line) = log_rx.try_recv() {
            state.receive_log(line);
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
                    state.status = Some(format!("NITTree: {message}"));
                    file_tree::rebuild_view(state, Some(preserve));
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

        // redraw
        if needs_redraw || last_tick.elapsed() >= TICK_RATE {
            system_stats.refresh_if_needed();
            draw(
                terminal,
                state,
                theme,
                syntax,
                &system_stats,
                &mut seed_runtime,
                &mut gol_petri,
                &mut games_petri,
            )?;
            needs_redraw = false;
            last_tick = Instant::now();
        }
    }
    file_tree_runner.shutdown();
    Ok(())
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
) -> io::Result<()> {
    let start = Instant::now();
    terminal.draw(|f| {
        let layout = layout::split(f.size());

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
        let notes_total = state.notes_buffer().lines_len().max(1);
        let notes_line_width = notes_total.to_string().len().max(3) as u16;
        let notes_gutter = notes_line_width + 4;
        let notes_text_width = layout
            .notes
            .width
            .saturating_sub(2)
            .saturating_sub(notes_gutter);
        let notes_height = layout.notes.height.saturating_sub(2) as usize;
        let notes_width = notes_text_width as usize;
        {
            let buf = state.notes_buffer_mut();
            let resized = buf.viewport.height != notes_height || buf.viewport.width != notes_width;
            buf.set_viewport_size(notes_height, notes_width);
            if resized {
                buf.ensure_visible();
            }
        }

        let editor_id = state.active_editor_buffer_id;
        let notes_id = state.notes_buffer_id;
        top_bar::render(f, layout.top, state, theme);
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
        let notes_cursor = {
            let notes_render = syntax.render_snapshot_for(notes_id, state.notes_buffer());
            notes_view::render_notes(
                f,
                layout.notes,
                state.notes_buffer(),
                notes_render.snapshot,
                notes_render.line_map,
                state.focus,
                state.mode,
                theme,
                state.settings.editor.tab_width as usize,
            )
        };
        job_output_view::render(f, layout.job, state, theme);
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
                games_visualizer_view::render(f, layout.visualizer, state, theme);
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
                    petri.handle_pending_requests(state, seed_runtime, f.size());
                    petri.render(f, f.size(), state, theme);
                }
            }
            AppKind::Games => {
                if let Some(petri) = games_petri.as_mut() {
                    petri.handle_pending_requests(state);
                    petri.render(f, f.size(), state, theme);
                }
            }
        }
        if state.app_kind == AppKind::Games && state.games.analysis.open {
            let area = dynamic_popup_rect(f.size(), games_analysis_popup::preferred_size(f.size()));
            games_analysis_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.run_browser.open {
            let area =
                dynamic_popup_rect(f.size(), games_run_browser_popup::preferred_size(f.size()));
            games_run_browser_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.replay.open {
            let area = dynamic_popup_rect(f.size(), games_replay_popup::preferred_size(f.size()));
            games_replay_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.strategy_inspect.open {
            let area = dynamic_popup_rect(f.size(), games_strategy_popup::preferred_size(f.size()));
            games_strategy_popup::render(f, area, state, theme);
        }
        if state.app_kind == AppKind::Games && state.games.tm_sim.open {
            let area = dynamic_popup_rect(f.size(), games_tm_sim_popup::preferred_size(f.size()));
            games_tm_sim_popup::render(f, area, state, theme);
        }
        if state.rule_picker.open {
            rule_picker::render(f, f.size(), state, theme);
        }
        if state.show_help {
            let area = dynamic_popup_rect(f.size(), help_overlay::preferred_size(f.size()));
            help_overlay::render(f, area, state, theme);
        }
        if let Some(Prompt::ConfirmQuit) = state.prompt {
            let message = "Quit without saving? (Y/N)";
            let area = dynamic_popup_rect(f.size(), prompt_size(message));
            render_prompt(f, area, theme, message);
        }
        let mut command_cursor = None;
        if let Some(cmd) = state.command_line.as_ref() {
            let message = format!(":{}", cmd.input);
            let area = dynamic_popup_rect(f.size(), prompt_size(&message));
            render_command_prompt(f, area, theme, &message);
            command_cursor = command_prompt_cursor(area, &cmd.input, cmd.cursor);
        }

        // cursor
        let petri_visible = match state.app_kind {
            AppKind::Gol => gol_petri.as_ref().map(|p| p.is_visible()).unwrap_or(false),
            AppKind::Games => games_petri
                .as_ref()
                .map(|p| p.is_visible())
                .unwrap_or(false),
        };
        if let Some((x, y)) = command_cursor {
            f.set_cursor(x, y);
        } else if petri_visible || state.command_line.is_some() {
            f.set_cursor(f.size().x, f.size().y);
        } else if state.file_tree.open {
            f.set_cursor(f.size().x, f.size().y);
        } else if let Some(pos) = if state.focus == PaneId::Editor {
            editor_cursor
        } else {
            notes_cursor
        } {
            f.set_cursor(pos.x, pos.y);
        } else {
            f.set_cursor(f.size().x, f.size().y);
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

    if state.focus == PaneId::JobOutput && is_clear_logs_key(&key) {
        return Some(Action::ClearLogs);
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

    match key {
        KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            ..
        }
        | KeyEvent {
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
            code: KeyCode::F(1),
            ..
        } => Some(if state.show_help {
            Action::HideHelp
        } else {
            Action::ShowHelp
        }),
        KeyEvent {
            code: KeyCode::Char('?'),
            modifiers,
            ..
        } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
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
            if matches!(state.focus, PaneId::Editor | PaneId::Notes) && state.mode == Mode::Insert {
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
            code: KeyCode::Char(':'),
            ..
        } => Some(Action::CommandPromptOpen),
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
            && matches!(state.focus, PaneId::Editor | PaneId::Notes)
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
        PaneId::Notes => (PaneId::Notes, state.notes_buffer()),
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
    matches!(state.focus, PaneId::Editor | PaneId::Notes) && state.mode == Mode::Normal
}

fn is_visual_mode(state: &AppState) -> bool {
    matches!(state.focus, PaneId::Editor | PaneId::Notes) && state.mode == Mode::Visual
}

fn is_motion_mode(state: &AppState) -> bool {
    matches!(state.focus, PaneId::Editor | PaneId::Notes)
        && matches!(state.mode, Mode::Normal | Mode::Visual)
}

fn is_insert_editing(state: &AppState) -> bool {
    matches!(state.focus, PaneId::Editor | PaneId::Notes) && state.mode == Mode::Insert
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
            code: KeyCode::Char('c') | KeyCode::Char('q'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    )
}

fn is_petri_show_key(key: &KeyEvent, state: &AppState) -> bool {
    let (hidden, running) = match state.app_kind {
        AppKind::Gol => (state.visualizer.petri_hidden, state.visualizer.running),
        AppKind::Games => (state.games.petri_hidden, state.games.running),
    };
    if !hidden || !running {
        return false;
    }
    match key {
        KeyEvent {
            code: KeyCode::Char('^') | KeyCode::Char('6'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => true,
        _ => false,
    }
}

fn games_petri_visible(state: &AppState) -> bool {
    state.app_kind == AppKind::Games && state.games.running && !state.games.petri_hidden
}

fn is_clear_logs_key(key: &KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('l') | KeyCode::Char('L'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
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
            code: KeyCode::Char('\u{0}'),
            modifiers,
            ..
        } if modifiers.is_empty()
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

fn handle_mouse_event(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
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

            if games_petri_visible(state) {
                return true;
            }

            let layout = layout::split(screen);
            if point_in_rect(mouse.column, mouse.row, layout.editor) {
                scroll_buffer(state.editor_buffer_mut(), delta);
                return true;
            }
            if point_in_rect(mouse.column, mouse.row, layout.notes) {
                scroll_buffer(state.notes_buffer_mut(), delta);
                return true;
            }
            if point_in_rect(mouse.column, mouse.row, layout.job) {
                let height = layout.job.height.saturating_sub(3) as usize;
                let max_scroll = state.logs.len().saturating_sub(height);
                let mut scroll = state.logs_scroll;
                bump_scroll(&mut scroll, delta);
                state.logs_scroll = scroll.min(max_scroll);
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

fn map_job_output_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    let layout = layout::split(screen);
    let text_area = job_output_text_area(layout.job);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines: Vec<String> = state.logs.iter().cloned().collect();
    if lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let total = lines.len();
    let max_scroll = total.saturating_sub(height);
    let scroll = state.logs_scroll.min(max_scroll);
    let start = total.saturating_sub(height + scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &lines,
        start,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, lines))
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
    let config_text = state.editor_buffer().content_as_string();
    let config_result = GamesConfig::from_toml_with_root(&config_text, Some(&state.workspace_root));
    let layout_info = games_visualizer_view::layout_for_config(inner, config_result.as_ref().ok());
    let area = layout_info.main;
    if !point_in_rect(mouse.column, mouse.row, area) && !clamp {
        return None;
    }
    let lines = games_visualizer_view::build_main_lines(
        state,
        theme,
        &config_result,
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
    let config_text = state.editor_buffer().content_as_string();
    let config_result = GamesConfig::from_toml_with_root(&config_text, Some(&state.workspace_root));
    let layout_info = games_visualizer_view::layout_for_config(inner, config_result.as_ref().ok());
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
    let text = selection_text(lines, selection);
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
    if point_in_rect(mouse.column, mouse.row, layout.notes) {
        set_buffer_cursor_from_mouse(
            state,
            PaneId::Notes,
            mouse,
            layout.notes,
            state.settings.editor.tab_width as usize,
            false,
        );
        if state.mode == Mode::Visual {
            state.mode = Mode::Normal;
        }
        state.notes_buffer_mut().clear_selection();
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::Buffer(PaneId::Notes),
            line: state.notes_buffer().cursor.line,
            col: state.notes_buffer().cursor.col,
        });
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
    if let Some((line_idx, col, lines)) = map_job_output_mouse(mouse, screen, state, false) {
        state.focus = PaneId::JobOutput;
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
                PaneId::Notes => (layout.notes, state.settings.editor.tab_width as usize),
                _ => return false,
            };
            state.focus = pane;
            let buffer = match pane {
                PaneId::Editor => state.editor_buffer_mut(),
                PaneId::Notes => state.notes_buffer_mut(),
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
                    .map(|(line_idx, col, lines)| (line_idx, col, lines)),
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
            };
            let Some((line_idx, col, lines)) = result else {
                return false;
            };
            state.ui_selection = Some(UiSelection {
                pane,
                start_line: anchor.line,
                start_col: anchor.col,
                end_line: line_idx,
                end_col: col,
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
    if ctrl_nav_dir(key).is_some() {
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
    if matches!(key.code, KeyCode::Char(':')) {
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

struct TerminalGuard {
    active: bool,
    keyboard_flags_pushed: bool,
    mouse_capture: bool,
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
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), SetCursorStyle::DefaultUserShape);
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
}
