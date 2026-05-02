use arboard::Clipboard;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::{
    AgentAlertSeverity, AgentChannel, AgentDiagnosticEvent, AgentMessage, AgentStatus, AppState,
    MissionPhase, CONSOLE_SCROLL_BOTTOM,
};

use crate::claude_runner::{ClaudeCommand, ClaudeRunner};
use crate::codex_runner::{CodexCommand, CodexRunner, CodexRunnerConfig};
use crate::shadow::{parse_shadow_command, should_auto_enable_shadows, ShadowRuntime};
use crate::swarm::{
    create_chat_clone, current_fd_soft_limit, detect_swarm_mission_kind_from_prompt,
    drain_queued_turns_for_agent_pub, effective_max_swarm_size,
    explicit_swarm_mission_kind_from_prompt, is_agent_busy, is_agent_family_busy,
    is_chat_clone_agent_id, is_light_planner, large_swarm_warn_threshold, parse_swarm_command,
    parse_swarm_mission_kind, push_system_alert_to_mission, select_swarm_agents,
    swarm_intended_size, SwarmCommand, SwarmMissionKind, SwarmRuntime, SwarmSize,
    BULK_PRACTICAL_MAX, DEFAULT_SWARM_SIZE, LARGE_SWARM_WARN_THRESHOLD,
    LIGHT_PLANNER_SWARM_THRESHOLD,
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

pub(crate) struct ChatInputEditResult {
    pub(crate) handled: bool,
    pub(crate) changed: bool,
    pub(crate) follow_cursor: bool,
}

#[allow(clippy::too_many_arguments)]
fn try_dispatch_swarm_command(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    swarm: &mut SwarmRuntime,
    cmd: &mut SwarmCommand,
    raw: &str,
) -> bool {
    let auto_switch_ops_dag = cmd
        .template
        .as_deref()
        .is_some_and(|t| matches!(t, "bulk" | "bo"));
    chat_history_remember(state, raw);
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
        let original_size = cmd.size;
        let bulk_template = matches!(cmd.template.as_deref(), Some("bulk") | Some("bo"));
        let bulk_capped_from: Option<usize> = if bulk_template {
            let intended = swarm_intended_size(state, original_size);
            if intended > BULK_PRACTICAL_MAX {
                cmd.size = SwarmSize::Count(BULK_PRACTICAL_MAX);
                Some(intended)
            } else {
                None
            }
        } else {
            None
        };
        let agents = select_swarm_agents(state, &planner, cmd.size, cmd.template.as_deref());
        let agent_count = agents.len();
        let intended_count = swarm_intended_size(state, original_size);
        if let Some((mission_id, dispatches)) = swarm.start(
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
            sync_cursor_to_input_end(state);
            let _ = push_chat_message(state);
            let warn_threshold = large_swarm_warn_threshold();
            let ceiling = effective_max_swarm_size();
            let fd_limit = current_fd_soft_limit();
            let final_size = match cmd.size {
                SwarmSize::Default => DEFAULT_SWARM_SIZE.min(ceiling).max(1),
                SwarmSize::All => agent_count,
                SwarmSize::Count(n) => n.clamp(1, ceiling),
            };
            let was_clamped = intended_count > final_size;
            let fd_bound_clamp = was_clamped && bulk_capped_from.is_none() && final_size == ceiling;
            if let Some(intended_bulk) = bulk_capped_from {
                push_system_alert_to_mission(
                    state,
                    &mission_id,
                    format!(
                        "Bulk template capped at {BULK_PRACTICAL_MAX} proposers \
                         (requested {intended_bulk}, started {final_size}). The \
                         judge's per-dep budget is 240KB total / N proposers; past \
                         {BULK_PRACTICAL_MAX} each proposal is truncated below ~20KB \
                         and contributes headers, not reasoning. Use `template=parallel` \
                         for larger fan-outs where each agent owns its own write region."
                    ),
                );
            } else if fd_bound_clamp {
                push_system_alert_to_mission(
                    state,
                    &mission_id,
                    format!(
                        "Requested {intended_count} agents, started {final_size} \
                         (effective ceiling {ceiling}; `ulimit -n` is {fd_limit}). \
                         Bump `ulimit -n 4096` and restart nit for more headroom."
                    ),
                );
            } else if was_clamped {
                push_system_alert_to_mission(
                    state,
                    &mission_id,
                    format!(
                        "Requested {intended_count} agents, started {final_size} \
                         (only {final_size} eligible agents in the roster)."
                    ),
                );
            } else if final_size >= warn_threshold {
                let msg = if ceiling < LARGE_SWARM_WARN_THRESHOLD {
                    format!(
                        "Large swarm ({final_size} agents). Process FD limit \
                         is {fd_limit} (`ulimit -n`); effective swarm ceiling \
                         ~{ceiling}. Run `ulimit -n 4096` and restart nit for \
                         more headroom."
                    )
                } else {
                    format!(
                        "Large swarm ({final_size} agents). Each agent spawns a \
                         Codex/Claude subprocess (~4 fds, ~50–200 MB each). \
                         Verify the host has spare RAM/CPU before continuing."
                    )
                };
                push_system_alert_to_mission(state, &mission_id, msg);
            }
            if final_size > LIGHT_PLANNER_SWARM_THRESHOLD && is_light_planner(&planner) {
                push_system_alert_to_mission(
                    state,
                    &mission_id,
                    format!(
                        "Planner `{planner}` is a lightweight model — coherently \
                         planning {final_size} task assignments may exceed its \
                         reasoning depth. Consider re-running with a sonnet/opus-tier \
                         planner for swarms past ~{LIGHT_PLANNER_SWARM_THRESHOLD} agents."
                    ),
                );
            }
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
            true
        } else {
            state.agents.chat_input = cmd.prompt.clone();
            sync_cursor_to_input_end(state);
            false
        }
    } else {
        state.status = Some("@swarm requires at least one Codex or Claude agent".into());
        state.agents.chat_input.clear();
        state.agents.chat_input_cursor = 0;
        true
    }
}

