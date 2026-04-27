use std::io::{self, Stdout};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use nit_core::AppState;
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Terminal,
};

use super::dispatch::dispatch_pane_prompt;
use super::focus;
use super::grid;
use crate::claude_runner::{ClaudeCommand, ClaudeRunner, ClaudeRunnerConfig};
use crate::codex_runner::{CodexCommand, CodexRunner, CodexRunnerConfig, CodexRuntimeMode};
use crate::theme::Theme;
use crate::vitals::VitalsState;
use crate::widgets::agent_console_view::{self, ChatCursor};

const TICK_RATE: Duration = Duration::from_millis(50);
const ESC_ESC_ABORT_WINDOW: Duration = Duration::from_millis(500);

/// Multipane main loop. Spawns its own `CodexRunner` + `ClaudeRunner` +
/// `VitalsState`; does NOT spawn `SyntaxRuntime`, file watcher, genome
/// worker, workspace scan, petri/seed/games preview, or mcp_backchannel.
/// `log_rx` is drained-and-discarded in v1 (Phase 5 polish: route to a
/// status row).
pub fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut AppState,
    theme: &Theme,
    log_rx: Receiver<String>,
    codex_runtime: CodexRuntimeMode,
    codex_config: CodexRunnerConfig,
    claude_config: ClaudeRunnerConfig,
) -> io::Result<()> {
    state.agents.codex_max_parallel_turns = codex_config.max_parallel_turns;
    state.agents.claude_max_parallel_turns = claude_config.max_parallel_turns;
    let codex = CodexRunner::spawn(codex_runtime, codex_config, None);
    let claude = ClaudeRunner::spawn(claude_config);
    let mut vitals = VitalsState::default();
    let mut last_esc_at: Option<Instant> = None;

    loop {
        for log_line in log_rx.try_iter() {
            // v1: discard. Phase 5 may route to a status row.
            let _ = log_line;
        }
        for event in codex.events.try_iter() {
            event.apply(state);
        }
        for event in claude.events.try_iter() {
            event.apply(state);
        }
        capture_pane_mission_ids(state);

        terminal.draw(|frame| {
            let area = frame.size();
            let cursor = render_grid(frame, area, state, theme);
            if let Some(c) = cursor {
                frame.set_cursor(c.x, c.y);
            }
        })?;

        if !event::poll(TICK_RATE)? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if handle_key(state, &mut vitals, &codex, &claude, key, &mut last_esc_at) {
                    return Ok(());
                }
            }
            Event::Mouse(mouse) => {
                handle_mouse(state, terminal_size(terminal)?, mouse);
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

fn terminal_size(terminal: &Terminal<CrosstermBackend<Stdout>>) -> io::Result<Rect> {
    let size = terminal.size()?;
    Ok(Rect::new(0, 0, size.width, size.height))
}

/// Each `dispatch_pane_prompt` allocates a mission inside the standard
/// dispatch path; capture the resulting mission_id back into the pane so
/// subsequent abort routing targets the right mission.
fn capture_pane_mission_ids(state: &mut AppState) {
    let Some(mp) = state.multipane.as_mut() else {
        return;
    };
    for pane in &mut mp.panes {
        if pane.mission_id.is_some() {
            continue;
        }
        if let Some(lane) = state.agents.agents.iter().find(|l| l.id == pane.agent_id) {
            pane.mission_id = lane.current_mission.clone();
        }
    }
}

fn render_grid(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &mut AppState,
    theme: &Theme,
) -> Option<ChatCursor> {
    let (panes_len, focused_idx, cols, rows) = {
        let mp = state.multipane.as_ref()?;
        (mp.panes.len(), mp.focused, mp.grid_cols, mp.grid_rows)
    };
    if panes_len == 0 {
        return None;
    }

    let mut cursor: Option<ChatCursor> = None;
    for idx in 0..panes_len {
        let rect = grid::pane_rect(area, cols, rows, idx);
        if rect.width < 2 || rect.height < 2 {
            continue;
        }
        let focused = idx == focused_idx;
        let inner = paint_pane_chrome(frame, rect, state, idx, focused, theme);
        if inner.width == 0 || inner.height == 0 {
            continue;
        }
        // Borrow pane immutably for render — the chrome already mutated
        // any cache fields it needs, so this clone is cheap (PaneSession
        // is fields-only, no Arc / Vec of size).
        let pane_clone = state
            .multipane
            .as_ref()
            .and_then(|mp| mp.panes.get(idx))
            .cloned();
        let Some(pane) = pane_clone else { continue };
        if let Some(c) =
            agent_console_view::render_pane(frame, inner, state, None, theme, &pane, focused)
        {
            if focused {
                cursor = Some(c);
            }
        }
    }
    cursor
}

fn paint_pane_chrome(
    frame: &mut ratatui::Frame,
    rect: Rect,
    state: &AppState,
    idx: usize,
    focused: bool,
    theme: &Theme,
) -> Rect {
    let mp = match state.multipane.as_ref() {
        Some(mp) => mp,
        None => return rect,
    };
    let Some(pane) = mp.panes.get(idx) else {
        return rect;
    };
    let cwd_text = pane.cwd.display().to_string();
    let title = format!(" pane {idx} · {cwd_text} ");
    let border_color = if focused {
        theme.border_focused
    } else {
        theme.border
    };
    let title_color = if focused {
        theme.title_focused
    } else {
        theme.title
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(if focused {
            BorderType::Thick
        } else {
            BorderType::Plain
        })
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            title,
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(theme.background));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    paint_hint_line(frame, inner, theme);
    inner_rect_after_hint(inner)
}

fn paint_hint_line(frame: &mut ratatui::Frame, inner: Rect, theme: &Theme) {
    if inner.height == 0 {
        return;
    }
    let hint = Line::from(Span::styled(
        " /abort · Ctrl+C · Esc Esc ",
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::ITALIC | Modifier::DIM),
    ));
    let hint_rect = Rect::new(inner.x, inner.y, inner.width, 1);
    frame.render_widget(Paragraph::new(hint), hint_rect);
}

fn inner_rect_after_hint(inner: Rect) -> Rect {
    if inner.height <= 1 {
        return Rect::new(inner.x, inner.y, inner.width, 0);
    }
    Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1)
}

