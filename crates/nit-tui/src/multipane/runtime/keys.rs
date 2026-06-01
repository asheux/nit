use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::{AgentChannel, AgentMessage, AppState};
use ratatui::layout::Rect;

use crate::swarm::SYSTEM_ALERT_KIND;

use crate::app::{
    chat_history_next, chat_history_prev, clear_chat_esc_state, handle_abort,
    handle_chat_input_editing_key, is_global_quit_key, lane_has_in_flight_turn,
    record_chat_esc_press, AbortScope,
};
use crate::claude_runner::ClaudeRunner;
use crate::codex_runner::CodexRunner;
use crate::multipane::dir_search::{self, ParsedQuery};
use crate::multipane::dir_search_runner::DirSearchRunner;
use crate::multipane::dispatch_focused::{submit_focused_pane_input, with_focused_pane_aliased};
use crate::multipane::focus;
use crate::multipane::roster_view;
use crate::multipane::scroll::{scroll_chat_thread, CHAT_THREAD_PAGE_STEP};
use crate::shadow::ShadowRuntime;
use crate::swarm::SwarmRuntime;
use crate::vitals::VitalsState;

const ROSTER_PAGE_STEP: i32 = 8;

/// Returns `true` to exit the loop.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_key(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
    dir_runner: &DirSearchRunner,
    key: KeyEvent,
    clipboard: &mut Option<arboard::Clipboard>,
    area: Rect,
) -> bool {
    if let Some(exit) = consume_global_chrome_keys(state, &key) {
        return exit;
    }

    if !matches!(key.code, KeyCode::Esc) {
        clear_chat_esc_state();
    }

    if try_toggle_terminal(state, &key) {
        return false;
    }

    if try_toggle_terminal_popup(state, &key) {
        return false;
    }

    if try_cycle_focus(state, &key) {
        return false;
    }

    if focused_pane_dir_search_active(state) {
        return handle_dir_search_key(state, dir_runner, key, codex, claude, swarm, shadow);
    }

    if focused_pane_in_roster_mode(state) {
        return handle_roster_key(state, codex, claude, swarm, key);
    }
    handle_chat_key(
        state, vitals, codex, claude, swarm, shadow, dir_runner, key, clipboard, area,
    )
}

/// Resolve the chrome short-circuits — artifacts popup, help overlay
/// toggle, Ctrl+Q. Returns:
/// - `Some(true)`  → exit the run loop (Ctrl+Q).
/// - `Some(false)` → key consumed by a chrome short-circuit.
/// - `None`        → key is for the active pane; continue dispatch.
fn consume_global_chrome_keys(state: &mut AppState, key: &KeyEvent) -> Option<bool> {
    if state.agents.artifacts_popup_open && matches!(key.code, KeyCode::Esc) {
        state.agents.artifacts_popup_open = false;
        clear_chat_esc_state();
        return Some(false);
    }

    if matches!(state.multipane.as_ref(), Some(mp) if mp.help_open) {
        let close = matches!(key.code, KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('?'));
        if close {
            set_help_open(state, false);
            clear_chat_esc_state();
        }
        return Some(false);
    }

    if is_global_quit_key(key) {
        return Some(true);
    }

    let chord = key.modifiers;
    let is_unmodified_question_mark = matches!(key.code, KeyCode::Char('?'))
        && !chord.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let opens_help = matches!(key.code, KeyCode::F(1))
        || (is_unmodified_question_mark && focused_chat_input_is_empty(state));
    if opens_help {
        set_help_open(state, true);
        return Some(false);
    }
    None
}

fn set_help_open(state: &mut AppState, open: bool) {
    if let Some(mp) = state.multipane.as_mut() {
        mp.help_open = open;
    }
}

/// Tab / Shift+Tab / BackTab cycle pane focus regardless of mode and
/// never move the per-pane roster cursor. Closing dir-search on tab is
/// the safe default — operator can re-open it in the new pane.
fn try_cycle_focus(state: &mut AppState, key: &KeyEvent) -> bool {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let forward = match key.code {
        KeyCode::Tab => !shift,
        KeyCode::BackTab => false,
        _ => return false,
    };
    close_focused_dir_search(state);
    if let Some(mp) = state.multipane.as_mut() {
        if forward {
            focus::cycle_forward(mp);
        } else {
            focus::cycle_backward(mp);
        }
    }
    true
}