#[allow(clippy::too_many_arguments)]
fn try_dispatch_shadow_pipeline(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    shadow: &mut ShadowRuntime,
    selected_agent: &Option<String>,
    prompt: &str,
    mission_id: &Option<String>,
    prompt_msg_idx: usize,
) -> bool {
    if let Some(main_agent_id) = selected_agent.as_ref() {
        let dispatchable = state
            .agents
            .agents
            .iter()
            .find(|lane| &lane.id == main_agent_id)
            .is_some_and(|lane| lane.is_codex() || lane.is_claude());
        if dispatchable && !shadow.has_run_for(main_agent_id) {
            if let Some(dispatches) = shadow.start(
                state,
                main_agent_id.clone(),
                prompt.to_string(),
                mission_id.clone(),
                Some(prompt_msg_idx),
            ) {
                for mut d in dispatches {
                    super::augment_shadow_prompt_with_landscape(state, &mut d);
                    dispatch_agent_prompt(
                        state,
                        vitals,
                        codex,
                        claude,
                        d.agent_id,
                        d.mission_id,
                        d.prompt,
                    );
                }
                maybe_dispatch_next_queued_codex_turn(state, vitals, codex);
                maybe_dispatch_next_queued_claude_turn(state, vitals, claude);
                return true;
            }
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn dispatch_to_selected_targets(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    targets: Vec<String>,
    prompt: &str,
    mission_id: &Option<String>,
    prompt_msg_idx: usize,
    force_new: bool,
) {
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
                dispatch_agent_turn_for_model_kind(
                    state,
                    vitals,
                    codex,
                    claude,
                    &clone_id,
                    mission_id,
                    prompt.to_string(),
                    prompt_msg_idx,
                    true,
                    is_claude,
                );
            } else if is_claude {
                enqueue_claude_turn(
                    state,
                    vitals,
                    Some(base_model),
                    mission_id.clone(),
                    prompt.to_string(),
                    Some(prompt_msg_idx),
                );
            } else {
                enqueue_codex_turn(
                    state,
                    vitals,
                    Some(base_model),
                    mission_id.clone(),
                    prompt.to_string(),
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
                    prompt.to_string(),
                    Some(prompt_msg_idx),
                );
            } else {
                enqueue_codex_turn(
                    state,
                    vitals,
                    Some(base_model),
                    mission_id.clone(),
                    prompt.to_string(),
                    Some(prompt_msg_idx),
                );
            }
        } else {
            dispatch_agent_turn_for_model_kind(
                state,
                vitals,
                codex,
                claude,
                &base_model,
                mission_id,
                prompt.to_string(),
                prompt_msg_idx,
                false,
                is_claude,
            );
        }
    }
    maybe_dispatch_next_queued_codex_turn(state, vitals, codex);
    maybe_dispatch_next_queued_claude_turn(state, vitals, claude);
}

