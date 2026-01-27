use std::io::{self, Stdout};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use arboard::Clipboard;
use crossterm::{
    cursor::SetCursorStyle,
    event::{
        self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use nit_core::{
    actions::Action, apply_action, io as core_io, AppState, Mode, PaneId, Prompt, Viewport,
    YankKind,
};
use ratatui::{
    backend::CrosstermBackend,
    style::Style,
    widgets::{Block, Borders, Clear, Paragraph},
    Terminal,
};
use tracing::info;

use crate::{
    layout,
    petri_dish::PetriDishRuntime,
    seed_runtime::SeedRuntime,
    system_stats::SystemStats,
    syntax::SyntaxRuntime,
    theme::Theme,
    widgets::{
        bottom_bar, editor_view, gate_monitor_view, help_overlay, job_output_view, notes_view,
        rule_picker, top_bar, visualizer_view,
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
    };
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
    syntax.prime_buffer(editor_id, state.editor_buffer(), true);
    syntax.prime_buffer(notes_id, state.notes_buffer(), false);

    let result = run_loop(&mut terminal, &mut state, &theme, &mut syntax, log_rx);

    terminal.show_cursor()?;
    if guard.keyboard_flags_pushed {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        guard.keyboard_flags_pushed = false;
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
    let mut seed_runtime = SeedRuntime::new(state);
    let mut petri = PetriDishRuntime::new(state);
    tracing::info!("SECURITY: no plugins, no network, no shell execution");
    loop {
        if let Some(deferred) = input_state.take_deferred() {
            if let Some(action) = map_key_to_action(deferred, state, &mut input_state) {
                prepare_clipboard_paste(state, &mut clipboard, &action);
                let action_copy = action.clone();
                let outcome = apply_action_with_syntax(state, syntax, action);
                handle_clipboard_copy(state, &mut clipboard, &action_copy);
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
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Release {
                    continue;
                }
                handled_input = true;
                if state.rule_picker.open {
                    if rule_picker::handle_key(&key, state) {
                        needs_redraw = true;
                        continue;
                    }
                }
                if petri.is_visible() && state.command_line.is_none() && state.prompt.is_none() {
                    let screen = terminal.size().unwrap_or_default();
                    if petri.handle_key(&key, state, &mut seed_runtime, screen) {
                        needs_redraw = true;
                        continue;
                    }
                }
                if let Some(action) = map_key_to_action(key, state, &mut input_state) {
                    prepare_clipboard_paste(state, &mut clipboard, &action);
                    let action_copy = action.clone();
                    let outcome = apply_action_with_syntax(state, syntax, action);
                    handle_clipboard_copy(state, &mut clipboard, &action_copy);
                    if outcome.should_exit {
                        break;
                    }
                    needs_redraw = needs_redraw || outcome.state_changed;
                }
            }
        }

        if !handled_input && matches!(state.focus, PaneId::Editor) && state.mode == Mode::Insert {
            if let Some(action) = input_state.flush_insert_timeout() {
                prepare_clipboard_paste(state, &mut clipboard, &action);
                let action_copy = action.clone();
                let outcome = apply_action_with_syntax(state, syntax, action);
                handle_clipboard_copy(state, &mut clipboard, &action_copy);
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

        // syntax ticks
        let editor_id = state.active_editor_buffer_id;
        let notes_id = state.notes_buffer_id;
        syntax.tick(editor_id, state.editor_buffer());
        syntax.tick(notes_id, state.notes_buffer());
        syntax.poll_results(editor_id, state.editor_buffer().version());
        syntax.poll_results(notes_id, state.notes_buffer().version());

        let tick_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            seed_runtime.tick(state);
            petri.tick(state);
        }));
        if let Err(err) = tick_result {
            tracing::error!("Runtime panic: {:?}", err);
            state.visualizer.paused = true;
            state.visualizer.paused_by_attractor = false;
            state.status = Some("Petri dish paused (error)".into());
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
                &mut petri,
            )?;
            needs_redraw = false;
            last_tick = Instant::now();
        }
    }
    Ok(())
}

fn draw(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut AppState,
    theme: &Theme,
    syntax: &mut SyntaxRuntime,
    system_stats: &SystemStats,
    seed_runtime: &mut SeedRuntime,
    petri: &mut PetriDishRuntime,
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
        state.set_viewport(
            PaneId::Editor,
            Viewport::with_dims(
                layout.editor.height.saturating_sub(2) as usize,
                editor_text_width as usize,
            ),
        );
        let notes_total = state.notes_buffer().lines_len().max(1);
        let notes_line_width = notes_total.to_string().len().max(3) as u16;
        let notes_gutter = notes_line_width + 4;
        let notes_text_width = layout
            .notes
            .width
            .saturating_sub(2)
            .saturating_sub(notes_gutter);
        state.set_viewport(
            PaneId::Notes,
            Viewport::with_dims(
                layout.notes.height.saturating_sub(2) as usize,
                notes_text_width as usize,
            ),
        );
        state.editor_buffer_mut().ensure_visible();
        state.notes_buffer_mut().ensure_visible();

        let editor_id = state.active_editor_buffer_id;
        let notes_id = state.notes_buffer_id;
        top_bar::render(f, layout.top, state, theme);
        let editor_cursor = {
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
        let viz_inner_width = layout.visualizer.width.saturating_sub(2) as usize;
        let viz_inner_height = layout.visualizer.height.saturating_sub(2) as usize;
        let viz_grid_rows = viz_inner_height.saturating_sub(1);
        let (grid_w, grid_h) = crate::seed_render::grid_size_for_mode(
            viz_inner_width,
            viz_grid_rows,
            state.visualizer.seed_plate_mode,
        );
        seed_runtime.ensure_size(grid_w, grid_h, state);
        visualizer_view::render(f, layout.visualizer, state, theme, seed_runtime);
        let syntax_status = syntax.status_label_for(editor_id, state.editor_buffer().version());
        let syntax_debug = {
            let latest = syntax.latest_snapshot_for(editor_id);
            gate_monitor_view::SyntaxDebugInfo {
                buffer_version: state.editor_buffer().version(),
                snapshot_version: latest.map(|s| s.version),
                engine_state: syntax.engine_state_label(editor_id),
                last_job_ms: latest.map(|s| s.duration_ms),
            }
        };
        gate_monitor_view::render(
            f,
            layout.gate,
            state,
            theme,
            &syntax_status,
            Some(syntax_debug),
        );
        bottom_bar::render(f, layout.bottom, state, theme, system_stats);

        petri.handle_pending_requests(state, seed_runtime, f.size());
        petri.render(f, f.size(), state, theme);
        if state.rule_picker.open {
            rule_picker::render(f, f.size(), state, theme);
        }
        if state.show_help {
            let area = dynamic_popup_rect(
                f.size(),
                help_overlay::preferred_size(f.size()),
            );
            help_overlay::render(f, area, theme);
        }
        if let Some(Prompt::ConfirmQuit) = state.prompt {
            let message = "Quit without saving? (Y/N)";
            let area = dynamic_popup_rect(
                f.size(),
                prompt_size(message),
            );
            render_prompt(f, area, theme, message);
        }
        if state.command_line.is_some() {
            let cmd = state
                .command_line
                .as_ref()
                .map(|c| c.input.as_str())
                .unwrap_or("");
            let message = format!(":{cmd}");
            let area = dynamic_popup_rect(f.size(), prompt_size(&message));
            render_command_prompt(f, area, theme, &message);
        }

        // cursor
        if petri.is_visible() || state.command_line.is_some() {
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
            KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
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
        return Some(Action::PetriShow);
    }

    if is_global_run_key(&key) {
        return Some(Action::VisualizerRun);
    }

    if let Some(action) = visualizer_ctrl_action(&key, state) {
        return Some(action);
    }

    if state.focus == PaneId::Visualizer {
        if let Some(action) = visualizer_inspector_action(&key, state, input) {
            return Some(action);
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
        } if is_motion_mode(state) => {
            Some(Action::MoveLeft)
        }
        KeyEvent {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => {
            Some(Action::MoveDown)
        }
        KeyEvent {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => {
            Some(Action::MoveUp)
        }
        KeyEvent {
            code: KeyCode::Char('l'),
            modifiers: KeyModifiers::NONE,
            ..
        } if is_motion_mode(state) => {
            Some(Action::MoveRight)
        }
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
}

impl InputState {
    fn new() -> Self {
        Self {
            normal_last_char: None,
            normal_last_time: Instant::now(),
            pending_insert: None,
            deferred_key: None,
            visualizer_jump: None,
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

fn is_petri_show_key(key: &KeyEvent, state: &AppState) -> bool {
    if !state.visualizer.petri_hidden || !state.visualizer.running {
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

fn dynamic_popup_rect(
    screen: ratatui::layout::Rect,
    desired: (u16, u16),
) -> ratatui::layout::Rect {
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

fn prompt_size(message: &str) -> (u16, u16) {
    let width = message.chars().count().max(12) as u16 + 4;
    let height = 3;
    (width, height)
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
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            if self.keyboard_flags_pushed {
                let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
            }
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), SetCursorStyle::DefaultUserShape);
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
}
