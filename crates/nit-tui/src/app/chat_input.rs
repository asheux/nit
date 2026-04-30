use arboard::Clipboard;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::{
    AgentAlertSeverity, AgentChannel, AgentDiagnosticEvent, AgentMessage, AppState, MissionPhase,
    CONSOLE_SCROLL_BOTTOM,
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

fn reset_chat_input_and_history_nav(state: &mut AppState) {
    state.agents.chat_input.clear();
    state.agents.chat_input_cursor = 0;
    state.agents.chat_input_selection_anchor = None;
    super::chat_history_reset_nav(state);
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

// Handles text manipulation keys (characters, backspace, delete, cursor movement,
// selection, clipboard). Does NOT handle Enter-submit, Esc, or Up/Down — those are
// context-specific and left to the caller.
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
            update_chat_selection_anchor(state, selecting, cursor);
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
            update_chat_selection_anchor(state, selecting, cursor);
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
            update_chat_selection_anchor(state, selecting, cursor);
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

/// Parser for the abort command. Accepts `/abort`, `@abort` (the latter
/// for symmetry with the `@swarm` family even though `@` usually
/// dispatches new work). Forms: bare → Current, `all` → All, anything
/// else → Agent(literal). Trailing whitespace tolerated.
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

/// Single entry point for every abort trigger (`/abort`, `@abort`,
/// Ctrl+C, Esc-Esc, Mission-tab `x`). Resolves the scope into a list of
/// agent ids whose runners need a `CancelTurn`, sends the runner-side
/// commands, and posts a system alert. Returns `true` when something was
/// aborted, `false` when the request was a no-op (e.g. `/abort` with no
/// active mission).
pub(crate) fn handle_abort(
    state: &mut AppState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    swarm: &mut SwarmRuntime,
    scope: AbortScope,
) -> bool {
    let agents_to_cancel: Vec<String> = match scope {
        AbortScope::Current => {
            // Resolve the abort target with a fallback: prefer the
            // selected/context mission, but if it's already terminal
            // (a common case — operator aborts once, starts a new
            // swarm, then triggers abort again without re-selecting
            // the mission tab), fall back to any still-running swarm
            // mission. Otherwise `/abort` would keep replying "not
            // active" and the live swarm would survive.
            let selected = state
                .agents
                .selected_context_mission()
                .map(|s| s.to_string())
                .filter(|mid| swarm.is_active_mission(mid));
            let target = selected.or_else(|| swarm.active_mission_ids().into_iter().next());
            let Some(mission_id) = target else {
                state.status = Some(
                    "/abort: no active swarm mission. Use `/abort all` for runner-wide cancel."
                        .into(),
                );
                return false;
            };
            let aborted = swarm.abort_mission(state, &mission_id);
            if aborted.is_empty() {
                // Should be unreachable now (we filtered to active), but
                // keep the message for diagnostics if a mission terminates
                // between the filter and the abort call.
                state.status = Some(format!(
                    "/abort: mission `{mission_id}` is not active (already complete or unknown)."
                ));
                return false;
            }
            aborted
        }
        AbortScope::All => {
            let aborted = swarm.abort_all(state);
            // Belt-and-braces: also send CancelAll to runners so any
            // non-swarm chat turns or shadow agents in flight die too.
            if let Some(c) = codex {
                let _ = c.send(CodexCommand::CancelAll);
            }
            if let Some(c) = claude {
                let _ = c.send(ClaudeCommand::CancelAll);
            }
            aborted
        }
        AbortScope::Agent(agent_id) => {
            // Surgical: drain state-side queues for this one agent and
            // forward CancelTurn to the runners. Doesn't touch swarm
            // mission state — the rest of the mission keeps running.
            drain_queued_turns_for_agent_pub(state, &agent_id);
            vec![agent_id]
        }
    };

    for agent_id in &agents_to_cancel {
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
    !agents_to_cancel.is_empty()
}

// Submit the chat input, dispatch to agents, and return whether a prompt was sent.
// Shared between the main Agent Chat Enter handler and the Artifacts popup input.
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
            // Capture the operator's *original* size request before any
            // template-specific clamping, so the advisory below can report
            // "Requested 50, started 12" verbatim.
            let original_size = cmd.size;
            // Bulk template caps proposer count: past ~BULK_PRACTICAL_MAX
            // the per-dep budget for the judge collapses (240k / N chars
            // per dep), so additional proposers contribute headers, not
            // reasoning. Hard-cap here before select_swarm_agents so the
            // downstream pipeline never sees the original oversized request.
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
            // Intended count uses the original request so messages reflect
            // what the operator typed, not the post-clamp value.
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
                // `agent_count` is the pre-clone pool size returned by
                // `select_swarm_agents`. The actual swarm size emerges
                // after `ensure_size_clones` runs inside `swarm.start`,
                // padding the roster up to the requested target (capped
                // at the FD ceiling). For `All` no cloning happens, so
                // the final size matches the pool. Compute the final
                // size deterministically here so the clamp message
                // reflects what the operator will actually see.
                let final_size = match cmd.size {
                    SwarmSize::Default => DEFAULT_SWARM_SIZE.min(ceiling).max(1),
                    SwarmSize::All => agent_count,
                    SwarmSize::Count(n) => n.clamp(1, ceiling),
                };
                // Advisory ordering: bulk-template cap > FD-bound clamp >
                // pool-bound clamp > generic large-swarm warning. Only one
                // fires; whichever explains the surprise the operator most
                // needs to see.
                let was_clamped = intended_count > final_size;
                let fd_bound_clamp =
                    was_clamped && bulk_capped_from.is_none() && final_size == ceiling;
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
                // Independent advisory: nit's planner produces the entire
                // task DAG in one pass (no hierarchical planning). Past
                // ~20 task assignments, lightweight models (haiku / mini
                // / nano / flash) start producing repetitive or shallow
                // role assignments. Surface this as a separate message so
                // operators can swap the planner before paying for a run
                // that won't be coherent.
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
                swarm_handled = true;
            } else {
                state.agents.chat_input = cmd.prompt.clone();
                sync_cursor_to_input_end(state);
            }
        } else {
            state.status = Some("@swarm requires at least one Codex or Claude agent".into());
            state.agents.chat_input.clear();
            state.agents.chat_input_cursor = 0;
            swarm_handled = true;
        }
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
        if let Some((channel, prompt)) = sent {
            let prompt = augment_with_module_file_checklist(state, prompt);
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
            // Shadow pipeline: enabled explicitly (@shadow) or auto for heavy
            // single-agent prompts. Skipped for swarm followups, broadcasts,
            // @new clone dispatches, @queue prompts, and when no single agent
            // is selected.
            let shadow_eligible = !is_swarm_mission
                && !force_new
                && !legacy_queue
                && matches!(channel, AgentChannel::Agent);
            let shadow_requested =
                shadow_eligible && (shadow_explicit || should_auto_enable_shadows(&prompt));
            let mut shadow_handled = false;
            if shadow_requested {
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
                            prompt.clone(),
                            mission_id.clone(),
                            Some(prompt_msg_idx),
                        ) {
                            // Proposers dispatch immediately — the workspace-
                            // scan runtime keeps `state.genome_reports`
                            // populated so the augmented landscape is ready
                            // without per-dispatch prescan.
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
                            shadow_handled = true;
                        }
                    }
                }
            }

            let targets = if is_swarm_mission || shadow_handled {
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

// When the prompt names a module or directory, append a non-negotiable per-file
// checklist so a single-agent refactor covers every file rather than
// cherry-picking. Returns the prompt unchanged when no scope is detected.
fn augment_with_module_file_checklist(state: &AppState, prompt: String) -> String {
    let scope = crate::swarm::enumerate_scope_files(state.workspace_root.as_path(), &prompt);
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
        "Do NOT add inline test modules (`#[cfg(test)] mod tests { ... }`) inside source files. Tests must live in a dedicated tests directory or test file.\n",
    );
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