fn reset_chat_input_and_history_nav(state: &mut AppState) {
    state.agents.chat_input.clear();
    state.agents.chat_input_cursor = 0;
    state.agents.chat_input_selection_anchor = None;
    super::chat_history_reset_nav(state);
}

#[allow(clippy::too_many_arguments)]
fn dispatch_agent_turn_for_model_kind(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    agent_id: &str,
    mission_id: &Option<String>,
    prompt: String,
    prompt_msg_idx: usize,
    force_new: bool,
    is_claude: bool,
) {
    if force_new {
        if is_claude {
            state
                .agents
                .claude_turn_prompt_idx
                .insert(agent_id.to_string(), prompt_msg_idx);
            maybe_dispatch_claude_turn(
                state,
                vitals,
                claude,
                Some(agent_id.to_string()),
                mission_id.clone(),
                prompt,
                true,
            );
        } else {
            state
                .agents
                .codex_turn_prompt_idx
                .insert(agent_id.to_string(), prompt_msg_idx);
            maybe_dispatch_codex_turn(
                state,
                vitals,
                codex,
                Some(agent_id.to_string()),
                mission_id.clone(),
                prompt,
                true,
            );
        }
    } else if is_claude {
        state
            .agents
            .claude_turn_prompt_idx
            .insert(agent_id.to_string(), prompt_msg_idx);
        maybe_dispatch_claude_turn(
            state,
            vitals,
            claude,
            Some(agent_id.to_string()),
            mission_id.clone(),
            prompt,
            true,
        );
    } else {
        state
            .agents
            .codex_turn_prompt_idx
            .insert(agent_id.to_string(), prompt_msg_idx);
        maybe_dispatch_codex_turn(
            state,
            vitals,
            codex,
            Some(agent_id.to_string()),
            mission_id.clone(),
            prompt,
            true,
        );
    }
}

fn sync_cursor_to_input_end(state: &mut AppState) {
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
}

fn update_chat_selection_anchor(state: &mut AppState, selecting: bool, cursor: usize) {
    if selecting {
        if state.agents.chat_input_selection_anchor.is_none() {
            state.agents.chat_input_selection_anchor = Some(cursor);
        }
    } else {
        state.agents.chat_input_selection_anchor = None;
    }
}

fn apply_cursor_movement(
    state: &mut AppState,
    clipboard: &mut Option<arboard::Clipboard>,
    new_cursor: usize,
    selecting: bool,
) -> (bool, bool) {
    let cursor = state.agents.chat_input_cursor;
    let (changed, follow_cursor) = if new_cursor != cursor {
        state.agents.chat_input_cursor = new_cursor;
        (true, true)
    } else {
        (false, false)
    };
    if selecting {
        copy_chat_input_selection(state, clipboard);
    }
    (changed, follow_cursor)
}