/// Handle a key event. Returns `true` to exit the loop.
fn handle_key(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    key: KeyEvent,
    last_esc_at: &mut Option<Instant>,
) -> bool {
    let modifiers = key.modifiers;
    let is_ctrl = modifiers.contains(KeyModifiers::CONTROL);

    if !matches!(key.code, KeyCode::Esc) {
        *last_esc_at = None;
    }

    match key.code {
        KeyCode::Tab if !modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(mp) = state.multipane.as_mut() {
                focus::cycle_forward(mp);
            }
            false
        }
        KeyCode::BackTab => {
            if let Some(mp) = state.multipane.as_mut() {
                focus::cycle_backward(mp);
            }
            false
        }
        KeyCode::Tab if modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(mp) = state.multipane.as_mut() {
                focus::cycle_backward(mp);
            }
            false
        }
        KeyCode::Char('c') if is_ctrl => {
            let empty = focused_chat_input_is_empty(state);
            if empty {
                abort_focused_pane(state, codex, claude);
            } else if let Some(pane) = focused_pane_mut(state) {
                pane.chat_input.clear();
                pane.chat_input_cursor = 0;
                pane.chat_input_selection_anchor = None;
                pane.chat_input_scroll = 0;
            }
            false
        }
        KeyCode::Esc => {
            let now = Instant::now();
            let double_tap = last_esc_at
                .map(|t| now.duration_since(t) <= ESC_ESC_ABORT_WINDOW)
                .unwrap_or(false);
            if double_tap {
                abort_focused_pane(state, codex, claude);
                *last_esc_at = None;
            } else {
                *last_esc_at = Some(now);
            }
            false
        }
        KeyCode::Enter => {
            let prompt = focused_pane_input(state);
            let trimmed = prompt.trim();
            if trimmed.is_empty() {
                return false;
            }
            // /abort and @abort short-circuit the dispatch path so they
            // never get sent as prompts to the runner.
            if let Some(scope) = parse_abort_scope(trimmed) {
                handle_abort_scope(state, codex, claude, scope);
                clear_focused_pane_input(state);
                return false;
            }
            let pane_idx = state.multipane.as_ref().map(|mp| mp.focused).unwrap_or(0);
            push_history_and_clear_input(state);
            dispatch_pane_prompt(state, vitals, Some(codex), Some(claude), pane_idx, prompt);
            false
        }
        KeyCode::Backspace => {
            if let Some(pane) = focused_pane_mut(state) {
                if pane.chat_input_cursor > 0 {
                    let chars: Vec<char> = pane.chat_input.chars().collect();
                    let new_cursor = pane.chat_input_cursor - 1;
                    let mut next = String::with_capacity(pane.chat_input.len());
                    for (i, c) in chars.iter().enumerate() {
                        if i != new_cursor {
                            next.push(*c);
                        }
                    }
                    pane.chat_input = next;
                    pane.chat_input_cursor = new_cursor;
                    pane.chat_input_selection_anchor = None;
                }
            }
            false
        }
        KeyCode::Left => {
            if let Some(pane) = focused_pane_mut(state) {
                pane.chat_input_cursor = pane.chat_input_cursor.saturating_sub(1);
                pane.chat_input_selection_anchor = None;
            }
            false
        }
        KeyCode::Right => {
            if let Some(pane) = focused_pane_mut(state) {
                let max_cursor = pane.chat_input.chars().count();
                pane.chat_input_cursor = pane.chat_input_cursor.saturating_add(1).min(max_cursor);
                pane.chat_input_selection_anchor = None;
            }
            false
        }
        KeyCode::Up => {
            walk_history(state, true);
            false
        }
        KeyCode::Down => {
            walk_history(state, false);
            false
        }
        KeyCode::Home => {
            if let Some(pane) = focused_pane_mut(state) {
                pane.chat_input_cursor = 0;
            }
            false
        }
        KeyCode::End => {
            if let Some(pane) = focused_pane_mut(state) {
                pane.chat_input_cursor = pane.chat_input.chars().count();
            }
            false
        }
        KeyCode::Char(c) => {
            if let Some(pane) = focused_pane_mut(state) {
                let mut chars: Vec<char> = pane.chat_input.chars().collect();
                let cursor = pane.chat_input_cursor.min(chars.len());
                chars.insert(cursor, c);
                pane.chat_input = chars.into_iter().collect();
                pane.chat_input_cursor = cursor + 1;
                pane.chat_input_selection_anchor = None;
            }
            false
        }
        _ => false,
    }
}