/// `Ctrl+\` flips the focused pane between chat and terminal. The event loop
/// reconciles this flag into a PtySession; while the terminal is live it
/// intercepts keystrokes before this handler runs, so the toggle-off reaches
/// here by falling through the forwarder.
fn try_toggle_terminal(state: &mut AppState, key: &KeyEvent) -> bool {
    if !crate::pty::is_terminal_toggle_key(key) {
        return false;
    }
    let idx = focused_pane_idx(state);
    if let Some(mp) = state.multipane.as_mut() {
        if let Some(pane) = mp.panes.get_mut(idx) {
            pane.terminal_active = !pane.terminal_active;
        }
    }
    true
}

/// `Ctrl+Shift+T` toggles the one-per-process modal terminal popup over the
/// whole grid (not per-pane). The event loop pins the focused pane's cwd and
/// reconciles the persistent PtySession.
fn try_toggle_terminal_popup(state: &mut AppState, key: &KeyEvent) -> bool {
    if !crate::app::popup_keys::is_terminal_popup_toggle_key(key) {
        return false;
    }
    state.terminal_popup.toggle_requested = true;
    true
}

fn handle_roster_key(
    state: &mut AppState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    key: KeyEvent,
) -> bool {
    let modifiers = key.modifiers;
    let is_ctrl = modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Char('c') if is_ctrl => {
            push_pane_system_message(state, "no agent selected — nothing to abort".into());
            false
        }
        KeyCode::Esc => {
            if record_chat_esc_press() {
                push_pane_system_message(state, "no agent selected — nothing to abort".into());
                clear_chat_esc_state();
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
            commit_roster_selection(state, codex, claude, swarm);
            false
        }
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_chat_key(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
    dir_runner: &DirSearchRunner,
    key: KeyEvent,
    clipboard: &mut Option<arboard::Clipboard>,
    area: Rect,
) -> bool {
    let modifiers = key.modifiers;
    let is_ctrl = modifiers.contains(KeyModifiers::CONTROL);
    let is_super = modifiers.contains(KeyModifiers::SUPER);

    match key.code {
        KeyCode::Char('/') if is_ctrl => {
            toggle_focused_pane_dir_search(state, dir_runner);
            false
        }
        KeyCode::F(2) => {
            // F2 fallback: many terminals (default macOS Terminal.app)
            // do not deliver Ctrl+/. Same payload either way.
            toggle_focused_pane_dir_search(state, dir_runner);
            false
        }
        KeyCode::Char('r') if is_ctrl => {
            revert_focused_pane_to_roster(state);
            false
        }
        KeyCode::Char('c') if is_super => {
            // Best-effort macOS Cmd+C: only fires on terminals that
            // forward SUPER (Kitty / WezTerm / iTerm with CSI-u).
            // Default Terminal.app does not deliver this. If a
            // chat-thread selection was consumed, we're done; otherwise
            // fall through to the canonical input editor so input-box
            // selections still copy.
            if !super::mouse::try_copy_focused_pane_selection(state, clipboard, area) {
                with_focused_pane_aliased(state, |state| {
                    let _ = handle_chat_input_editing_key(&key, state, clipboard);
                });
            }
            false
        }
        KeyCode::Char('c') if is_ctrl => {
            if super::mouse::try_copy_focused_pane_selection(state, clipboard, area) {
                return false;
            }
            // Empty input: Ctrl+C is the abort sentinel for the
            // focused pane. Non-empty input: defer to the canonical
            // handler, which copies an active input selection or
            // clears the input — same behavior as single-pane.
            if focused_chat_input_is_empty(state) {
                abort_focused_pane(state, codex, claude, swarm, shadow);
            } else {
                with_focused_pane_aliased(state, |state| {
                    let _ = handle_chat_input_editing_key(&key, state, clipboard);
                });
            }
            false
        }
        KeyCode::Esc => {
            if record_chat_esc_press() {
                abort_focused_pane(state, codex, claude, swarm, shadow);
                clear_chat_esc_state();
            }
            false
        }
        KeyCode::Enter => {
            submit_focused_pane_input(state, vitals, codex, claude, swarm, shadow);
            false
        }
        KeyCode::Up if !modifiers.contains(KeyModifiers::SHIFT) => {
            with_focused_pane_aliased(state, |state| {
                let _ = chat_history_prev(state);
            });
            false
        }
        KeyCode::Down if !modifiers.contains(KeyModifiers::SHIFT) => {
            with_focused_pane_aliased(state, |state| {
                let _ = chat_history_next(state);
            });
            false
        }
        KeyCode::PageUp => {
            scroll_chat_thread(state, swarm, area, -CHAT_THREAD_PAGE_STEP);
            false
        }
        KeyCode::PageDown => {
            scroll_chat_thread(state, swarm, area, CHAT_THREAD_PAGE_STEP);
            false
        }
        _ => {
            with_focused_pane_aliased(state, |state| {
                let _ = handle_chat_input_editing_key(&key, state, clipboard);
            });
            false
        }
    }
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

pub(super) fn move_roster_cursor(state: &mut AppState, delta: i32) {
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
        roster_view::sync_auto_expansion(pane, row.as_ref());
    }
}

pub(super) fn jump_roster_cursor_to_top(state: &mut AppState) {
    let (_, rows) = focused_pane_rows(state);
    let row = roster_view::row_at_cursor(&rows, 0).cloned();
    if let Some(pane) = focused_pane_mut(state) {
        pane.roster_cursor = 0;
        pane.roster_scroll = 0;
        roster_view::sync_tree_selection(pane, row.as_ref());
        roster_view::sync_auto_expansion(pane, row.as_ref());
    }
}

pub(super) fn jump_roster_cursor_to_bottom(state: &mut AppState) {
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
        roster_view::sync_auto_expansion(pane, row.as_ref());
    }
}

pub(super) fn collapse_at_cursor(state: &mut AppState) {
    let Some(row) = row_under_focused_cursor(state) else {
        return;
    };
    match row {
        roster_view::PaneRosterRow::Backend { .. } => {
            // Drilling the cursor up off a Backend row clears
            // auto_expanded_backend through sync_auto_expansion, which
            // is the only source of "is this backend visible?".
            move_roster_cursor(state, -1);
        }
        roster_view::PaneRosterRow::Agent { agent_id, .. } => {
            if let Some(pane) = focused_pane_mut(state) {
                pane.roster_collapsed_agent_ids.insert(agent_id);
            }
        }
        _ => {}
    }
    clamp_focused_roster_cursor(state);
}

pub(super) fn expand_at_cursor(state: &mut AppState) {
    let Some(row) = row_under_focused_cursor(state) else {
        return;
    };
    match row {
        roster_view::PaneRosterRow::Backend { .. } => {
            // Drilling cursor down to the first child auto-expands the
            // parent backend through sync_auto_expansion.
            move_roster_cursor(state, 1);
        }
        roster_view::PaneRosterRow::Agent { agent_id, .. } => {
            if let Some(pane) = focused_pane_mut(state) {
                pane.roster_collapsed_agent_ids.remove(&agent_id);
            }
        }
        _ => {}
    }
}

fn clamp_focused_roster_cursor(state: &mut AppState) {
    let (_, rows) = focused_pane_rows(state);
    let stops = roster_view::selectable_count(&rows);
    if let Some(pane) = focused_pane_mut(state) {
        if stops == 0 {
            pane.roster_cursor = 0;
        } else if pane.roster_cursor >= stops {
            pane.roster_cursor = stops - 1;
        }
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
    let pane_idx = state.multipane.as_ref().map(|mp| mp.focused).unwrap_or(0);
    roster_view::toggle_size_leaf(state, pane_idx, &agent_id, leaf_idx);
}

fn commit_roster_selection(
    state: &mut AppState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
) {
    let _ = (codex, claude, swarm);
    let (pane_idx, rows) = focused_pane_rows(state);
    let cursor = focused_pane_mut(state)
        .map(|p| p.roster_cursor)
        .unwrap_or(0);
    let Some(row) = roster_view::row_at_cursor(&rows, cursor).cloned() else {
        push_pane_system_message(state, "no agents available to select".into());
        return;
    };
    let pane_idx = pane_idx.unwrap_or(0);
    super::mouse::dispatch_commit(state, pane_idx, row);
}

pub(super) fn revert_focused_pane_to_roster(state: &mut AppState) {
    if let Some(pane) = focused_pane_mut(state) {
        let pane_idx = pane.pane_id;
        pane.selected_agent_id = None;
        pane.agent_id.clear();
        pane.chat_input.clear();
        pane.chat_input_cursor = 0;
        pane.chat_input_selection_anchor = None;
        pane.chat_input_scroll = 0;
        pane.mission_id = None;
        pane.mission_ids.clear();
        // Re-derive the synthetic chat id so subsequent default-chat
        // dispatches (after re-committing an agent) still tag with a
        // stable per-pane id.
        pane.chat_mission_id = crate::multipane::agent_id::pane_chat_mission_id(pane_idx);
        // Clear staleness from the cursor-driven latches and the dir
        // search overlay so a re-entered roster does not flash stale
        // state.
        pane.auto_expanded_backend = None;
        pane.auto_expanded_agent = None;
        pane.dir_search = None;
    }
}

pub(in crate::multipane) fn focused_pane_mut(
    state: &mut AppState,
) -> Option<&mut nit_core::PaneSession> {
    let mp = state.multipane.as_mut()?;
    let idx = mp.focused;
    mp.panes.get_mut(idx)
}

pub(in crate::multipane) fn focused_pane_idx(state: &AppState) -> usize {
    state.multipane.as_ref().map(|mp| mp.focused).unwrap_or(0)
}

pub(super) fn focused_pane_in_roster_mode(state: &AppState) -> bool {
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

pub(in crate::multipane) fn clear_focused_pane_input(state: &mut AppState) {
    if let Some(pane) = focused_pane_mut(state) {
        pane.chat_input.clear();
        pane.chat_input_cursor = 0;
        pane.chat_input_selection_anchor = None;
        pane.chat_input_scroll = 0;
    }
}

pub(in crate::multipane) fn push_pane_system_message(state: &mut AppState, text: String) {
    push_focused_pane_message(state, text, "multipane-system");
}

fn push_pane_system_alert(state: &mut AppState, text: String) {
    push_focused_pane_message(state, text, SYSTEM_ALERT_KIND);
}

fn push_focused_pane_message(state: &mut AppState, text: String, kind: &str) {
    let Some(pane) = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .cloned()
    else {
        return;
    };
    let agent_id = if pane.agent_id.is_empty() {
        pane.selected_agent_id.clone()
    } else {
        Some(pane.agent_id.clone())
    };
    let at = format!("t+{}", state.metrics.frame_count);
    state.agents.messages.push(AgentMessage {
        at,
        channel: AgentChannel::Agent,
        agent_id,
        mission_id: pane.mission_id.clone(),
        text,
        prompt_msg_idx: None,
        kind: Some(kind.into()),
    });
}

/// Multipane abort. Routes through the canonical
/// `chat_input::handle_abort` so swarm missions roll over to
/// `completed_runs` with `report_status="ABORTED"`, queues drain via
/// `release_queued_slot`, and the system alert lands as a
/// `SYSTEM_ALERT_KIND` message — same semantics as the standard chat.
///
/// The pane's `mission_id` is aliased into
/// `state.agents.selected_mission` so `AbortScope::Current` resolves
/// to the right mission. When a pane has no real swarm mission (only
/// the synthetic chat id), routes to a surgical per-agent
/// `CancelTurn` instead so the swarm-wide fallback in `handle_abort`
/// cannot reach into another pane's mission.
pub(super) fn abort_focused_pane(
    state: &mut AppState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
) {
    let Some(focused_agent) = focused_pane_agent_id(state) else {
        push_pane_system_message(state, "no agent selected — nothing to abort".into());
        return;
    };
    // Inspect mission scope BEFORE entering with_focused_pane_aliased
    // because synthetic-id-only state must route to AbortScope::Agent
    // (per-agent CancelTurn), never AbortScope::Current.
    let pane = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .cloned();
    let Some(pane) = pane else {
        return;
    };
    let real_mission = pane
        .mission_id
        .as_deref()
        .filter(|m| !crate::multipane::agent_id::is_pane_chat_mission_id(m))
        .map(|s| s.to_string())
        .or_else(|| {
            state
                .agents
                .agents_get(&focused_agent)
                .and_then(|lane| lane.current_mission.clone())
                .filter(|m| !crate::multipane::agent_id::is_pane_chat_mission_id(m))
        });
    let swarm_active = real_mission
        .as_deref()
        .is_some_and(|mid| swarm.is_active_mission(mid));
    if swarm_active {
        // Alias places this pane's mission into selected_mission so
        // AbortScope::Current resolves to exactly this pane's swarm —
        // no cross-pane fallback.
        with_focused_pane_aliased(state, |state| {
            handle_abort(state, Some(codex), Some(claude), swarm, AbortScope::Current);
        });
        return;
    }
    // Single-agent shadow mode: a `@shadow` prompt (or auto-shadow on
    // a heavy single-agent prompt) spins up hidden propose-a /
    // propose-b / judge / review lanes. While they run, the *base*
    // lane is idle, so the lane-in-flight check below is false even
    // though the operator clearly sees activity in the breather.
    // Detect the shadow run first and tear it down before falling
    // through, otherwise `/abort` posts "no active mission for this
    // pane" while propose / judge keep burning tokens.
    if shadow.has_run_for(&focused_agent) {
        let shadow_lanes = shadow.abort_run(state, &focused_agent);
        // CancelTurn for each shadow lane — `cleanup_shadow_lanes`
        // (called inside `abort_run`) only purges in-process
        // bookkeeping; the runner subprocesses are still alive and
        // would otherwise keep streaming until they hit their idle
        // reaper.
        for lane_id in &shadow_lanes {
            let _ = codex.send(crate::codex_runner::CodexCommand::CancelTurn {
                agent_id: lane_id.clone(),
            });
            let _ = claude.send(crate::claude_runner::ClaudeCommand::CancelTurn {
                agent_id: lane_id.clone(),
            });
        }
        // Drain any queued main-agent turn the shadow pipeline was
        // about to dispatch once review finished.
        crate::swarm::drain_queued_turns_for_agent_pub(state, &focused_agent);
        push_pane_system_message(
            state,
            format!("aborted shadow run ({} lanes)", shadow_lanes.len()),
        );
        return;
    }
    // Stale mission id, or never had a real swarm overlay. Surgically
    // cancel the focused pane's lane via AbortScope::Agent if a turn
    // is live; otherwise post a "nothing to abort" system message.
    if !lane_has_in_flight_turn(state, &focused_agent) {
        push_pane_system_message(state, "no active mission for this pane".into());
        return;
    }
    with_focused_pane_aliased(state, |state| {
        handle_abort(
            state,
            Some(codex),
            Some(claude),
            swarm,
            AbortScope::Agent(focused_agent.clone()),
        );
    });
}

pub(super) fn focused_pane_dir_search_active(state: &AppState) -> bool {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .map(|p| p.dir_search.is_some())
        .unwrap_or(false)
}

pub(super) fn close_focused_dir_search(state: &mut AppState) {
    if let Some(pane) = focused_pane_mut(state) {
        pane.dir_search = None;
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn toggle_focused_pane_dir_search(state: &mut AppState, runner: &DirSearchRunner) {
    let gitignored = state.gitignored_dirs.clone();
    let Some(pane) = focused_pane_mut(state) else {
        return;
    };
    if pane.dir_search.take().is_some() {
        return;
    }
    let cwd = pane.cwd.clone();
    let parsed = dir_search::parse_query("", &cwd, home_dir().as_deref());
    let id = runner.query(parsed.base.clone(), parsed.needle, false, gitignored);
    pane.dir_search = Some(nit_core::DirSearchState {
        base: parsed.base,
        generation: id,
        ..Default::default()
    });
}

fn issue_dir_search_query(state: &mut AppState, runner: &DirSearchRunner) {
    let gitignored = state.gitignored_dirs.clone();
    let Some(pane) = focused_pane_mut(state) else {
        return;
    };
    let Some(ds) = pane.dir_search.as_mut() else {
        return;
    };
    let ParsedQuery { base, needle } =
        dir_search::parse_query(&ds.query, &pane.cwd, home_dir().as_deref());
    if base != ds.base {
        ds.expanded.clear();
    }
    let expanded = ds.expanded.clone();
    let id = runner.query_with_expanded(base.clone(), needle, ds.show_hidden, gitignored, expanded);
    ds.base = base;
    ds.generation = id;
    ds.results.clear();
    ds.selected = 0;
    ds.view_offset = 0;
}

fn handle_dir_search_key(
    state: &mut AppState,
    runner: &DirSearchRunner,
    key: KeyEvent,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
) -> bool {
    let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => handle_dir_search_esc(state, codex, claude, swarm, shadow),
        KeyCode::Enter => commit_dir_search(state),
        KeyCode::Up => with_focused_dir_search(state, move_selected_up),
        KeyCode::Down => with_focused_dir_search(state, move_selected_down),
        // IMPORTANT: Ctrl+chord arms must precede the catch-all
        // KeyCode::Char(ch) below — otherwise the char inserts into
        // the query and the chord silently fails.
        KeyCode::Char('j') if is_ctrl => with_focused_dir_search(state, move_selected_down),
        KeyCode::Char('k') if is_ctrl => with_focused_dir_search(state, move_selected_up),
        KeyCode::Right => {
            expand_dir_search_at_cursor(state);
            issue_dir_search_query(state, runner);
        }
        KeyCode::Char('l') if is_ctrl => {
            expand_dir_search_at_cursor(state);
            issue_dir_search_query(state, runner);
        }
        KeyCode::Left => {
            collapse_dir_search_at_cursor(state);
            issue_dir_search_query(state, runner);
        }
        KeyCode::Char('h') if is_ctrl => {
            collapse_dir_search_at_cursor(state);
            issue_dir_search_query(state, runner);
        }
        KeyCode::Home => with_focused_dir_search(state, |ds| ds.query_cursor = 0),
        KeyCode::End => with_focused_dir_search(state, |ds| {
            ds.query_cursor = ds.query.chars().count();
        }),
        KeyCode::Backspace => {
            with_focused_dir_search(state, mutate_backspace);
            issue_dir_search_query(state, runner);
        }
        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::ALT) => {
            with_focused_dir_search(state, |ds| ds.show_hidden = !ds.show_hidden);
            issue_dir_search_query(state, runner);
        }
        KeyCode::Char(ch) => {
            with_focused_dir_search(state, |ds| insert_query_char(ds, ch));
            issue_dir_search_query(state, runner);
        }
        _ => {}
    }
    false
}

pub(super) fn move_selected_up(ds: &mut nit_core::DirSearchState) {
    if ds.results.is_empty() {
        return;
    }
    ds.selected = ds.selected.saturating_sub(1);
    super::render::clamp_viewport(ds, ds.last_visible as usize);
}

pub(super) fn move_selected_down(ds: &mut nit_core::DirSearchState) {
    if ds.results.is_empty() {
        return;
    }
    let max = ds.results.len() - 1;
    ds.selected = (ds.selected + 1).min(max);
    super::render::clamp_viewport(ds, ds.last_visible as usize);
}

pub(super) fn expand_dir_search_at_cursor(state: &mut AppState) {
    with_focused_dir_search(state, |ds| {
        if let Some(path) = ds.results.get(ds.selected).cloned() {
            if path.is_dir() {
                ds.expanded.insert(path);
            }
        }
    });
}

pub(super) fn collapse_dir_search_at_cursor(state: &mut AppState) {
    with_focused_dir_search(state, |ds| {
        let Some(path) = ds.results.get(ds.selected).cloned() else {
            return;
        };
        if ds.expanded.remove(&path) {
            return;
        }
        let mut current: Option<&Path> = path.parent();
        while let Some(p) = current {
            if ds.expanded.remove(p) {
                return;
            }
            current = p.parent();
        }
    });
}

pub(super) fn with_focused_dir_search<F: FnOnce(&mut nit_core::DirSearchState)>(
    state: &mut AppState,
    f: F,
) {
    let Some(pane) = focused_pane_mut(state) else {
        return;
    };
    let Some(ds) = pane.dir_search.as_mut() else {
        return;
    };
    f(ds);
}

fn mutate_backspace(ds: &mut nit_core::DirSearchState) {
    if ds.query_cursor == 0 {
        return;
    }
    let drop_at = ds.query_cursor - 1;
    ds.query = ds
        .query
        .chars()
        .enumerate()
        .filter_map(|(i, c)| (i != drop_at).then_some(c))
        .collect();
    ds.query_cursor = drop_at;
}

fn insert_query_char(ds: &mut nit_core::DirSearchState, ch: char) {
    let mut chars: Vec<char> = ds.query.chars().collect();
    let at = ds.query_cursor.min(chars.len());
    chars.insert(at, ch);
    ds.query = chars.into_iter().collect();
    ds.query_cursor = at + 1;
}

fn handle_dir_search_esc(
    state: &mut AppState,
    codex: &CodexRunner,
    claude: &ClaudeRunner,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
) {
    // Esc closes the overlay and feeds the shared esc-press latch so
    // a second Esc within the abort window aborts the focused pane
    // (consistent with the chat-mode Esc handler).
    let double_tap = record_chat_esc_press();
    close_focused_dir_search(state);
    if !double_tap {
        return;
    }
    if focused_pane_in_roster_mode(state) {
        push_pane_system_message(state, "no agent selected — nothing to abort".into());
    } else {
        abort_focused_pane(state, codex, claude, swarm, shadow);
    }
    clear_chat_esc_state();
}

pub(super) fn commit_dir_search(state: &mut AppState) {
    let chosen = take_dir_search_choice(state);
    let Some(path) = chosen else { return };
    if let Some(pane) = focused_pane_mut(state) {
        pane.cwd = path.clone();
    }
    invalidate_focused_pane_resume_sessions(state);
    push_pane_system_alert(state, format!("cwd → {}", path.display()));
}

/// Drop the focused pane's resume ids so a fresh session is created
/// in the new cwd. Otherwise CLI session metadata re-anchors the
/// spawn cwd to the original workspace silently.
fn invalidate_focused_pane_resume_sessions(state: &mut AppState) {
    let Some(agent_id) = focused_pane_agent_id(state) else {
        return;
    };
    let mission_id = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(mp.focused))
        .and_then(|p| p.mission_id.clone());
    state.agents.codex_thread_ids.remove(&agent_id);
    state.agents.claude_session_ids.remove(&agent_id);
    if let Some(mid) = mission_id.as_deref() {
        if let Some(threads) = state.agents.codex_mission_thread_ids.get_mut(mid) {
            threads.remove(&agent_id);
        }
        if let Some(sessions) = state.agents.claude_mission_session_ids.get_mut(mid) {
            sessions.remove(&agent_id);
        }
    }
}

fn take_dir_search_choice(state: &mut AppState) -> Option<PathBuf> {
    let pane = focused_pane_mut(state)?;
    let candidate = pane
        .dir_search
        .as_ref()
        .and_then(|ds| ds.results.get(ds.selected).cloned());
    pane.dir_search = None;
    let path = candidate?;
    // Race-guard: if the filesystem mutated between walk and Enter,
    // refuse to switch into a path that is no longer a directory.
    path.is_dir().then_some(path)
}

pub(in crate::multipane) fn focused_pane_agent_id(state: &AppState) -> Option<String> {
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