pub(crate) fn handle_chat_input_editing_key(
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
                reset_chat_input_and_history_nav(state);
                changed = true;
                follow_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('c'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            // Copy selection if any, else clear non-empty input. When
            // BOTH branches no-op (no selection AND empty input), leave
            // `handled = false` so the agent-console-level Ctrl+C
            // handler (added for the abort feature) gets a turn.
            if copy_chat_input_selection(state, clipboard) {
                handled = true;
            } else if !state.agents.chat_input.is_empty() {
                reset_chat_input_and_history_nav(state);
                handled = true;
                changed = true;
                follow_cursor = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('\u{3}'),
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            // Same as above for terminals that send Ctrl+C as the raw
            // ETX byte — fall through when nothing to do so the abort
            // path can pick it up.
            if copy_chat_input_selection(state, clipboard) {
                handled = true;
            } else if !state.agents.chat_input.is_empty() {
                reset_chat_input_and_history_nav(state);
                handled = true;
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
            update_chat_selection_anchor(state, selecting, cursor);
            let new_cursor = if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                chat_cursor_move_word_left(&state.agents.chat_input, cursor)
            } else {
                cursor.saturating_sub(1)
            };
            let (c, f) = apply_cursor_movement(state, clipboard, new_cursor, selecting);
            changed = changed || c;
            follow_cursor = follow_cursor || f;
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
            update_chat_selection_anchor(state, selecting, cursor);
            let new_cursor = if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                chat_cursor_move_word_right(&state.agents.chat_input, cursor)
            } else {
                cursor.saturating_add(1).min(max)
            };
            let (c, f) = apply_cursor_movement(state, clipboard, new_cursor, selecting);
            changed = changed || c;
            follow_cursor = follow_cursor || f;
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
            update_chat_selection_anchor(state, selecting, cursor);
            let (line_start, _line_end) =
                super::chat_current_line_bounds(&state.agents.chat_input, cursor);
            let new_cursor = if modifiers.contains(KeyModifiers::CONTROL) {
                0
            } else {
                line_start
            };
            let (c, f) = apply_cursor_movement(state, clipboard, new_cursor, selecting);
            changed = changed || c;
            follow_cursor = follow_cursor || f;
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
            update_chat_selection_anchor(state, selecting, cursor);
            let (_line_start, line_end) =
                super::chat_current_line_bounds(&state.agents.chat_input, cursor);
            let new_cursor = if modifiers.contains(KeyModifiers::CONTROL) {
                max
            } else {
                line_end
            };
            let (c, f) = apply_cursor_movement(state, clipboard, new_cursor, selecting);
            changed = changed || c;
            follow_cursor = follow_cursor || f;
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

/// Maximum gap between two Esc presses for them to count as Esc-Esc.
/// Tuned above typical double-press latency (~300ms) and below the
/// threshold where the operator could plausibly hit two unrelated
/// Escs in a row.
pub(crate) const ESC_ESC_ABORT_WINDOW: std::time::Duration = std::time::Duration::from_millis(500);

// Per-thread state for the Esc-Esc trigger. Thread-local (not a
// process-wide static) so parallel `cargo test` runs don't pollute
// each other through shared state — an Esc in test A would otherwise
// bleed into test B if their threads were close enough in time. Lives
// here (not on `InputState`) because `handle_agent_console_key`
// doesn't take an `InputState` and threading one through every key
// path for this single bit is more surgery than it's worth.
thread_local! {
    static LAST_CHAT_ESC_AT: std::cell::Cell<Option<std::time::Instant>>
        = const { std::cell::Cell::new(None) };
}

/// Returns true when an Esc key event lands within `ESC_ESC_ABORT_WINDOW`
/// of the previous one. Updates the timestamp regardless. Caller resets
/// (via `clear_chat_esc_state`) once the abort fires so a third Esc
/// doesn't immediately re-trigger.
pub(crate) fn record_chat_esc_press() -> bool {
    let now = std::time::Instant::now();
    LAST_CHAT_ESC_AT.with(|cell| {
        let prev = cell.get();
        cell.set(Some(now));
        prev.is_some_and(|t| now.duration_since(t) <= ESC_ESC_ABORT_WINDOW)
    })
}

pub(crate) fn clear_chat_esc_state() {
    LAST_CHAT_ESC_AT.with(|cell| cell.set(None));
}

/// Scope of an `/abort` (or `@abort`) request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AbortScope {
    /// Abort the mission currently in chat context. Most common form;
    /// `/abort` alone resolves to this.
    Current,
    /// Abort every active swarm mission and clear both runner queues.
    /// Triggered by `/abort all`.
    All,
    /// Cancel any in-flight + queued turns for a single agent id, leave
    /// the rest of the mission running. Surgical strike for `/abort
    /// <agent-id>` when one clone is hung.
    Agent(String),
}

/// Parses `/abort` and `@abort` prefixes. Forms: bare → `Current`,
/// `all` → `All`, `<agent-id>` → `Agent`. Rejects substring matches
/// like `/abortif` so prompts that happen to start with those letters
/// aren't hijacked.
pub(crate) fn parse_abort_command(raw: &str) -> Option<AbortScope> {
    let trimmed = raw.trim_start();
    let after = trimmed
        .strip_prefix("/abort")
        .or_else(|| trimmed.strip_prefix("@abort"))?;
    // Reject substring matches like `/abortif` so the parser doesn't
    // hijack a real prompt that happens to start with the same letters.
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        return None;
    }
    let arg = after.trim();
    if arg.is_empty() {
        return Some(AbortScope::Current);
    }
    if arg.eq_ignore_ascii_case("all") {
        return Some(AbortScope::All);
    }
    Some(AbortScope::Agent(arg.to_string()))
}

