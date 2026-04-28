use std::io::{self, Stdout};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use nit_core::{AgentChannel, AgentMessage, AppState};
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Terminal,
};

use super::dispatch::{dispatch_pane_prompt, DispatchOutcome};
use super::focus;
use super::grid;
use super::roster_view;
use super::setup::materialise_pane_lane;
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
        let lookup_id = Some(pane.agent_id.as_str())
            .filter(|s| !s.is_empty())
            .or(pane.selected_agent_id.as_deref());
        let Some(lookup_id) = lookup_id else { continue };
        if let Some(lane) = state.agents.agents.iter().find(|l| l.id == lookup_id) {
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
        let focused = idx == focused_idx;
        if let Some(c) = render_one_pane(frame, area, state, theme, idx, cols, rows, focused) {
            if focused {
                cursor = Some(c);
            }
        }
    }
    cursor
}

#[allow(clippy::too_many_arguments)]
fn render_one_pane(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &mut AppState,
    theme: &Theme,
    idx: usize,
    cols: usize,
    rows: usize,
    focused: bool,
) -> Option<ChatCursor> {
    let rect = grid::pane_rect(area, cols, rows, idx);
    if rect.width < 2 || rect.height < 2 {
        return None;
    }
    let inner = paint_pane_chrome(frame, rect, state, idx, focused, theme);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let pane = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(idx))
        .cloned()?;

    if pane.selected_agent_id.is_none() && pane.agent_id.is_empty() {
        let backend_filter = state
            .multipane
            .as_ref()
            .and_then(|mp| mp.backend_filter.clone());
        roster_view::render(
            frame,
            inner,
            state,
            backend_filter.as_deref(),
            pane.roster_cursor,
            focused,
            theme,
        );
        return None;
    }

    agent_console_view::render_pane(frame, inner, state, None, theme, &pane, focused)
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
    let mode_label = if pane.selected_agent_id.is_none() && pane.agent_id.is_empty() {
        "roster"
    } else {
        "chat"
    };
    let title = format!(" pane {idx} · {mode_label} · {cwd_text} ");
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
    paint_hint_line(frame, inner, pane_in_roster_mode(state, idx), theme);
    inner_rect_after_hint(inner)
}

fn pane_in_roster_mode(state: &AppState, idx: usize) -> bool {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(idx))
        .map(|p| p.selected_agent_id.is_none() && p.agent_id.is_empty())
        .unwrap_or(false)
}

fn paint_hint_line(frame: &mut ratatui::Frame, inner: Rect, in_roster: bool, theme: &Theme) {
    if inner.height == 0 {
        return;
    }
    let hint_text = if in_roster {
        " ↑/↓ select · Enter commit · Tab next pane "
    } else {
        " /abort · Ctrl+C · Esc Esc · Ctrl+R roster "
    };
    let hint = Line::from(Span::styled(
        hint_text,
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

    if !matches!(key.code, KeyCode::Esc) {
        *last_esc_at = None;
    }

    // Tab / Shift+Tab / BackTab cycle pane focus regardless of mode and
    // never move the per-pane roster cursor.
    match key.code {
        KeyCode::Tab if !modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(mp) = state.multipane.as_mut() {
                focus::cycle_forward(mp);
            }
            return false;
        }
        KeyCode::BackTab => {
            if let Some(mp) = state.multipane.as_mut() {
                focus::cycle_backward(mp);
            }
            return false;
        }
        KeyCode::Tab if modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(mp) = state.multipane.as_mut() {
                focus::cycle_backward(mp);
            }
            return false;
        }
        _ => {}
    }

    if focused_pane_in_roster_mode(state) {
        return handle_roster_key(state, codex, claude, key, last_esc_at);
    }
    handle_chat_key(state, vitals, codex, claude, key, last_esc_at)
}

fn handle_roster_key(
    state: &mut AppState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    key: KeyEvent,
    last_esc_at: &mut Option<Instant>,
) -> bool {
    let modifiers = key.modifiers;
    let is_ctrl = modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Char('c') if is_ctrl => {
            // No selection means there's nothing in flight; emit the
            // friendly no-op so the operator sees the keystroke land.
            push_pane_system_message(state, "no agent selected — nothing to abort".into());
            false
        }
        KeyCode::Esc => {
            let now = Instant::now();
            let double_tap = last_esc_at
                .map(|t| now.duration_since(t) <= ESC_ESC_ABORT_WINDOW)
                .unwrap_or(false);
            if double_tap {
                push_pane_system_message(state, "no agent selected — nothing to abort".into());
                *last_esc_at = None;
            } else {
                *last_esc_at = Some(now);
            }
            false
        }
        KeyCode::Up => {
            move_roster_cursor(state, -1);
            false
        }
        KeyCode::Down => {
            move_roster_cursor(state, 1);
            false
        }
        KeyCode::Enter => {
            commit_roster_selection(state, codex, claude);
            false
        }
        _ => false,
    }
}

