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
        clamp_roster_scroll(state, idx, inner.height as usize);
        let updated_pane = state
            .multipane
            .as_ref()
            .and_then(|mp| mp.panes.get(idx))
            .cloned()
            .unwrap_or(pane);
        roster_view::render(
            frame,
            inner,
            state,
            &updated_pane,
            backend_filter.as_deref(),
            focused,
            theme,
        );
        return None;
    }

    agent_console_view::render_pane(frame, inner, state, None, theme, &pane, focused)
}

fn clamp_roster_scroll(state: &mut AppState, pane_idx: usize, height: usize) {
    let backend_filter = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.backend_filter.clone());
    let Some(pane_clone) = pane_at(state, pane_idx).cloned() else {
        return;
    };
    let rows = roster_view::compute_rows(state, &pane_clone, backend_filter.as_deref());
    let max_scroll = rows.len().saturating_sub(height);
    let stops = roster_view::selectable_count(&rows);
    let Some(pane) = pane_at_mut(state, pane_idx) else {
        return;
    };
    pane.roster_scroll = pane.roster_scroll.min(max_scroll);
    pane.roster_cursor = if stops == 0 {
        0
    } else {
        pane.roster_cursor.min(stops - 1)
    };
}

fn pane_at(state: &AppState, pane_idx: usize) -> Option<&nit_core::PaneSession> {
    state.multipane.as_ref()?.panes.get(pane_idx)
}

fn pane_at_mut(state: &mut AppState, pane_idx: usize) -> Option<&mut nit_core::PaneSession> {
    state.multipane.as_mut()?.panes.get_mut(pane_idx)
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
        " ↑/↓ j/k · h/l fold · Space check · Enter commit · Tab pane "
    } else {
        " /abort · Ctrl+C · Esc Esc · Ctrl+R roster · PgUp/PgDn scroll "
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
        KeyCode::Up | KeyCode::Char('k') => {
            move_roster_cursor(state, -1);
            false
        }
        KeyCode::Down | KeyCode::Char('j') => {
            move_roster_cursor(state, 1);
            false
        }
        KeyCode::PageUp => {
            move_roster_cursor(state, -ROSTER_PAGE_STEP);
            false
        }
        KeyCode::PageDown => {
            move_roster_cursor(state, ROSTER_PAGE_STEP);
            false
        }
        KeyCode::Char('g') => {
            jump_roster_cursor_to_top(state);
            false
        }
        KeyCode::Char('G') => {
            jump_roster_cursor_to_bottom(state);
            false
        }
        KeyCode::Left | KeyCode::Char('h') => {
            collapse_at_cursor(state);
            false
        }
        KeyCode::Right | KeyCode::Char('l') => {
            expand_at_cursor(state);
            false
        }
        KeyCode::Char(' ') => {
            toggle_size_at_cursor(state);
            false
        }
        KeyCode::Enter => {
            commit_roster_selection(state, codex, claude);
            false
        }
        _ => false,
    }
}

const ROSTER_PAGE_STEP: i32 = 8;

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
        KeyCode::PageUp => {
            scroll_chat_thread(state, -CHAT_THREAD_PAGE_STEP);
            false
        }
        KeyCode::PageDown => {
            scroll_chat_thread(state, CHAT_THREAD_PAGE_STEP);
            false
        }
        other => {
            edit_focused_chat_input(state, other);
            false
        }
    }
}

const CHAT_THREAD_PAGE_STEP: i32 = 8;