/// Routes every abort trigger (chat `/abort`, `@abort`, Ctrl+C with
/// empty input, Esc-Esc, mission `x`) to the same path: resolve scope
/// to a list of agent ids, send `CancelTurn` to each runner, and post
/// a system alert. Returns true when at least one agent was cancelled.
pub(crate) fn handle_abort(
    state: &mut AppState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    swarm: &mut SwarmRuntime,
    scope: AbortScope,
) -> bool {
    let multipane_focus = state.multipane.as_ref().map(|mp| mp.focused);
    let agents_to_cancel = match scope {
        AbortScope::Current => resolve_current_abort(state, swarm, multipane_focus),
        AbortScope::All => resolve_all_abort(state, codex, claude, swarm, multipane_focus),
        AbortScope::Agent(agent_id) => resolve_agent_abort(state, agent_id, multipane_focus),
    };
    let Some(targets) = agents_to_cancel else {
        return false;
    };
    for agent_id in &targets {
        if let Some(c) = codex {
            let _ = c.send(CodexCommand::CancelTurn {
                agent_id: agent_id.clone(),
            });
        }
        if let Some(c) = claude {
            let _ = c.send(ClaudeCommand::CancelTurn {
                agent_id: agent_id.clone(),
            });
        }
    }
    !targets.is_empty()
}

/// Resolve `AbortScope::Current` to the list of agent ids whose runners
/// still need a `CancelTurn`. Returns `None` (with a `state.status`
/// message attached) when there's nothing to abort. In multipane the
/// pane-aware caller routes synthetic-only state to `AbortScope::Agent`
/// before this runs, so the global `active_mission_ids` fallback is
/// disabled here to avoid cross-pane swarm kills.
fn resolve_current_abort(
    state: &mut AppState,
    swarm: &mut SwarmRuntime,
    multipane_focus: Option<usize>,
) -> Option<Vec<String>> {
    let selected = state
        .agents
        .selected_context_mission()
        .map(|s| s.to_string())
        .filter(|mid| swarm.is_active_mission(mid));
    // Multipane synthetic-only state — single-agent dispatch with only a
    // mp-pane-NN-chat id. Surgically cancel the focused pane's lane via
    // AbortScope::Agent, the same path Ctrl+C / Esc-Esc / Mission-tab `x`
    // use through `abort_focused_pane`. Without this fallthrough, /abort
    // posts "no active swarm mission" while a turn is genuinely in
    // flight, leaving the operator stuck.
    let mission_target = if selected.is_some() {
        selected
    } else if multipane_focus.is_some() {
        let agent = state
            .agents
            .selected_context_agent()
            .map(str::to_string)
            .filter(|candidate| lane_has_in_flight_turn(state, candidate.as_str()));
        let Some(agent) = agent else {
            state.status = Some("/abort: no active mission for this pane.".into());
            return None;
        };
        drain_queued_turns_for_agent_pub(state, &agent);
        return Some(vec![agent]);
    } else {
        swarm.active_mission_ids().into_iter().next()
    };
    let Some(mission_id) = mission_target else {
        state.status = Some(
            "/abort: no active swarm mission. Use `/abort all` for runner-wide cancel.".into(),
        );
        return None;
    };
    let aborted = swarm.abort_mission(state, &mission_id);
    if aborted.is_empty() {
        state.status = Some(format!(
            "/abort: mission `{mission_id}` is not active (already complete or unknown)."
        ));
        return None;
    }
    Some(aborted)
}

/// Resolve `AbortScope::All`. Single-pane is global (`swarm.abort_all` +
/// `CancelAll`). Multipane scopes to the focused pane's missions and
/// emits per-agent `CancelTurn` instead of `CancelAll` so sibling-pane
/// work survives.
fn resolve_all_abort(
    state: &mut AppState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    swarm: &mut SwarmRuntime,
    multipane_focus: Option<usize>,
) -> Option<Vec<String>> {
    if let Some(focus) = multipane_focus {
        return Some(abort_all_for_pane(state, swarm, focus));
    }
    let aborted = swarm.abort_all(state);
    if let Some(c) = codex {
        let _ = c.send(CodexCommand::CancelAll);
    }
    if let Some(c) = claude {
        let _ = c.send(ClaudeCommand::CancelAll);
    }
    Some(aborted)
}

/// Resolve `AbortScope::Agent(<id>)`. Single-pane: surgical, always
/// honoured. Multipane: rejected (with a `state.status` message) when
/// the targeted id does not belong to the focused pane, so the operator
/// can't kill sibling-pane work via the surgical path.
fn resolve_agent_abort(
    state: &mut AppState,
    agent_id: String,
    multipane_focus: Option<usize>,
) -> Option<Vec<String>> {
    if let Some(focus) = multipane_focus {
        let owns_via_id = crate::multipane::agent_id::pane_owns_agent(&agent_id, focus);
        let owns_via_lane = state
            .multipane
            .as_ref()
            .and_then(|mp| mp.panes.get(focus))
            .is_some_and(|p| p.agent_id == agent_id);
        if !owns_via_id && !owns_via_lane {
            state.status = Some(format!(
                "/abort {agent_id}: agent does not belong to the focused pane."
            ));
            return None;
        }
    }
    drain_queued_turns_for_agent_pub(state, &agent_id);
    Some(vec![agent_id])
}

