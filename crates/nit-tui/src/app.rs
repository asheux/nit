use std::io::{self, Stdout};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use nit_core::{actions::Action, apply_action, AppState, Mode, PaneId, Prompt, Viewport};
use ratatui::{
    backend::CrosstermBackend,
    style::Style,
    widgets::{Block, Borders, Clear, Paragraph},
    Terminal,
};
use tracing::info;

use crate::{
    layout,
    theme::Theme,
    widgets::{
        bottom_bar, editor_view, gate_monitor_view, help_overlay, job_output_view, notes_view,
        top_bar, visualizer_view,
    },
};

const TICK_RATE: Duration = Duration::from_millis(50);
const JOB_TICK: Duration = Duration::from_millis(120);
const LOG_TICK: Duration = Duration::from_millis(900);

pub fn run(mut state: AppState, theme: Theme, log_rx: Receiver<String>) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut guard = TerminalGuard { active: true };
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let result = run_loop(&mut terminal, &mut state, &theme, log_rx);

    terminal.show_cursor()?;
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    guard.active = false;
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut AppState,
    theme: &Theme,
    log_rx: Receiver<String>,
) -> io::Result<()> {
    let mut last_tick = Instant::now();
    let mut last_job = Instant::now();
    let mut last_log = Instant::now();
    let mut needs_redraw = true;
    loop {
        // Poll input with tick fallback
        let timeout = TICK_RATE;
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if let Some(action) = map_key_to_action(key, state) {
                    let outcome = apply_action(state, action);
                    if outcome.should_exit {
                        break;
                    }
                    needs_redraw = needs_redraw || outcome.state_changed;
                }
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

        // redraw
        if needs_redraw || last_tick.elapsed() >= TICK_RATE {
            draw(terminal, state, theme)?;
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
) -> io::Result<()> {
    let start = Instant::now();
    terminal.draw(|f| {
        let layout = layout::split(f.size());

        // Update viewports
        state.set_viewport(
            PaneId::Editor,
            Viewport::with_dims(
                layout.editor.height.saturating_sub(2) as usize,
                layout.editor.width.saturating_sub(2) as usize,
            ),
        );
        state.set_viewport(
            PaneId::Notes,
            Viewport::with_dims(
                layout.notes.height.saturating_sub(2) as usize,
                layout.notes.width.saturating_sub(2) as usize,
            ),
        );

        top_bar::render(f, layout.top, state, theme);
        let editor_cursor = editor_view::render_editor(
            f,
            layout.editor,
            state.editor_buffer(),
            state.focus,
            state.mode,
            theme,
        );
        let notes_cursor = notes_view::render_notes(
            f,
            layout.notes,
            state.notes_buffer(),
            state.focus,
            state.mode,
            theme,
        );
        job_output_view::render(f, layout.job, state, theme);
        visualizer_view::render(f, layout.visualizer, state, theme);
        gate_monitor_view::render(f, layout.gate, state, theme);
        bottom_bar::render(f, layout.bottom, state, theme);

        if state.show_help {
            let area = centered_rect(60, 40, f.size());
            help_overlay::render(f, area, theme);
        }

        if let Some(Prompt::ConfirmQuit) = state.prompt {
            let area = centered_rect(50, 20, f.size());
            render_prompt(f, area, theme, "Quit without saving? (Y/N)");
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
    state.metrics.last_render_ms = start.elapsed().as_millis();
    state.metrics.frame_count += 1;
    Ok(())
}

fn map_key_to_action(key: KeyEvent, state: &AppState) -> Option<Action> {
    // Prompt confirm takes precedence
    if let Some(Prompt::ConfirmQuit) = state.prompt {
        return match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Some(Action::ConfirmQuitYes),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some(Action::ConfirmQuitNo),
            _ => None,
        };
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
        } => Some(Action::ToggleMode),
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
            code: KeyCode::Char('l'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => Some(Action::ClearLogs),
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

fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    r: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    use ratatui::layout::{Constraint, Direction, Layout};
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ]
            .as_ref(),
        )
        .split(r);
    let vertical = popup_layout[1];
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ]
            .as_ref(),
        )
        .split(vertical)[1]
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
        .title("CONFIRM");
    let paragraph = Paragraph::new(message)
        .style(Style::default().bg(theme.background).fg(theme.foreground))
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
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
}