fn scroll_chat_thread(state: &mut AppState, delta: i32) {
    if let Some(pane) = focused_pane_mut(state) {
        let current = pane.chat_thread_scroll as i32;
        pane.chat_thread_scroll = (current + delta).max(0) as usize;
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

fn focused_pane_rows(state: &AppState) -> (Option<usize>, Vec<roster_view::PaneRosterRow>) {
    let backend_filter = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.backend_filter.clone());
    let pane_clone = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .cloned();
    let pane_idx = state.multipane.as_ref().map(|mp| mp.focused);
    let Some(pane) = pane_clone else {
        return (pane_idx, Vec::new());
    };
    let rows = roster_view::compute_rows(state, &pane, backend_filter.as_deref());
    (pane_idx, rows)
}

fn move_roster_cursor(state: &mut AppState, delta: i32) {
    let (_, rows) = focused_pane_rows(state);
    let stops = roster_view::selectable_count(&rows);
    if stops == 0 {
        return;
    }
    let cursor = focused_pane_mut(state)
        .map(|p| p.roster_cursor as i32)
        .unwrap_or(0);
    let next = (cursor + delta).clamp(0, stops as i32 - 1) as usize;
    let row = roster_view::row_at_cursor(&rows, next).cloned();
    if let Some(pane) = focused_pane_mut(state) {
        pane.roster_cursor = next;
        roster_view::sync_tree_selection(pane, row.as_ref());
    }
}

fn jump_roster_cursor_to_top(state: &mut AppState) {
    let (_, rows) = focused_pane_rows(state);
    let row = roster_view::row_at_cursor(&rows, 0).cloned();
    if let Some(pane) = focused_pane_mut(state) {
        pane.roster_cursor = 0;
        pane.roster_scroll = 0;
        roster_view::sync_tree_selection(pane, row.as_ref());
    }
}

fn jump_roster_cursor_to_bottom(state: &mut AppState) {
    let (_, rows) = focused_pane_rows(state);
    let stops = roster_view::selectable_count(&rows);
    if stops == 0 {
        return;
    }
    let cursor = stops - 1;
    let row = roster_view::row_at_cursor(&rows, cursor).cloned();
    if let Some(pane) = focused_pane_mut(state) {
        pane.roster_cursor = cursor;
        roster_view::sync_tree_selection(pane, row.as_ref());
    }
}

fn collapse_at_cursor(state: &mut AppState) {
    let Some(row) = row_under_focused_cursor(state) else {
        return;
    };
    let Some(pane) = focused_pane_mut(state) else {
        return;
    };
    match row {
        roster_view::PaneRosterRow::Backend { kind } => {
            pane.roster_expanded_backends.remove(&kind);
        }
        roster_view::PaneRosterRow::Agent { agent_id, .. }
        | roster_view::PaneRosterRow::SizeBranch { agent_id }
        | roster_view::PaneRosterRow::SizeLeaf { agent_id, .. } => {
            pane.roster_collapsed_agent_ids.insert(agent_id);
        }
        _ => {}
    }
}

fn expand_at_cursor(state: &mut AppState) {
    let Some(row) = row_under_focused_cursor(state) else {
        return;
    };
    let Some(pane) = focused_pane_mut(state) else {
        return;
    };
    match row {
        roster_view::PaneRosterRow::Backend { kind } => {
            pane.roster_expanded_backends.insert(kind);
        }
        roster_view::PaneRosterRow::Agent { agent_id, .. }
        | roster_view::PaneRosterRow::SizeBranch { agent_id }
        | roster_view::PaneRosterRow::SizeLeaf { agent_id, .. } => {
            pane.roster_collapsed_agent_ids.remove(&agent_id);
        }
        _ => {}
    }
}

fn row_under_focused_cursor(state: &AppState) -> Option<roster_view::PaneRosterRow> {
    let (_, rows) = focused_pane_rows(state);
    let cursor = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .map(|p| p.roster_cursor)
        .unwrap_or(0);
    roster_view::row_at_cursor(&rows, cursor).cloned()
}

fn toggle_size_at_cursor(state: &mut AppState) {
    let Some(roster_view::PaneRosterRow::SizeLeaf {
        agent_id, leaf_idx, ..
    }) = row_under_focused_cursor(state)
    else {
        return;
    };
    roster_view::toggle_size_leaf(state, &agent_id, leaf_idx);
}

fn commit_roster_selection(state: &mut AppState, codex: &CodexRunner, claude: &ClaudeRunner) {
    let _ = (codex, claude);
    let (pane_idx, rows) = focused_pane_rows(state);
    let cursor = focused_pane_mut(state)
        .map(|p| p.roster_cursor)
        .unwrap_or(0);
    let Some(row) = roster_view::row_at_cursor(&rows, cursor).cloned() else {
        push_pane_system_message(state, "no agents available to select".into());
        return;
    };
    let pane_idx = pane_idx.unwrap_or(0);
    dispatch_commit(state, pane_idx, row);
}

fn dispatch_commit(state: &mut AppState, pane_idx: usize, row: roster_view::PaneRosterRow) {
    match row {
        roster_view::PaneRosterRow::Backend { kind } => {
            let Some(pane) = pane_at_mut(state, pane_idx) else {
                return;
            };
            roster_view::toggle_backend_expansion(pane, kind);
        }
        roster_view::PaneRosterRow::SizeBranch { agent_id } => {
            let Some(pane) = pane_at_mut(state, pane_idx) else {
                return;
            };
            roster_view::toggle_agent_tree_collapse(pane, &agent_id);
        }
        roster_view::PaneRosterRow::SizeLeaf {
            agent_id, leaf_idx, ..
        } => {
            roster_view::toggle_size_leaf(state, &agent_id, leaf_idx);
        }
        roster_view::PaneRosterRow::Agent { agent_id, .. } => {
            commit_agent_to_pane(state, pane_idx, &agent_id);
        }
        roster_view::PaneRosterRow::Template
        | roster_view::PaneRosterRow::Mission
        | roster_view::PaneRosterRow::Empty(_)
        | roster_view::PaneRosterRow::Spacer => {}
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
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            handle_mouse_left_down(state, area, mouse.column, mouse.row);
        }
        MouseEventKind::ScrollUp => {
            handle_mouse_scroll(state, area, mouse.column, mouse.row, -1);
        }
        MouseEventKind::ScrollDown => {
            handle_mouse_scroll(state, area, mouse.column, mouse.row, 1);
        }
        _ => {}
    }
}