/// Pane-scoped `/abort all`. Aborts every active swarm mission owned
/// by `pane_idx`, then surgically drains any pane-owned lane with an
/// in-flight non-swarm turn. Returns the union of agent ids whose
/// runners still need a `CancelTurn`.
fn abort_all_for_pane(
    state: &mut AppState,
    swarm: &mut SwarmRuntime,
    pane_idx: usize,
) -> Vec<String> {
    use crate::multipane::agent_id::pane_owns_agent;

    let pane_missions = collect_pane_missions(state, swarm, pane_idx);
    let mut aborted: Vec<String> = Vec::new();
    for mid in &pane_missions {
        for agent in swarm.abort_mission(state, mid) {
            if !aborted.contains(&agent) {
                aborted.push(agent);
            }
        }
    }
    let pane_lanes: Vec<String> = state
        .agents
        .agents
        .iter()
        .filter(|lane| pane_owns_agent(&lane.id, pane_idx))
        .map(|lane| lane.id.clone())
        .collect();
    for lane_id in pane_lanes {
        if aborted.contains(&lane_id) || !lane_has_in_flight_turn(state, &lane_id) {
            continue;
        }
        drain_queued_turns_for_agent_pub(state, &lane_id);
        aborted.push(lane_id);
    }
    if aborted.is_empty() {
        state.status = Some("/abort all: no active work for this pane.".into());
    }
    aborted
}

fn collect_pane_missions(state: &AppState, swarm: &SwarmRuntime, pane_idx: usize) -> Vec<String> {
    use crate::multipane::agent_id::pane_owns_agent;
    swarm
        .active_mission_ids()
        .into_iter()
        .filter(|mid| {
            state
                .agents
                .missions
                .iter()
                .find(|m| &m.id == mid)
                .is_some_and(|m| {
                    m.assigned_agents
                        .iter()
                        .any(|aid| pane_owns_agent(aid, pane_idx))
                })
        })
        .collect()
}

pub(crate) fn lane_has_in_flight_turn(state: &AppState, lane_id: &str) -> bool {
    let agents = &state.agents;
    agents.active_turns.contains_key(lane_id)
        || agents
            .queued_codex_turns
            .iter()
            .any(|t| t.agent_id == lane_id)
        || agents
            .queued_claude_turns
            .iter()
            .any(|t| t.agent_id == lane_id)
        || agents
            .agents_get(lane_id)
            .is_some_and(|lane| matches!(lane.status, AgentStatus::Running))
}

#[allow(clippy::too_many_arguments)]
fn dispatch_swarm_followup(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    swarm: &mut SwarmRuntime,
    mission_id: &Option<String>,
    prompt: &str,
    prompt_msg_idx: usize,
) -> bool {
    let swarm_config = mission_id
        .as_deref()
        .and_then(|mid| swarm.session_config(mid));
    let Some(config) = swarm_config else {
        return false;
    };
    let mid = mission_id.as_deref().unwrap();
    crate::swarm::ensure_swarm_agents_for_followup(state, mid, &config);
    swarm.reactivate_for_followup(state, mid);
    if let Some(mission) = state.agents.missions.iter_mut().find(|m| m.id == mid) {
        mission.status = "PLAN".into();
        mission.phase = MissionPhase::Plan;
    }
    let planner = config.planner_agent_id.clone();
    let plan_prompt = swarm
        .build_followup_planner_prompt(state, mid, prompt)
        .unwrap_or_else(|| prompt.to_string());
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
    true
}