fn handle_mouse(state: &mut AppState, area: Rect, mouse: MouseEvent) {
    if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
        return;
    }
    if let Some(mp) = state.multipane.as_mut() {
        focus::focus_at_point(mp, area, mouse.column, mouse.row);
    }
}

fn focused_pane_mut(state: &mut AppState) -> Option<&mut nit_core::PaneSession> {
    let mp = state.multipane.as_mut()?;
    let idx = mp.focused;
    mp.panes.get_mut(idx)
}

fn focused_chat_input_is_empty(state: &AppState) -> bool {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .map(|p| p.chat_input.trim().is_empty())
        .unwrap_or(true)
}

fn focused_pane_input(state: &AppState) -> String {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .map(|p| p.chat_input.clone())
        .unwrap_or_default()
}

fn clear_focused_pane_input(state: &mut AppState) {
    if let Some(pane) = focused_pane_mut(state) {
        pane.chat_input.clear();
        pane.chat_input_cursor = 0;
        pane.chat_input_selection_anchor = None;
        pane.chat_input_scroll = 0;
    }
}

fn push_history_and_clear_input(state: &mut AppState) {
    if let Some(pane) = focused_pane_mut(state) {
        let prompt = pane.chat_input.clone();
        if !prompt.trim().is_empty() {
            pane.chat_prompt_history.push(prompt);
        }
        pane.chat_prompt_history_pos = None;
        pane.chat_input.clear();
        pane.chat_input_cursor = 0;
        pane.chat_input_selection_anchor = None;
        pane.chat_input_scroll = 0;
    }
}

