use std::io::{self, Stdout};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use arboard::Clipboard;
use crossterm::{
    cursor::SetCursorStyle,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use nit_core::{actions::Action, apply_action, AppState, Mode, PaneId, Prompt, Viewport, YankKind};
use ratatui::{
    backend::CrosstermBackend,
    style::Style,
    widgets::{Block, Borders, Clear, Paragraph},
    Terminal,
};
use tracing::info;

use crate::{
    layout,
    system_stats::SystemStats,
    syntax::SyntaxRuntime,
    theme::Theme,
    widgets::{
        bottom_bar, editor_view, gate_monitor_view, help_overlay, job_output_view, notes_view,
        top_bar, visualizer_view,
    },
};

const TICK_RATE: Duration = Duration::from_millis(50);
const JOB_TICK: Duration = Duration::from_millis(120);
const LOG_TICK: Duration = Duration::from_millis(900);
const CHORD_TIMEOUT: Duration = Duration::from_millis(300);

pub fn run(mut state: AppState, theme: Theme, log_rx: Receiver<String>) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut guard = TerminalGuard { active: true };
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let mut syntax = SyntaxRuntime::new(state.settings.highlight.clone());
    let editor_id = state.active_editor_buffer_id;
    let notes_id = state.notes_buffer_id;
    syntax.prime_buffer(editor_id, state.editor_buffer());
    syntax.prime_buffer(notes_id, state.notes_buffer());

    let result = run_loop(&mut terminal, &mut state, &theme, &mut syntax, log_rx);

    terminal.show_cursor()?;
    execute!(io::stdout(), SetCursorStyle::DefaultUserShape)?;
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    guard.active = false;
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
                handled_input = true;
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

        // redraw
        if needs_redraw || last_tick.elapsed() >= TICK_RATE {
            system_stats.refresh_if_needed();
            draw(terminal, state, theme, syntax, &system_stats)?;
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
    syntax: &SyntaxRuntime,
    system_stats: &SystemStats,
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
        let editor_snapshot = syntax.snapshot_for(editor_id, state.editor_buffer().version());
        let notes_snapshot = syntax.snapshot_for(notes_id, state.notes_buffer().version());

        top_bar::render(f, layout.top, state, theme);
        let editor_cursor = editor_view::render_editor(
            f,
            layout.editor,
            state.editor_buffer(),
            editor_snapshot,
            state.focus,
            state.mode,
            theme,
            state.settings.editor.tab_width as usize,
        );
        let notes_cursor = notes_view::render_notes(
            f,
            layout.notes,
            state.notes_buffer(),
            notes_snapshot,
            state.focus,
            state.mode,
            theme,
            state.settings.editor.tab_width as usize,
        );
        job_output_view::render(f, layout.job, state, theme);
        visualizer_view::render(f, layout.visualizer, state, theme);
        gate_monitor_view::render(
            f,
            layout.gate,
            state,
            theme,
            &syntax.status_label(editor_id),
        );
        bottom_bar::render(f, layout.bottom, state, theme, system_stats);

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

        // cursor
        if let Some(pos) = if state.focus == PaneId::Editor {
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
    // Prompt confirm takes precedence
    if let Some(Prompt::ConfirmQuit) = state.prompt {
        return match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Some(Action::ConfirmQuitYes),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some(Action::ConfirmQuitNo),
            _ => None,
        };
    }

    if state.focus == PaneId::JobOutput && is_clear_logs_key(&key) {
        return Some(Action::ClearLogs);
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
            code: KeyCode::Char('s') | KeyCode::Char('S'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
            && modifiers.contains(KeyModifiers::SHIFT) =>
        {
            Some(Action::ToggleSyntax)
        }
        KeyEvent {
            code: KeyCode::Char('g'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(if state.show_help {
            Action::HideHelp
        } else {
            Action::ShowHelp
        }),
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
            code: KeyCode::Char('r'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::VisualizerReseed),
        KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::VisualizerApply),
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
    let editor_id = state.active_editor_buffer_id;
    let notes_id = state.notes_buffer_id;
    let editor_version = state.editor_buffer().version();
    let notes_version = state.notes_buffer().version();
    let outcome = apply_action(state, action.clone());

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
        syntax.prime_buffer(editor_id, state.editor_buffer());
        syntax.prime_buffer(notes_id, state.notes_buffer());
    }

    outcome
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
}

impl InputState {
    fn new() -> Self {
        Self {
            normal_last_char: None,
            normal_last_time: Instant::now(),
            pending_insert: None,
            deferred_key: None,
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

fn is_clear_logs_key(key: &KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('n') | KeyCode::Char('N'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
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

struct TerminalGuard {
    active: bool,
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), SetCursorStyle::DefaultUserShape);
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
}