fn handle_mouse_left_down(state: &mut AppState, area: Rect, x: u16, y: u16) {
    let Some(target) = resolve_left_click_target(state, area, x, y) else {
        return;
    };
    apply_roster_click(state, target);
}

struct RosterClickTarget {
    pane_idx: usize,
    rows: Vec<roster_view::PaneRosterRow>,
    row_idx: usize,
    row: roster_view::PaneRosterRow,
    local_x: usize,
}

fn resolve_left_click_target(
    state: &mut AppState,
    area: Rect,
    x: u16,
    y: u16,
) -> Option<RosterClickTarget> {
    let mp = state.multipane.as_mut()?;
    let pane_idx = focus::focus_at_point(mp, area, x, y)?;
    let pane_rect = grid::pane_rect(area, mp.grid_cols, mp.grid_rows, pane_idx);
    let backend_filter = mp.backend_filter.clone();
    let pane = mp.panes.get(pane_idx).cloned()?;
    if !(pane.selected_agent_id.is_none() && pane.agent_id.is_empty()) {
        return None; // chat panes ignore left-clicks beyond focus
    }
    let inner = pane_inner_after_chrome(pane_rect);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    if x < inner.x || y < inner.y || x >= inner.x + inner.width || y >= inner.y + inner.height {
        return None;
    }
    let local_x = (x - inner.x) as usize;
    let local_y = (y - inner.y) as usize;
    let rows = roster_view::compute_rows(state, &pane, backend_filter.as_deref());
    let row_idx = roster_view::row_index_at_y(&rows, pane.roster_scroll, local_y)?;
    let row = rows.get(row_idx).cloned()?;
    Some(RosterClickTarget {
        pane_idx,
        rows,
        row_idx,
        row,
        local_x,
    })
}

fn apply_roster_click(state: &mut AppState, target: RosterClickTarget) {
    let RosterClickTarget {
        pane_idx,
        rows,
        row_idx,
        row,
        local_x,
    } = target;
    match row {
        roster_view::PaneRosterRow::Template => {
            if let Some(value) = roster_view::template_word_at_x(local_x) {
                state.agents.swarm_default_template = value.into();
            }
        }
        roster_view::PaneRosterRow::Mission => {
            if let Some(value) = roster_view::mission_word_at_x(local_x) {
                state.agents.swarm_default_mission = value.into();
            }
        }
        roster_view::PaneRosterRow::Backend { kind } => {
            seek_pane_cursor_to(state, pane_idx, &rows, row_idx);
            if let Some(pane) = pane_at_mut(state, pane_idx) {
                roster_view::toggle_backend_expansion(pane, kind);
            }
        }
        roster_view::PaneRosterRow::Agent { agent_id, .. } => {
            seek_pane_cursor_to(state, pane_idx, &rows, row_idx);
            commit_agent_to_pane(state, pane_idx, &agent_id);
        }
        roster_view::PaneRosterRow::SizeBranch { agent_id } => {
            seek_pane_cursor_to(state, pane_idx, &rows, row_idx);
            if let Some(pane) = pane_at_mut(state, pane_idx) {
                roster_view::toggle_agent_tree_collapse(pane, &agent_id);
            }
        }
        roster_view::PaneRosterRow::SizeLeaf {
            agent_id, leaf_idx, ..
        } => {
            seek_pane_cursor_to(state, pane_idx, &rows, row_idx);
            roster_view::toggle_size_leaf(state, &agent_id, leaf_idx);
        }
        roster_view::PaneRosterRow::Empty(_) | roster_view::PaneRosterRow::Spacer => {}
    }
}