fn walk_history(state: &mut AppState, up: bool) {
    let Some(pane) = focused_pane_mut(state) else {
        return;
    };
    if pane.chat_prompt_history.is_empty() {
        return;
    }
    let len = pane.chat_prompt_history.len();
    let next = match (pane.chat_prompt_history_pos, up) {
        (None, true) => Some(len - 1),
        (None, false) => None,
        (Some(0), true) => Some(0),
        (Some(idx), true) => Some(idx - 1),
        (Some(idx), false) if idx + 1 >= len => None,
        (Some(idx), false) => Some(idx + 1),
    };
    pane.chat_prompt_history_pos = next;
    pane.chat_input = match next {
        Some(idx) => pane.chat_prompt_history[idx].clone(),
        None => String::new(),
    };
    pane.chat_input_cursor = pane.chat_input.chars().count();
    pane.chat_input_selection_anchor = None;
    pane.chat_input_scroll = 0;
}

#[derive(Debug, PartialEq, Eq)]
enum AbortScope {
    /// Cancel focused pane only (default for `/abort` with no arg).
    Focused,
    /// Cancel every pane.
    All,
    /// Cancel a specific agent_id.
    Agent(String),
}

fn parse_abort_scope(input: &str) -> Option<AbortScope> {
    let trimmed = input.trim_start();
    let after = trimmed
        .strip_prefix("/abort")
        .or_else(|| trimmed.strip_prefix("@abort"))?;
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        return None;
    }
    let arg = after.trim();
    if arg.is_empty() {
        return Some(AbortScope::Focused);
    }
    if arg.eq_ignore_ascii_case("all") {
        return Some(AbortScope::All);
    }
    Some(AbortScope::Agent(arg.to_string()))
}

fn handle_abort_scope(
    state: &mut AppState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    scope: AbortScope,
) {
    let agent_ids: Vec<String> = match scope {
        AbortScope::Focused => focused_pane_agent_id(state).into_iter().collect(),
        AbortScope::All => state
            .multipane
            .as_ref()
            .map(|mp| mp.panes.iter().map(|p| p.agent_id.clone()).collect())
            .unwrap_or_default(),
        AbortScope::Agent(id) => vec![id],
    };
    for agent_id in agent_ids {
        crate::swarm::drain_queued_turns_for_agent_pub(state, &agent_id);
        let _ = codex.send(CodexCommand::CancelTurn {
            agent_id: agent_id.clone(),
        });
        let _ = claude.send(ClaudeCommand::CancelTurn { agent_id });
    }
}

fn abort_focused_pane(state: &mut AppState, codex: &CodexRunner, claude: &ClaudeRunner) {
    handle_abort_scope(state, codex, claude, AbortScope::Focused);
}

fn focused_pane_agent_id(state: &AppState) -> Option<String> {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .map(|p| p.agent_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_abort_scope_recognises_forms() {
        assert_eq!(parse_abort_scope("/abort"), Some(AbortScope::Focused));
        assert_eq!(parse_abort_scope("@abort"), Some(AbortScope::Focused));
        assert_eq!(parse_abort_scope("/abort all"), Some(AbortScope::All));
        assert_eq!(parse_abort_scope("/abort  ALL"), Some(AbortScope::All));
        assert_eq!(
            parse_abort_scope("/abort claude#mp-pane-02"),
            Some(AbortScope::Agent("claude#mp-pane-02".into()))
        );
    }

    #[test]
    fn parse_abort_scope_rejects_substring_match() {
        assert_eq!(parse_abort_scope("/abortif"), None);
        assert_eq!(parse_abort_scope("just a regular prompt"), None);
    }
}
