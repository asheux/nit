use arboard::Clipboard;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::{
    AgentAlertSeverity, AgentChannel, AgentDiagnosticEvent, AgentMessage, AppState, MissionPhase,
    CONSOLE_SCROLL_BOTTOM,
};

use crate::claude_runner::ClaudeRunner;
use crate::codex_runner::{CodexRunner, CodexRunnerConfig};
use crate::swarm::{
    create_chat_clone, detect_swarm_mission_kind_from_prompt,
    explicit_swarm_mission_kind_from_prompt, is_agent_busy, is_agent_family_busy,
    is_chat_clone_agent_id, parse_swarm_command, parse_swarm_mission_kind, select_swarm_agents,
    SwarmCommand, SwarmMissionKind, SwarmRuntime, SwarmSize,
};
use crate::vitals::VitalsState;

use super::dispatch::{
    apply_swarm_task_role, dispatch_agent_prompt, enqueue_claude_turn, enqueue_codex_turn,
    estimate_codex_context_tokens, maybe_dispatch_claude_turn, maybe_dispatch_codex_turn,
    maybe_dispatch_next_queued_claude_turn, maybe_dispatch_next_queued_codex_turn,
};
use super::{
    chat_current_line_indent, chat_cursor_move_word_left, chat_cursor_move_word_right,
    copy_chat_input_selection, delete_chat_input_selection, insert_chat_input_text,
    mark_mission_provenance_dirty, timestamp_label,
};

const CHAT_PROMPT_HISTORY_MAX: usize = 200;

// ---------------------------------------------------------------------------
// ChatInputEditResult
// ---------------------------------------------------------------------------

/// Result from `handle_chat_input_editing_key` indicating what happened.
pub(super) struct ChatInputEditResult {
    pub(super) handled: bool,
    pub(super) changed: bool,
    pub(super) follow_cursor: bool,
}

// ---------------------------------------------------------------------------
// handle_chat_input_editing_key
// ---------------------------------------------------------------------------