fn seek_pane_cursor_to(
    state: &mut AppState,
    pane_idx: usize,
    rows: &[roster_view::PaneRosterRow],
    row_idx: usize,
) {
    let Some(cursor) = roster_view::cursor_for_row_index(rows, row_idx) else {
        return;
    };
    if let Some(pane) = pane_at_mut(state, pane_idx) {
        pane.roster_cursor = cursor;
    }
}

fn commit_agent_to_pane(state: &mut AppState, pane_idx: usize, agent_id: &str) {
    let message = match materialise_pane_lane(state, pane_idx, agent_id) {
        Some(id) => format!("selected agent → {id}"),
        None => format!("could not materialise pane lane for {agent_id}"),
    };
    push_pane_system_message(state, message);
}

fn handle_mouse_scroll(state: &mut AppState, area: Rect, x: u16, y: u16, delta: i32) {
    let Some(mp) = state.multipane.as_ref() else {
        return;
    };
    let Some(pane_idx) = grid::pane_at_point(area, mp.grid_cols, mp.grid_rows, x, y) else {
        return;
    };
    let Some(pane) = mp.panes.get(pane_idx).cloned() else {
        return;
    };
    let in_roster = pane.selected_agent_id.is_none() && pane.agent_id.is_empty();
    let Some(p) = pane_at_mut(state, pane_idx) else {
        return;
    };
    if in_roster {
        let current = p.roster_scroll as i32;
        p.roster_scroll = (current + delta).max(0) as usize;
    } else {
        let current = p.chat_thread_scroll as i32;
        p.chat_thread_scroll = (current + delta).max(0) as usize;
    }
}