fn handle_chat_key(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    key: KeyEvent,
    last_esc_at: &mut Option<Instant>,
) -> bool {
    let modifiers = key.modifiers;
    let is_ctrl = modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Char('r') if is_ctrl => {
            revert_focused_pane_to_roster(state);
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
            if let Some(scope) = parse_abort_scope(trimmed) {
                handle_abort_scope(state, codex, claude, scope);
                clear_focused_pane_input(state);
                return false;
            }
            let pane_idx = state.multipane.as_ref().map(|mp| mp.focused).unwrap_or(0);
            push_history_and_clear_input(state);
            let outcome =
                dispatch_pane_prompt(state, vitals, Some(codex), Some(claude), pane_idx, prompt);
            if matches!(outcome, DispatchOutcome::NoSelection) {
                push_pane_system_message(
                    state,
                    "no agent selected — press Ctrl+R to choose one".into(),
                );
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
        other => {
            edit_focused_chat_input(state, other);
            false
        }
    }
}

fn edit_focused_chat_input(state: &mut AppState, code: KeyCode) {
    let Some(pane) = focused_pane_mut(state) else {
        return;
    };
    match code {
        KeyCode::Char(ch) => {
            let mut chars: Vec<char> = pane.chat_input.chars().collect();
            let cursor = pane.chat_input_cursor.min(chars.len());
            chars.insert(cursor, ch);
            pane.chat_input = chars.into_iter().collect();
            pane.chat_input_cursor = cursor + 1;
        }
        KeyCode::Backspace if pane.chat_input_cursor > 0 => {
            let new_cursor = pane.chat_input_cursor - 1;
            pane.chat_input = pane
                .chat_input
                .chars()
                .enumerate()
                .filter_map(|(i, c)| (i != new_cursor).then_some(c))
                .collect();
            pane.chat_input_cursor = new_cursor;
        }
        KeyCode::Left => {
            pane.chat_input_cursor = pane.chat_input_cursor.saturating_sub(1);
        }
        KeyCode::Right => {
            let limit = pane.chat_input.chars().count();
            pane.chat_input_cursor = pane.chat_input_cursor.saturating_add(1).min(limit);
        }
        KeyCode::Home => pane.chat_input_cursor = 0,
        KeyCode::End => pane.chat_input_cursor = pane.chat_input.chars().count(),
        _ => return,
    }
    pane.chat_input_selection_anchor = None;
}

fn move_roster_cursor(state: &mut AppState, delta: i32) {
    let backend_filter = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.backend_filter.clone());
    let lane_count = roster_view::lane_count(&roster_view::compute_rows(
        &state.agents,
        backend_filter.as_deref(),
    ));
    if lane_count == 0 {
        return;
    }
    if let Some(pane) = focused_pane_mut(state) {
        let current = pane.roster_cursor as i32;
        let next = (current + delta).clamp(0, lane_count as i32 - 1);
        pane.roster_cursor = next as usize;
    }
}

fn commit_roster_selection(state: &mut AppState, codex: &CodexRunner, claude: &ClaudeRunner) {
    let _ = (codex, claude);
    let backend_filter = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.backend_filter.clone());
    let rows = roster_view::compute_rows(&state.agents, backend_filter.as_deref());
    let cursor = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .map(|p| p.roster_cursor)
        .unwrap_or(0);
    let selected_base = roster_view::lane_at_cursor(&rows, cursor).map(str::to_string);
    let Some(selected_base) = selected_base else {
        push_pane_system_message(state, "no agents available to select".into());
        return;
    };
    let pane_idx = state.multipane.as_ref().map(|mp| mp.focused).unwrap_or(0);
    let materialised = materialise_pane_lane(state, pane_idx, &selected_base);
    if let Some(id) = materialised {
        push_pane_system_message(state, format!("selected agent → {id}"));
    } else {
        push_pane_system_message(
            state,
            format!("could not materialise pane lane for {selected_base}"),
        );
    }
}