pub(crate) fn submit_chat_input_and_dispatch(
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    swarm: &mut SwarmRuntime,
    shadow: &mut ShadowRuntime,
) -> bool {
    let raw = state.agents.chat_input.clone();
    if raw.trim().is_empty() {
        return false;
    }
    // `/abort` and `@abort` short-circuit before any other parsing —
    // they're never a prompt, never a swarm/shadow command, and the
    // operator wants the side-effect (kill in-flight work), not a
    // dispatch. Always clear the input regardless of outcome so the
    // operator doesn't accidentally re-submit the abort string.
    if let Some(scope) = parse_abort_command(&raw) {
        let aborted = handle_abort(state, codex, claude, swarm, scope);
        reset_chat_input_and_history_nav(state);
        return aborted;
    }
    // `@shadow <prompt>` — explicit opt-in to the single-agent shadow pipeline.
    // Strip the prefix so `push_chat_message` sees just the user's text.
    let shadow_explicit = parse_shadow_command(&raw).is_some();
    if shadow_explicit {
        let stripped = raw.trim_start().strip_prefix("@shadow").unwrap_or(&raw);
        state.agents.chat_input = stripped.trim_start().to_string();
        sync_cursor_to_input_end(state);
    }
    let mut swarm_handled = false;
    // When `@shadow` was given explicitly, do NOT also try to interpret the
    // prompt as `@swarm` — the user picked shadows on purpose.
    let mut cmd = if shadow_explicit {
        None
    } else {
        parse_swarm_command(&raw)
            .map(|mut cmd| {
                if cmd.template.is_none() {
                    cmd.template = Some(state.agents.swarm_default_template.clone());
                }
                cmd.mission_kind =
                    effective_swarm_mission_kind(state, cmd.prompt.as_str(), cmd.mission_kind);
                cmd
            })
            .or_else(|| detect_implicit_swarm_command(state, &raw))
    };
    if let Some(cmd) = cmd.as_mut() {
        swarm_handled = try_dispatch_swarm_command(state, vitals, codex, claude, swarm, cmd, &raw);
    }

    if !swarm_handled {
        let prefix_isolated = |prefix: &str| -> bool {
            raw.starts_with(prefix)
                && (raw.len() == prefix.len()
                    || raw[prefix.len()..].starts_with(char::is_whitespace))
        };
        // @new: spawn a clone with fresh context when the agent family is busy.
        let force_new = prefix_isolated("@new");
        // @q / @queue: legacy alias — now the same as the default (queue to base).
        let legacy_queue = prefix_isolated("@queue") || prefix_isolated("@q");

        if force_new {
            state.agents.chat_input = raw["@new".len()..].trim_start().to_string();
            sync_cursor_to_input_end(state);
        } else if legacy_queue {
            let stripped = raw
                .strip_prefix("@queue")
                .or_else(|| raw.strip_prefix("@q"))
                .unwrap_or(&raw);
            state.agents.chat_input = stripped.trim_start().to_string();
            sync_cursor_to_input_end(state);
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
        if let Some((channel, raw_prompt)) = sent {
            // Keep the operator's original prompt around for heuristics
            // that should fire on user intent, not on machine-appended
            // boilerplate. `augment_with_module_file_checklist` can add
            // a 1-2 KB FILE CHECKLIST block when the workspace has any
            // git-changed files (the scope fallback in
            // `enumerate_scope_files`), which would otherwise push a
            // casual "hi there" past the 500-char auto-shadow threshold.
            let prompt = augment_with_module_file_checklist(
                state,
                selected_agent.as_deref(),
                raw_prompt.clone(),
            );
            // Index of the user prompt message just pushed — used to link agent
            // responses back to the correct prompt in the chat view.
            let prompt_msg_idx = state.agents.messages.len().saturating_sub(1);
            chat_history_remember(state, &raw);
            // For swarm missions, re-activate the run and dispatch only to
            // the planner so the swarm pipeline assigns roles to clones.
            let is_swarm_mission = dispatch_swarm_followup(
                state,
                vitals,
                codex,
                claude,
                swarm,
                &mission_id,
                &prompt,
                prompt_msg_idx,
            );
            // Shadow pipeline: enabled explicitly (@shadow) or auto for heavy
            // single-agent prompts. Skipped for swarm followups, broadcasts,
            // @new clone dispatches, @queue prompts, and when no single agent
            // is selected. The auto-enable heuristic runs on the *raw*
            // prompt — see `raw_prompt` above for why.
            let shadow_eligible = !is_swarm_mission
                && !force_new
                && !legacy_queue
                && matches!(channel, AgentChannel::Agent);
            let shadow_requested =
                shadow_eligible && (shadow_explicit || should_auto_enable_shadows(&raw_prompt));
            let shadow_handled = shadow_requested
                && try_dispatch_shadow_pipeline(
                    state,
                    vitals,
                    codex,
                    claude,
                    shadow,
                    &selected_agent,
                    &prompt,
                    &mission_id,
                    prompt_msg_idx,
                );

            let targets = if is_swarm_mission || shadow_handled {
                Vec::new()
            } else if matches!(channel, AgentChannel::Broadcast) {
                broadcast_target_agents(state, mission_id.as_deref())
            } else {
                selected_agent.clone().into_iter().collect::<Vec<_>>()
            };
            dispatch_to_selected_targets(
                state,
                vitals,
                codex,
                claude,
                targets,
                &prompt,
                &mission_id,
                prompt_msg_idx,
                force_new,
            );
        } else {
            return false;
        }
    }
    state.agents.chat_input_scroll = usize::MAX;
    true
}

// When the prompt names a module or directory, append a non-negotiable per-file
// checklist so a single-agent refactor covers every file rather than
// cherry-picking. Returns the prompt unchanged when no scope is detected.
//
// Walks the spawn cwd of the target agent (multipane resolves per-pane via
// `resolve_dispatch_cwd`), so a dotbox pane never picks up nit's own paths
// when nit's repo happens to be the harness workspace_root.
fn augment_with_module_file_checklist(
    state: &AppState,
    target_agent: Option<&str>,
    prompt: String,
) -> String {
    let cwd = target_agent
        .map(|id| crate::app::resolve_dispatch_cwd(state, id))
        .unwrap_or_else(|| state.workspace_root.clone());
    let scope = crate::swarm::enumerate_scope_files(cwd.as_path(), &prompt);
    if scope.is_empty() {
        return prompt;
    }
    let mut out = prompt;
    out.push_str("\n\n## FILE CHECKLIST (non-negotiable)\n");
    out.push_str("\"Refactor module\" = refactor EVERY file below. No exceptions, no skipping.\n");
    out.push_str(
        "Process this checklist in order. Open each file, read it, refactor it, then move to the next.\n",
    );
    out.push_str("Even if a file looks clean, improve naming, docs, structure, or consistency.\n");
    out.push_str(
        "COMMENTS: Trim doc comments that restate the type/function name, \
         echo visible type signatures, or describe obvious behavior (e.g. \
         \"/// Returns the value\" on fn value()). Keep comments that explain \
         WHY something is done, document non-obvious constraints, safety \
         invariants, or algorithmic choices. A comment worth keeping tells \
         the reader something the code alone cannot.\n",
    );
    out.push_str("Your task is NOT complete until every file has been modified.\n\n");
    for (i, path) in scope.iter().enumerate() {
        out.push_str(&format!("{}. {path}\n", i + 1));
    }
    out.push_str("\nAfter finishing, list every file and what you changed in each.\n");
    out
}

pub(crate) fn chat_history_remember(state: &mut AppState, raw: &str) {
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

pub(crate) fn broadcast_target_agents(state: &AppState, mission_id: Option<&str>) -> Vec<String> {
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

    // In multipane mode the fallback must NOT cross pane boundaries:
    // restrict to lanes whose id encodes the focused pane index. Without
    // this filter, `@all` from pane 0 fans out to every pane's lanes.
    let focused_pane = state.multipane.as_ref().map(|mp| mp.focused);
    state
        .agents
        .agents
        .iter()
        .filter(|agent| {
            if !(agent.is_codex() || agent.is_claude()) || is_chat_clone_agent_id(&agent.id) {
                return false;
            }
            match focused_pane {
                Some(idx) => crate::multipane::agent_id::pane_owns_agent(&agent.id, idx),
                None => true,
            }
        })
        .map(|agent| agent.id.clone())
        .collect()
}

pub(crate) fn parse_chat_input_channel(raw: &str) -> (AgentChannel, String) {
    if let Some(after) = raw.strip_prefix("@all") {
        if after.is_empty() || after.starts_with(char::is_whitespace) {
            return (AgentChannel::Broadcast, after.trim_start().to_string());
        }
    }
    (AgentChannel::Agent, raw.to_string())
}

pub(crate) fn push_chat_message(state: &mut AppState) -> Option<(AgentChannel, String)> {
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
        kind: None,
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

fn effective_swarm_mission_kind(
    state: &AppState,
    raw: &str,
    current: Option<SwarmMissionKind>,
) -> Option<SwarmMissionKind> {
    current
        .or_else(|| explicit_swarm_mission_kind_from_prompt(raw))
        .or_else(|| parse_swarm_mission_kind(Some(state.agents.swarm_default_mission.as_str())))
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