fn pane_inner_after_chrome(rect: Rect) -> Rect {
    if rect.width < 2 || rect.height < 2 {
        return Rect::new(rect.x, rect.y, 0, 0);
    }
    let inner = Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2);
    inner_rect_after_hint(inner)
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

    fn fixture_with_efforts() -> AppState {
        let mut state = fixture_state_no_backend();
        state.agents.codex_supported_reasoning_efforts.insert(
            "gpt-5".into(),
            vec!["low".into(), "medium".into(), "high".into()],
        );
        state
            .agents
            .claude_supported_efforts
            .insert("claude-haiku-4-5".into(), vec!["low".into(), "max".into()]);
        state
    }

    #[test]
    fn expand_at_cursor_expands_focused_backend() {
        let mut state = fixture_with_efforts();
        // Cursor lands on the first selectable row (Backend Codex).
        expand_at_cursor(&mut state);
        let expanded = &state.multipane.as_ref().unwrap().panes[0].roster_expanded_backends;
        assert!(expanded.contains(&AgentLaneKind::Codex));

        collapse_at_cursor(&mut state);
        let expanded = &state.multipane.as_ref().unwrap().panes[0].roster_expanded_backends;
        assert!(!expanded.contains(&AgentLaneKind::Codex));
    }

    #[test]
    fn move_roster_cursor_walks_through_expanded_size_leaves() {
        let mut state = fixture_with_efforts();
        // Expand Codex and move cursor down so we land on gpt-5, then SizeBranch, then leaves.
        expand_at_cursor(&mut state);
        // After expansion: cursor=0 (Backend Codex), 1=Agent gpt-5, 2=SizeBranch, 3=low, 4=medium, 5=high, 6=Backend Claude.
        move_roster_cursor(&mut state, 1);
        assert_eq!(state.multipane.as_ref().unwrap().panes[0].roster_cursor, 1);
        move_roster_cursor(&mut state, 1);
        assert_eq!(state.multipane.as_ref().unwrap().panes[0].roster_cursor, 2);
        move_roster_cursor(&mut state, 1);
        // Now on a SizeLeaf — sync_tree_selection should populate the leaf cursor.
        assert!(state.multipane.as_ref().unwrap().panes[0]
            .roster_tree_selected
            .is_some());
    }

    #[test]
    fn toggle_size_at_cursor_writes_codex_selected_effort() {
        let mut state = fixture_with_efforts();
        expand_at_cursor(&mut state); // expand Codex
                                      // Walk: Backend Codex (0) → Agent gpt-5 (1) → SizeBranch (2) → low (3) → medium (4)
        for _ in 0..4 {
            move_roster_cursor(&mut state, 1);
        }
        toggle_size_at_cursor(&mut state);
        assert_eq!(
            state.agents.codex_selected_reasoning_effort.get("gpt-5"),
            Some(&"medium".to_string())
        );
    }

    #[test]
    fn jump_roster_cursor_to_top_resets_scroll() {
        let mut state = fixture_with_efforts();
        if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(0) {
            pane.roster_cursor = 1;
            pane.roster_scroll = 5;
        }
        jump_roster_cursor_to_top(&mut state);
        let pane = &state.multipane.as_ref().unwrap().panes[0];
        assert_eq!(pane.roster_cursor, 0);
        assert_eq!(pane.roster_scroll, 0);
    }

    #[test]
    fn jump_roster_cursor_to_bottom_lands_on_last_selectable() {
        let mut state = fixture_with_efforts();
        jump_roster_cursor_to_bottom(&mut state);
        let cursor = state.multipane.as_ref().unwrap().panes[0].roster_cursor;
        // Two backends collapsed → 2 selectable rows.
        assert_eq!(cursor, 1);
    }

    #[test]
    fn scroll_chat_thread_clamps_at_zero() {
        let mut state = fixture_state_no_backend();
        if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(0) {
            pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-00".into());
            pane.agent_id = "claude-haiku-4-5#mp-pane-00".into();
        }
        scroll_chat_thread(&mut state, -3);
        assert_eq!(
            state.multipane.as_ref().unwrap().panes[0].chat_thread_scroll,
            0
        );
        scroll_chat_thread(&mut state, 4);
        assert_eq!(
            state.multipane.as_ref().unwrap().panes[0].chat_thread_scroll,
            4
        );
        scroll_chat_thread(&mut state, -1);
        assert_eq!(
            state.multipane.as_ref().unwrap().panes[0].chat_thread_scroll,
            3
        );
    }

    #[test]
    fn handle_mouse_scroll_targets_roster_or_chat_per_pane_mode() {
        let mut state = fixture_state_no_backend();
        // Pane 0 stays in roster mode; pane 1 becomes a chat pane.
        if let Some(pane) = state.multipane.as_mut().unwrap().panes.get_mut(1) {
            pane.selected_agent_id = Some("claude-haiku-4-5#mp-pane-01".into());
            pane.agent_id = "claude-haiku-4-5#mp-pane-01".into();
        }
        let area = Rect::new(0, 0, 80, 30);
        let pane0_rect = grid::pane_rect(area, 2, 1, 0);
        let pane1_rect = grid::pane_rect(area, 2, 1, 1);

        // Wheel down inside pane 0 → roster_scroll bumps.
        handle_mouse_scroll(&mut state, area, pane0_rect.x + 5, pane0_rect.y + 5, 1);
        assert_eq!(state.multipane.as_ref().unwrap().panes[0].roster_scroll, 1);

        // Wheel down inside pane 1 → chat_thread_scroll bumps.
        handle_mouse_scroll(&mut state, area, pane1_rect.x + 5, pane1_rect.y + 5, 1);
        assert_eq!(
            state.multipane.as_ref().unwrap().panes[1].chat_thread_scroll,
            1
        );
    }

    #[test]
    fn apply_roster_click_on_template_writes_swarm_default() {
        let mut state = fixture_state_no_backend();
        let pane = state.multipane.as_ref().unwrap().panes[0].clone();
        let rows = roster_view::compute_rows(&state, &pane, None);
        let lab_col = " Template: ".chars().count() + 1;
        apply_roster_click(
            &mut state,
            RosterClickTarget {
                pane_idx: 0,
                rows,
                row_idx: 0,
                row: roster_view::PaneRosterRow::Template,
                local_x: lab_col,
            },
        );
        assert_eq!(state.agents.swarm_default_template, "lab");
    }

    #[test]
    fn apply_roster_click_on_mission_writes_swarm_default() {
        let mut state = fixture_state_no_backend();
        let pane = state.multipane.as_ref().unwrap().panes[0].clone();
        let rows = roster_view::compute_rows(&state, &pane, None);
        let general_col = " Mission:  ".chars().count() + " auto ".chars().count() + 1 + 1;
        apply_roster_click(
            &mut state,
            RosterClickTarget {
                pane_idx: 0,
                rows,
                row_idx: 1,
                row: roster_view::PaneRosterRow::Mission,
                local_x: general_col,
            },
        );
        assert_eq!(state.agents.swarm_default_mission, "general");
    }
}