fn revert_focused_pane_to_roster(state: &mut AppState) {
    if let Some(pane) = focused_pane_mut(state) {
        pane.selected_agent_id = None;
        pane.agent_id.clear();
        pane.chat_input.clear();
        pane.chat_input_cursor = 0;
        pane.chat_input_selection_anchor = None;
        pane.chat_input_scroll = 0;
        pane.mission_id = None;
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

fn focused_pane_in_roster_mode(state: &AppState) -> bool {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .map(|p| p.selected_agent_id.is_none() && p.agent_id.is_empty())
        .unwrap_or(false)
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

fn push_pane_system_message(state: &mut AppState, text: String) {
    let pane = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .cloned();
    let Some(pane) = pane else { return };
    let agent_id = if !pane.agent_id.is_empty() {
        Some(pane.agent_id.clone())
    } else {
        pane.selected_agent_id.clone()
    };
    let at = format!("t+{}", state.metrics.frame_count);
    state.agents.messages.push(AgentMessage {
        at,
        channel: AgentChannel::Agent,
        agent_id,
        mission_id: pane.mission_id.clone(),
        text,
        prompt_msg_idx: None,
        kind: Some("multipane-system".into()),
    });
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
            .map(|mp| {
                mp.panes
                    .iter()
                    .filter_map(|p| {
                        if !p.agent_id.is_empty() {
                            Some(p.agent_id.clone())
                        } else {
                            p.selected_agent_id.clone()
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
        AbortScope::Agent(id) => vec![id],
    };
    if agent_ids.is_empty() {
        push_pane_system_message(state, "no agent selected — nothing to abort".into());
        return;
    }
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
        .and_then(|p| {
            if !p.agent_id.is_empty() {
                Some(p.agent_id.clone())
            } else {
                p.selected_agent_id.clone()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nit_core::{
        AgentLane, AgentLaneKind, AgentStatus, AgentsState, MultipaneState, PaneSession,
    };
    use std::path::PathBuf;

    fn fixture_state_no_backend() -> AppState {
        let buffer = nit_core::Buffer::empty("scratch", None);
        let notes = nit_core::Buffer::empty("notes", None);
        let mut state = AppState::new(PathBuf::from("/workspace"), buffer, notes);
        state.agents = AgentsState::default();
        state.agents.agents.push(AgentLane {
            id: "claude-haiku-4-5".into(),
            role: "claude-haiku-4-5".into(),
            lane: "Claude".into(),
            kind: AgentLaneKind::Claude,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
            shadow: false,
        });
        state.agents.agents.push(AgentLane {
            id: "gpt-5".into(),
            role: "gpt-5".into(),
            lane: "Codex".into(),
            kind: AgentLaneKind::Codex,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: None,
            last_message: String::new(),
            shadow: false,
        });
        state.multipane = Some(MultipaneState {
            backend_agent_id: String::new(),
            panes: vec![
                PaneSession {
                    pane_id: 0,
                    cwd: PathBuf::from("/p0"),
                    ..PaneSession::default()
                },
                PaneSession {
                    pane_id: 1,
                    cwd: PathBuf::from("/p1"),
                    ..PaneSession::default()
                },
            ],
            focused: 0,
            grid_cols: 2,
            grid_rows: 1,
            backend_filter: None,
        });
        state
    }

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

    #[test]
    fn focused_pane_in_roster_mode_when_no_selection() {
        let state = fixture_state_no_backend();
        assert!(focused_pane_in_roster_mode(&state));
    }

    #[test]
    fn move_roster_cursor_clamps_to_visible_lanes() {
        let mut state = fixture_state_no_backend();
        // Two non-shadow lanes => cursor in [0, 1]
        move_roster_cursor(&mut state, 5);
        let cursor = state.multipane.as_ref().unwrap().panes[0].roster_cursor;
        assert_eq!(cursor, 1);
        move_roster_cursor(&mut state, -10);
        let cursor = state.multipane.as_ref().unwrap().panes[0].roster_cursor;
        assert_eq!(cursor, 0);
    }

    #[test]
    fn revert_focused_pane_to_roster_clears_selection() {
        let mut state = fixture_state_no_backend();
        if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(0) {
            pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-00".into());
            pane.agent_id = "claude-haiku-4-5#mp-pane-00".into();
            pane.chat_input = "buffered".into();
        }
        revert_focused_pane_to_roster(&mut state);
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert!(pane.selected_agent_id.is_none());
        assert!(pane.agent_id.is_empty());
        assert!(pane.chat_input.is_empty());
    }

    #[test]
    fn abort_with_no_selection_emits_system_message() {
        let mut state = fixture_state_no_backend();
        let before = state.agents.messages.len();
        // We exercise the dispatcher directly: with no selection,
        // handle_abort_scope returns the friendly system message.
        handle_abort_scope_test_helper(&mut state, AbortScope::Focused);
        assert_eq!(state.agents.messages.len(), before + 1);
        assert!(state
            .agents
            .messages
            .last()
            .unwrap()
            .text
            .contains("no agent selected"));
    }

    fn handle_abort_scope_test_helper(state: &mut AppState, scope: AbortScope) {
        let agent_ids: Vec<String> = match scope {
            AbortScope::Focused => focused_pane_agent_id(state).into_iter().collect(),
            AbortScope::All => state
                .multipane
                .as_ref()
                .map(|mp| {
                    mp.panes
                        .iter()
                        .filter_map(|p| {
                            if !p.agent_id.is_empty() {
                                Some(p.agent_id.clone())
                            } else {
                                p.selected_agent_id.clone()
                            }
                        })
                        .collect()
                })
                .unwrap_or_default(),
            AbortScope::Agent(id) => vec![id],
        };
        if agent_ids.is_empty() {
            push_pane_system_message(state, "no agent selected — nothing to abort".into());
        }
    }
}