/// Reusable text-editing key handler for the chat input box.
/// Handles all text manipulation keys (characters, backspace, delete, cursor movement,
/// selection, copy/paste, etc.) but NOT Enter-submit, Esc, or Up/Down arrow keys.
/// Those are left to the caller so each context can provide its own behavior.
pub(super) fn handle_chat_input_editing_key(
    key: &KeyEvent,
    state: &mut AppState,
    clipboard: &mut Option<Clipboard>,
) -> ChatInputEditResult {
    let mut changed = false;
    let mut handled = false;
    let mut follow_cursor = false;

    let input_len_chars = state.agents.chat_input.chars().count();
    if state.agents.chat_input_cursor > input_len_chars {
        state.agents.chat_input_cursor = input_len_chars;
    }
    if state
        .agents
        .chat_input_selection_anchor
        .is_some_and(|anchor| anchor > input_len_chars)
    {
        state.agents.chat_input_selection_anchor = Some(input_len_chars);
    }

    match *key {
        KeyEvent {
            code: KeyCode::Enter,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SHIFT) => {
            handled = true;
            delete_chat_input_selection(state);
            let indent =
                chat_current_line_indent(&state.agents.chat_input, state.agents.chat_input_cursor);
            let insert = if indent.is_empty() {
                "\n".to_string()
            } else {
                format!("\n{indent}")
            };
            let insert_at =
                chat_input_byte_index(&state.agents.chat_input, state.agents.chat_input_cursor);
            state.agents.chat_input.insert_str(insert_at, &insert);
            state.agents.chat_input_cursor = state
                .agents
                .chat_input_cursor
                .saturating_add(insert.chars().count());
            state.agents.chat_input_selection_anchor = None;
            changed = true;
            follow_cursor = true;
        }
        KeyEvent {
            code: KeyCode::Char('a'),
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) => {
            handled = true;
            let total_chars = state.agents.chat_input.chars().count();
            state.agents.chat_input_selection_anchor = Some(0);
            state.agents.chat_input_cursor = total_chars;
            copy_chat_input_selection(state, clipboard);
            changed = true;
            follow_cursor = true;
        }
        KeyEvent {
            code: KeyCode::Char('c'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SUPER)
            || (modifiers.contains(KeyModifiers::CONTROL)
                && modifiers.contains(KeyModifiers::SHIFT)) =>
        {
            handled = true;
            copy_chat_input_selection(state, clipboard);
        }
        KeyEvent {
            code: KeyCode::Char('x'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SUPER)
            || (modifiers.contains(KeyModifiers::CONTROL)
                && modifiers.contains(KeyModifiers::SHIFT)) =>
        {
            handled = true;
            if copy_chat_input_selection(state, clipboard) && delete_chat_input_selection(state) {
                changed = true;
                follow_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('v'),
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) => {
            handled = true;
            if let Some(cb) = clipboard.as_mut() {
                if let Ok(text) = cb.get_text() {
                    if insert_chat_input_text(state, &text) {
                        changed = true;
                        follow_cursor = true;
                    }
                }
            }
        }
        KeyEvent {
            code: KeyCode::Insert,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            handled = true;
            copy_chat_input_selection(state, clipboard);
        }
        KeyEvent {
            code: KeyCode::Insert,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SHIFT) => {
            handled = true;
            if let Some(cb) = clipboard.as_mut() {
                if let Ok(text) = cb.get_text() {
                    if insert_chat_input_text(state, &text) {
                        changed = true;
                        follow_cursor = true;
                    }
                }
            }
        }
        KeyEvent {
            code: KeyCode::Char('u'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            handled = true;
            if !state.agents.chat_input.is_empty() {
                state.agents.chat_input.clear();
                state.agents.chat_input_cursor = 0;
                state.agents.chat_input_selection_anchor = None;
                super::chat_history_reset_nav(state);
                changed = true;
                follow_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('c'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            handled = true;
            if copy_chat_input_selection(state, clipboard) {
                // Copied.
            } else if !state.agents.chat_input.is_empty() {
                state.agents.chat_input.clear();
                state.agents.chat_input_cursor = 0;
                state.agents.chat_input_selection_anchor = None;
                super::chat_history_reset_nav(state);
                changed = true;
                follow_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('\u{3}'),
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            handled = true;
            if copy_chat_input_selection(state, clipboard) {
                // Copied.
            } else if !state.agents.chat_input.is_empty() {
                state.agents.chat_input.clear();
                state.agents.chat_input_cursor = 0;
                state.agents.chat_input_selection_anchor = None;
                super::chat_history_reset_nav(state);
                changed = true;
                follow_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::Backspace,
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
            handled = true;
            if delete_chat_input_selection(state) {
                changed = true;
                follow_cursor = true;
            } else {
                let cursor = state.agents.chat_input_cursor;
                let remove_start = chat_cursor_move_word_left(&state.agents.chat_input, cursor);
                if remove_start < cursor {
                    let start = chat_input_byte_index(&state.agents.chat_input, remove_start);
                    let end = chat_input_byte_index(&state.agents.chat_input, cursor);
                    state.agents.chat_input.replace_range(start..end, "");
                    state.agents.chat_input_cursor = remove_start;
                    state.agents.chat_input_selection_anchor = None;
                    changed = true;
                    follow_cursor = true;
                }
            }
        }
        KeyEvent {
            code: KeyCode::Backspace,
            ..
        } => {
            handled = true;
            if delete_chat_input_selection(state) {
                changed = true;
                follow_cursor = true;
            } else if state.agents.chat_input_cursor > 0 {
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
                state.agents.chat_input_selection_anchor = None;
                changed = true;
                follow_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::Delete,
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
            handled = true;
            if delete_chat_input_selection(state) {
                changed = true;
                follow_cursor = true;
            } else {
                let cursor = state.agents.chat_input_cursor;
                let remove_end = chat_cursor_move_word_right(&state.agents.chat_input, cursor);
                if remove_end > cursor {
                    let start = chat_input_byte_index(&state.agents.chat_input, cursor);
                    let end = chat_input_byte_index(&state.agents.chat_input, remove_end);
                    state.agents.chat_input.replace_range(start..end, "");
                    state.agents.chat_input_selection_anchor = None;
                    changed = true;
                    follow_cursor = true;
                }
            }
        }
        KeyEvent {
            code: KeyCode::Delete,
            ..
        } => {
            handled = true;
            if delete_chat_input_selection(state) {
                changed = true;
                follow_cursor = true;
            } else if state.agents.chat_input_cursor < state.agents.chat_input.chars().count() {
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
                state.agents.chat_input_selection_anchor = None;
                changed = true;
                follow_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::Left,
            modifiers,
            ..
        } => {
            handled = true;
            let total_chars = state.agents.chat_input.chars().count();
            let cursor = state.agents.chat_input_cursor.min(total_chars);
            let selecting = modifiers.contains(KeyModifiers::SHIFT);
            if selecting {
                if state.agents.chat_input_selection_anchor.is_none() {
                    state.agents.chat_input_selection_anchor = Some(cursor);
                }
            } else {
                state.agents.chat_input_selection_anchor = None;
            }
            let new_cursor = if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                chat_cursor_move_word_left(&state.agents.chat_input, cursor)
            } else {
                cursor.saturating_sub(1)
            };
            if new_cursor != cursor {
                state.agents.chat_input_cursor = new_cursor;
                changed = true;
                follow_cursor = true;
            }
            if selecting {
                copy_chat_input_selection(state, clipboard);
            }
        }
        KeyEvent {
            code: KeyCode::Right,
            modifiers,
            ..
        } => {
            handled = true;
            let max = state.agents.chat_input.chars().count();
            let cursor = state.agents.chat_input_cursor.min(max);
            let selecting = modifiers.contains(KeyModifiers::SHIFT);
            if selecting {
                if state.agents.chat_input_selection_anchor.is_none() {
                    state.agents.chat_input_selection_anchor = Some(cursor);
                }
            } else {
                state.agents.chat_input_selection_anchor = None;
            }
            let new_cursor = if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                chat_cursor_move_word_right(&state.agents.chat_input, cursor)
            } else {
                cursor.saturating_add(1).min(max)
            };
            if new_cursor != cursor {
                state.agents.chat_input_cursor = new_cursor;
                changed = true;
                follow_cursor = true;
            }
            if selecting {
                copy_chat_input_selection(state, clipboard);
            }
        }
        KeyEvent {
            code: KeyCode::Home,
            modifiers,
            ..
        } => {
            handled = true;
            let total_chars = state.agents.chat_input.chars().count();
            let cursor = state.agents.chat_input_cursor.min(total_chars);
            let selecting = modifiers.contains(KeyModifiers::SHIFT);
            if selecting {
                if state.agents.chat_input_selection_anchor.is_none() {
                    state.agents.chat_input_selection_anchor = Some(cursor);
                }
            } else {
                state.agents.chat_input_selection_anchor = None;
            }
            let (line_start, _line_end) =
                super::chat_current_line_bounds(&state.agents.chat_input, cursor);
            let new_cursor = if modifiers.contains(KeyModifiers::CONTROL) {
                0
            } else {
                line_start
            };
            if new_cursor != cursor {
                state.agents.chat_input_cursor = new_cursor;
                changed = true;
                follow_cursor = true;
            }
            if selecting {
                copy_chat_input_selection(state, clipboard);
            }
        }
        KeyEvent {
            code: KeyCode::End,
            modifiers,
            ..
        } => {
            handled = true;
            let max = state.agents.chat_input.chars().count();
            let cursor = state.agents.chat_input_cursor.min(max);
            let selecting = modifiers.contains(KeyModifiers::SHIFT);
            if selecting {
                if state.agents.chat_input_selection_anchor.is_none() {
                    state.agents.chat_input_selection_anchor = Some(cursor);
                }
            } else {
                state.agents.chat_input_selection_anchor = None;
            }
            let (_line_start, line_end) =
                super::chat_current_line_bounds(&state.agents.chat_input, cursor);
            let new_cursor = if modifiers.contains(KeyModifiers::CONTROL) {
                max
            } else {
                line_end
            };
            if new_cursor != cursor {
                state.agents.chat_input_cursor = new_cursor;
                changed = true;
                follow_cursor = true;
            }
            if selecting {
                copy_chat_input_selection(state, clipboard);
            }
        }
        KeyEvent {
            code: KeyCode::Tab,
            modifiers,
            ..
        } if modifiers.is_empty() => {
            handled = true;
            delete_chat_input_selection(state);
            let insert_at =
                chat_input_byte_index(&state.agents.chat_input, state.agents.chat_input_cursor);
            state.agents.chat_input.insert(insert_at, '\t');
            state.agents.chat_input_cursor = state.agents.chat_input_cursor.saturating_add(1);
            state.agents.chat_input_selection_anchor = None;
            changed = true;
            follow_cursor = true;
        }
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers,
            ..
        } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
            handled = true;
            delete_chat_input_selection(state);
            let insert_at =
                chat_input_byte_index(&state.agents.chat_input, state.agents.chat_input_cursor);
            state.agents.chat_input.insert(insert_at, c);
            state.agents.chat_input_cursor += 1;
            state.agents.chat_input_selection_anchor = None;
            changed = true;
            follow_cursor = true;
        }
        _ => {}
    }

    ChatInputEditResult {
        handled,
        changed,
        follow_cursor,
    }
}

// ---------------------------------------------------------------------------
// submit_chat_input_and_dispatch
// ---------------------------------------------------------------------------

/// Submit the chat input, dispatch to agents, and return whether a prompt was sent.
/// Shared between the main Agent Chat Enter handler and the Artifacts popup input.
pub(super) fn submit_chat_input_and_dispatch(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    swarm: &mut SwarmRuntime,
) -> bool {
    let raw = state.agents.chat_input.clone();
    if raw.trim().is_empty() {
        return false;
    }
    let mut swarm_handled = false;
    let mut cmd = parse_swarm_command(&raw)
        .map(|mut cmd| {
            if cmd.template.is_none() {
                cmd.template = Some(state.agents.swarm_default_template.clone());
            }
            cmd.mission_kind =
                effective_swarm_mission_kind(state, cmd.prompt.as_str(), cmd.mission_kind);
            cmd
        })
        .or_else(|| detect_implicit_swarm_command(state, &raw));
    if let Some(cmd) = cmd.as_mut() {
        let auto_switch_ops_dag = cmd
            .template
            .as_deref()
            .is_some_and(|t| matches!(t, "bulk" | "bo"));
        chat_history_remember(state, &raw);
        let planner = state
            .agents
            .selected_context_agent()
            .and_then(|id| {
                state
                    .agents
                    .agents
                    .iter()
                    .find(|lane| lane.id == id)
                    .is_some_and(|lane| lane.is_codex() || lane.is_claude())
                    .then_some(id.to_string())
            })
            .or_else(|| {
                state
                    .agents
                    .agents
                    .iter()
                    .find(|lane| lane.is_codex() || lane.is_claude())
                    .map(|lane| lane.id.clone())
            });

        if let Some(planner) = planner {
            let agents = select_swarm_agents(state, &planner, cmd.size, cmd.template.as_deref());
            if let Some((_mission_id, dispatches)) = swarm.start(
                state,
                planner.clone(),
                agents,
                cmd.size,
                cmd.template.clone(),
                cmd.mission_kind,
                cmd.prompt.clone(),
            ) {
                if auto_switch_ops_dag {
                    state.agents.dock_tab = nit_core::AgentOpsTab::Dag;
                }
                state.agents.chat_input = cmd.prompt.clone();
                state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
                let _ = push_chat_message(state);
                for dispatch in dispatches {
                    apply_swarm_task_role(state, &dispatch);
                    dispatch_agent_prompt(
                        state,
                        vitals,
                        codex,
                        claude,
                        dispatch.agent_id,
                        Some(dispatch.mission_id),
                        dispatch.prompt,
                    );
                }
                maybe_dispatch_next_queued_codex_turn(state, vitals, codex);
                maybe_dispatch_next_queued_claude_turn(state, vitals, claude);
                swarm_handled = true;
            } else {
                state.agents.chat_input = cmd.prompt.clone();
                state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
            }
        } else {
            state.status = Some("@swarm requires at least one Codex or Claude agent".into());
            state.agents.chat_input.clear();
            state.agents.chat_input_cursor = 0;
            swarm_handled = true;
        }
    }

    if !swarm_handled {
        // @new: spawn a clone with fresh context when the agent family is busy.
        let force_new = raw.starts_with("@new")
            && (raw.len() == 4 || raw[4..].starts_with(char::is_whitespace));
        // @q / @queue: legacy alias — now the same as the default (queue to base).
        let legacy_queue = (raw.starts_with("@queue")
            && (raw.len() == 6 || raw[6..].starts_with(char::is_whitespace)))
            || (raw.starts_with("@q")
                && (raw.len() == 2 || raw[2..].starts_with(char::is_whitespace)));

        if force_new {
            state.agents.chat_input = raw[4..].trim_start().to_string();
            state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
        } else if legacy_queue {
            let stripped = raw
                .strip_prefix("@queue")
                .unwrap_or_else(|| raw.strip_prefix("@q").unwrap_or(&raw));
            state.agents.chat_input = stripped.trim_start().to_string();
            state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
        }
        let mission_id = state
            .agents
            .selected_context_mission()
            .map(ToString::to_string);
        let selected_agent = state
            .agents
            .selected_context_agent()
            .map(ToString::to_string);
        let sent = push_chat_message(state);
        if let Some((channel, prompt)) = sent {
            // Index of the user prompt message just pushed — used to link agent
            // responses back to the correct prompt in the chat view.
            let prompt_msg_idx = state.agents.messages.len().saturating_sub(1);
            chat_history_remember(state, &raw);
            // For swarm missions, re-activate the run and dispatch only to
            // the planner so the swarm pipeline assigns roles to clones.
            let swarm_config = mission_id
                .as_deref()
                .and_then(|mid| swarm.session_config(mid));
            let is_swarm_mission = swarm_config.is_some();
            if is_swarm_mission {
                let config = swarm_config.as_ref().unwrap();
                let mid = mission_id.as_deref().unwrap();
                // Ensure clones exist in the roster.
                crate::swarm::ensure_swarm_agents_for_followup(state, mid, config);
                // Re-activate the completed run so handle_event_outcome
                // processes the planner's response as a new swarm plan.
                swarm.reactivate_for_followup(state, mid);
                // Update mission status.
                if let Some(mission) = state.agents.missions.iter_mut().find(|m| m.id == mid) {
                    mission.status = "PLAN".into();
                    mission.phase = MissionPhase::Plan;
                }
                // Wrap the user's prompt with planning instructions so the
                // planner generates a proper plan with role assignments.
                let planner = config.planner_agent_id.clone();
                let plan_prompt = swarm
                    .build_followup_planner_prompt(state, mid, &prompt)
                    .unwrap_or_else(|| prompt.clone());
                // Store prompt idx in the appropriate backend map.
                let planner_is_claude = state
                    .agents
                    .agents
                    .iter()
                    .find(|lane| lane.id == planner)
                    .is_some_and(|lane| lane.is_claude());
                if planner_is_claude {
                    state
                        .agents
                        .claude_turn_prompt_idx
                        .insert(planner.clone(), prompt_msg_idx);
                } else {
                    state
                        .agents
                        .codex_turn_prompt_idx
                        .insert(planner.clone(), prompt_msg_idx);
                }
                dispatch_agent_prompt(
                    state,
                    vitals,
                    codex,
                    claude,
                    planner,
                    mission_id.clone(),
                    plan_prompt,
                );
                maybe_dispatch_next_queued_codex_turn(state, vitals, codex);
                maybe_dispatch_next_queued_claude_turn(state, vitals, claude);
            }
            let targets = if is_swarm_mission {
                Vec::new() // already dispatched above
            } else if matches!(channel, AgentChannel::Broadcast) {
                broadcast_target_agents(state, mission_id.as_deref())
            } else {
                selected_agent.clone().into_iter().collect::<Vec<_>>()
            };
            for model in targets {
                let base_model = crate::swarm::resolve_base_agent_id(&model).to_string();
                let lane_kind = state
                    .agents
                    .agents
                    .iter()
                    .find(|lane| lane.id == base_model)
                    .map(|lane| lane.kind);
                let is_dispatchable = matches!(
                    lane_kind,
                    Some(nit_core::AgentLaneKind::Codex) | Some(nit_core::AgentLaneKind::Claude)
                );
                if !is_dispatchable {
                    continue;
                }
                let is_claude = lane_kind == Some(nit_core::AgentLaneKind::Claude);
                if force_new && is_agent_family_busy(state, &base_model) {
                    if let Some(clone_id) = create_chat_clone(state, &base_model) {
                        if is_claude {
                            state
                                .agents
                                .claude_turn_prompt_idx
                                .insert(clone_id.clone(), prompt_msg_idx);
                            maybe_dispatch_claude_turn(
                                state,
                                vitals,
                                claude,
                                Some(clone_id),
                                mission_id.clone(),
                                prompt.clone(),
                                true,
                            );
                        } else {
                            state
                                .agents
                                .codex_turn_prompt_idx
                                .insert(clone_id.clone(), prompt_msg_idx);
                            maybe_dispatch_codex_turn(
                                state,
                                vitals,
                                codex,
                                Some(clone_id),
                                mission_id.clone(),
                                prompt.clone(),
                                true,
                            );
                        }
                    } else if is_claude {
                        enqueue_claude_turn(
                            state,
                            vitals,
                            Some(base_model),
                            mission_id.clone(),
                            prompt.clone(),
                            Some(prompt_msg_idx),
                        );
                    } else {
                        enqueue_codex_turn(
                            state,
                            vitals,
                            Some(base_model),
                            mission_id.clone(),
                            prompt.clone(),
                            Some(prompt_msg_idx),
                        );
                    }
                } else if is_agent_busy(state, &base_model) {
                    if is_claude {
                        enqueue_claude_turn(
                            state,
                            vitals,
                            Some(base_model),
                            mission_id.clone(),
                            prompt.clone(),
                            Some(prompt_msg_idx),
                        );
                    } else {
                        enqueue_codex_turn(
                            state,
                            vitals,
                            Some(base_model),
                            mission_id.clone(),
                            prompt.clone(),
                            Some(prompt_msg_idx),
                        );
                    }
                } else if is_claude {
                    state
                        .agents
                        .claude_turn_prompt_idx
                        .insert(base_model.clone(), prompt_msg_idx);
                    maybe_dispatch_claude_turn(
                        state,
                        vitals,
                        claude,
                        Some(base_model),
                        mission_id.clone(),
                        prompt.clone(),
                        true,
                    );
                } else {
                    state
                        .agents
                        .codex_turn_prompt_idx
                        .insert(base_model.clone(), prompt_msg_idx);
                    maybe_dispatch_codex_turn(
                        state,
                        vitals,
                        codex,
                        Some(base_model),
                        mission_id.clone(),
                        prompt.clone(),
                        true,
                    );
                }
            }
            maybe_dispatch_next_queued_codex_turn(state, vitals, codex);
            maybe_dispatch_next_queued_claude_turn(state, vitals, claude);
        } else {
            return false;
        }
    }
    state.agents.chat_input_scroll = usize::MAX;
    true
}

// ---------------------------------------------------------------------------
// chat_history_remember
// ---------------------------------------------------------------------------

pub(super) fn chat_history_remember(state: &mut AppState, raw: &str) {
    super::chat_history_reset_nav(state);
    if raw.trim().is_empty() {
        return;
    }
    if state
        .agents
        .chat_prompt_history
        .last()
        .is_some_and(|prev| prev == raw)
    {
        return;
    }
    state.agents.chat_prompt_history.push(raw.to_string());
    if state.agents.chat_prompt_history.len() > CHAT_PROMPT_HISTORY_MAX {
        let excess = state.agents.chat_prompt_history.len() - CHAT_PROMPT_HISTORY_MAX;
        state.agents.chat_prompt_history.drain(0..excess);
    }
}

// ---------------------------------------------------------------------------
// chat_input_byte_index
// ---------------------------------------------------------------------------

pub(super) fn chat_input_byte_index(input: &str, char_idx: usize) -> usize {
    if char_idx == 0 {
        return 0;
    }
    input
        .char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(input.len())
}

// ---------------------------------------------------------------------------
// broadcast_target_agents
// ---------------------------------------------------------------------------

pub(super) fn broadcast_target_agents(state: &AppState, mission_id: Option<&str>) -> Vec<String> {
    if let Some(mission_id) = mission_id {
        if let Some(mission) = state.agents.missions.iter().find(|m| m.id == mission_id) {
            let mut targets = Vec::new();
            for agent_id in mission.assigned_agents.iter() {
                if targets.iter().any(|id| id == agent_id) {
                    continue;
                }
                // Skip @new chat clones — they should not receive @all broadcasts.
                if is_chat_clone_agent_id(agent_id) {
                    continue;
                }
                let is_dispatchable = state
                    .agents
                    .agents
                    .iter()
                    .find(|agent| agent.id == *agent_id)
                    .is_some_and(|agent| agent.is_codex() || agent.is_claude());
                if is_dispatchable {
                    targets.push(agent_id.clone());
                }
            }
            if !targets.is_empty() {
                return targets;
            }
        }
    }

    state
        .agents
        .agents
        .iter()
        .filter(|agent| {
            (agent.is_codex() || agent.is_claude()) && !is_chat_clone_agent_id(&agent.id)
        })
        .map(|agent| agent.id.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// parse_chat_input_channel
// ---------------------------------------------------------------------------

pub(super) fn parse_chat_input_channel(raw: &str) -> (AgentChannel, String) {
    if let Some(after) = raw.strip_prefix("@all") {
        if after.is_empty() || after.starts_with(char::is_whitespace) {
            return (AgentChannel::Broadcast, after.trim_start().to_string());
        }
    }
    (AgentChannel::Agent, raw.to_string())
}

// ---------------------------------------------------------------------------
// push_chat_message
// ---------------------------------------------------------------------------

pub(super) fn push_chat_message(state: &mut AppState) -> Option<(AgentChannel, String)> {
    let raw = state.agents.chat_input.clone();
    if raw.trim().is_empty() {
        return None;
    }
    let (channel, text) = parse_chat_input_channel(&raw);
    if text.trim().is_empty() {
        return None;
    }

    let message = AgentMessage {
        at: timestamp_label(state),
        channel,
        agent_id: None,
        mission_id: state
            .agents
            .selected_context_mission()
            .map(ToString::to_string),
        text: text.clone(),
        prompt_msg_idx: None,
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
    } else if let Some(agent_id) = state.agents.selected_context_agent().map(str::to_string) {
        mark_ad_hoc_provenance_dirty(state, &agent_id);
    }
    state.agents.messages.push(message);
    state.agents.diag_events.push(AgentDiagnosticEvent {
        severity: AgentAlertSeverity::Info,
        source: "thread".into(),
        message: format!("sent message: {raw}"),
        at: timestamp_label(state),
    });
    state.agents.console_scroll = CONSOLE_SCROLL_BOTTOM;
    state.agents.chat_input.clear();
    state.agents.chat_input_cursor = 0;
    state.agents.chat_input_scroll = usize::MAX;
    state.agents.chat_input_selection_anchor = None;
    Some((channel, text))
}

// ---------------------------------------------------------------------------
// slice_by_char
// ---------------------------------------------------------------------------

pub(super) fn slice_by_char(input: &str, start: usize, end: usize) -> String {
    if start >= end {
        return String::new();
    }
    let mut start_byte = None;
    let mut end_byte = None;
    for (count, (idx, _)) in input.char_indices().enumerate() {
        if count == start {
            start_byte = Some(idx);
        }
        if count == end {
            end_byte = Some(idx);
            break;
        }
    }
    let start_byte = start_byte.unwrap_or(input.len());
    let end_byte = end_byte.unwrap_or(input.len());
    input[start_byte..end_byte].to_string()
}

// ---------------------------------------------------------------------------
// Private helpers (only used by functions in this module)
// ---------------------------------------------------------------------------

fn detect_swarm_template_from_prompt(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line
            .trim()
            .trim_start_matches(['-', '*', '\u{2022}'])
            .trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix("template:") else {
            continue;
        };
        let value = rest.trim();
        if value.is_empty() {
            continue;
        }
        let value = value
            .trim_matches(|ch: char| matches!(ch, '`' | '"' | '\''))
            .trim();
        let token = value
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|ch: char| matches!(ch, ',' | '.' | ';' | ')'));
        let canonical =
            if token.eq_ignore_ascii_case("parallel") || token.eq_ignore_ascii_case("v1") {
                Some("parallel")
            } else if token.eq_ignore_ascii_case("bulk") || token.eq_ignore_ascii_case("bo") {
                Some("bulk")
            } else if token.eq_ignore_ascii_case("lab")
                || token.eq_ignore_ascii_case("default")
                || token.eq_ignore_ascii_case("v2")
            {
                Some("lab")
            } else {
                None
            };
        if let Some(canonical) = canonical {
            return Some(canonical.to_string());
        }
    }
    None
}

fn roster_default_swarm_mission_kind(state: &AppState) -> Option<SwarmMissionKind> {
    parse_swarm_mission_kind(Some(state.agents.swarm_default_mission.as_str()))
}

fn effective_swarm_mission_kind(
    state: &AppState,
    raw: &str,
    current: Option<SwarmMissionKind>,
) -> Option<SwarmMissionKind> {
    current
        .or_else(|| explicit_swarm_mission_kind_from_prompt(raw))
        .or_else(|| roster_default_swarm_mission_kind(state))
        .or_else(|| detect_swarm_mission_kind_from_prompt(raw))
}

fn detect_implicit_swarm_command(state: &AppState, raw: &str) -> Option<SwarmCommand> {
    if raw.trim().is_empty() {
        return None;
    }
    // Avoid hijacking explicit `@all`/`@swarm`/etc. prefixes.
    if raw.trim_start().starts_with('@') {
        return None;
    }

    let template = detect_swarm_template_from_prompt(raw)
        .or_else(|| {
            let upper = raw.to_ascii_uppercase();
            if upper.contains("SWARM PLANNER") || upper.contains("SWARM SYNTHESIZER") {
                Some(state.agents.swarm_default_template.clone())
            } else {
                None
            }
        })
        .or_else(|| {
            // Operator-selected template in the roster: treat the next prompt as a swarm launch.
            if matches!(
                state.agents.swarm_default_template.as_str(),
                "bulk" | "parallel"
            ) && state
                .agents
                .agents
                .iter()
                .filter(|lane| lane.is_codex())
                .count()
                >= 2
            {
                Some(state.agents.swarm_default_template.clone())
            } else {
                None
            }
        })?;

    let size = if state.agents.codex_max_parallel_turns
        != CodexRunnerConfig::default().max_parallel_turns
    {
        SwarmSize::Count(state.agents.codex_max_parallel_turns)
    } else {
        SwarmSize::Default
    };
    Some(SwarmCommand {
        size,
        template: Some(template),
        mission_kind: effective_swarm_mission_kind(state, raw, None),
        prompt: raw.to_string(),
    })
}

fn mark_ad_hoc_provenance_dirty(state: &mut AppState, agent_id: &str) {
    if state
        .agents
        .pending_provenance_agent_ids
        .iter()
        .all(|id| id != agent_id)
    {
        state
            .agents
            .pending_provenance_agent_ids
            .push(agent_id.to_string());
    }
}
