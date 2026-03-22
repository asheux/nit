use std::time::Instant;

use arboard::Clipboard;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::{AppState, UiSelectionPane, YankKind};
use ratatui::layout::Rect;

use crate::{
    claude_runner::ClaudeRunner,
    codex_runner::CodexRunner,
    swarm::{is_agent_busy, SwarmRuntime},
    theme::Theme,
    vitals::VitalsState,
    widgets::{agent_ops_view, artifacts_popup},
};

use super::chat_input::{
    chat_input_byte_index, push_chat_message, slice_by_char, submit_chat_input_and_dispatch,
    ChatInputEditResult,
};
use super::{
    bump_scroll_clamped, chat_current_line_bounds, chat_current_line_indent,
    chat_cursor_move_vertical, chat_cursor_move_word_left, chat_cursor_move_word_right,
    dynamic_popup_rect, enqueue_claude_turn, enqueue_codex_turn, is_global_quit_key,
    maybe_dispatch_claude_turn, maybe_dispatch_codex_turn, normalize_chat_input_text,
    popup_text_area,
};

pub(super) fn copy_popup_chat_input_selection(
    state: &mut AppState,
    clipboard: &mut Option<Clipboard>,
) -> bool {
    let total = state.agents.artifacts_popup_chat_input.chars().count();
    let cursor = state.agents.artifacts_popup_chat_cursor.min(total);
    let anchor = match state.agents.artifacts_popup_chat_selection_anchor {
        Some(a) => a.min(total),
        None => return false,
    };
    if anchor == cursor {
        return false;
    }
    let (start, end) = (anchor.min(cursor), anchor.max(cursor));
    let text = slice_by_char(&state.agents.artifacts_popup_chat_input, start, end);
    if text.is_empty() {
        return false;
    }
    state.yank_kind = if text.contains('\n') {
        YankKind::Line
    } else {
        YankKind::Char
    };
    state.yank = Some(text.clone());
    if let Some(cb) = clipboard.as_mut() {
        let _ = cb.set_text(text);
    }
    true
}

fn popup_chat_selection_range(state: &AppState) -> Option<(usize, usize)> {
    let total = state.agents.artifacts_popup_chat_input.chars().count();
    let cursor = state.agents.artifacts_popup_chat_cursor.min(total);
    let anchor = state.agents.artifacts_popup_chat_selection_anchor?.min(total);
    if anchor == cursor {
        return None;
    }
    Some((anchor.min(cursor), anchor.max(cursor)))
}

pub(super) fn delete_popup_chat_selection(state: &mut AppState) -> bool {
    let Some((start, end)) = popup_chat_selection_range(state) else {
        return false;
    };
    let remove_start =
        chat_input_byte_index(&state.agents.artifacts_popup_chat_input, start);
    let remove_end =
        chat_input_byte_index(&state.agents.artifacts_popup_chat_input, end);
    state
        .agents
        .artifacts_popup_chat_input
        .replace_range(remove_start..remove_end, "");
    state.agents.artifacts_popup_chat_cursor = start;
    state.agents.artifacts_popup_chat_selection_anchor = None;
    true
}

pub(super) fn insert_popup_chat_text(state: &mut AppState, text: &str) -> bool {
    let normalized = normalize_chat_input_text(text);
    if normalized.is_empty() {
        return false;
    }
    delete_popup_chat_selection(state);
    let insert_at = chat_input_byte_index(
        &state.agents.artifacts_popup_chat_input,
        state.agents.artifacts_popup_chat_cursor,
    );
    state
        .agents
        .artifacts_popup_chat_input
        .insert_str(insert_at, &normalized);
    state.agents.artifacts_popup_chat_cursor += normalized.chars().count();
    state.agents.artifacts_popup_chat_scroll = usize::MAX;
    state.agents.artifacts_popup_chat_selection_anchor = None;
    true
}

/// Self-contained key handler for the artifacts popup chat input.
/// Operates directly on `artifacts_popup_chat_*` fields — no swap needed.
pub(super) fn handle_artifacts_popup_chat_key(
    key: &KeyEvent,
    state: &mut AppState,
    clipboard: &mut Option<Clipboard>,
) -> ChatInputEditResult {
    let mut changed = false;
    let mut handled = false;
    let mut follow_cursor = false;

    let input_len_chars = state.agents.artifacts_popup_chat_input.chars().count();
    if state.agents.artifacts_popup_chat_cursor > input_len_chars {
        state.agents.artifacts_popup_chat_cursor = input_len_chars;
    }
    if state
        .agents
        .artifacts_popup_chat_selection_anchor
        .is_some_and(|anchor| anchor > input_len_chars)
    {
        state.agents.artifacts_popup_chat_selection_anchor = Some(input_len_chars);
    }

    match *key {
        // Shift+Enter: insert newline
        KeyEvent {
            code: KeyCode::Enter,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SHIFT) => {
            handled = true;
            delete_popup_chat_selection(state);
            let indent = chat_current_line_indent(
                &state.agents.artifacts_popup_chat_input,
                state.agents.artifacts_popup_chat_cursor,
            );
            let insert = if indent.is_empty() {
                "\n".to_string()
            } else {
                format!("\n{indent}")
            };
            let insert_at = chat_input_byte_index(
                &state.agents.artifacts_popup_chat_input,
                state.agents.artifacts_popup_chat_cursor,
            );
            state
                .agents
                .artifacts_popup_chat_input
                .insert_str(insert_at, &insert);
            state.agents.artifacts_popup_chat_cursor += insert.chars().count();
            state.agents.artifacts_popup_chat_selection_anchor = None;
            changed = true;
            follow_cursor = true;
        }
        // Ctrl+A / Cmd+A: select all
        KeyEvent {
            code: KeyCode::Char('a'),
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) => {
            handled = true;
            let total = state.agents.artifacts_popup_chat_input.chars().count();
            state.agents.artifacts_popup_chat_selection_anchor = Some(0);
            state.agents.artifacts_popup_chat_cursor = total;
            copy_popup_chat_input_selection(state, clipboard);
            changed = true;
            follow_cursor = true;
        }
        // Cmd+C or Ctrl+Shift+C: copy
        KeyEvent {
            code: KeyCode::Char('c'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SUPER)
            || (modifiers.contains(KeyModifiers::CONTROL)
                && modifiers.contains(KeyModifiers::SHIFT)) =>
        {
            handled = true;
            copy_popup_chat_input_selection(state, clipboard);
        }
        // Cmd+X or Ctrl+Shift+X: cut
        KeyEvent {
            code: KeyCode::Char('x'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SUPER)
            || (modifiers.contains(KeyModifiers::CONTROL)
                && modifiers.contains(KeyModifiers::SHIFT)) =>
        {
            handled = true;
            if copy_popup_chat_input_selection(state, clipboard)
                && delete_popup_chat_selection(state)
            {
                changed = true;
                follow_cursor = true;
            }
        }
        // Ctrl+V / Cmd+V: paste
        KeyEvent {
            code: KeyCode::Char('v'),
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) => {
            handled = true;
            if let Some(cb) = clipboard.as_mut() {
                if let Ok(text) = cb.get_text() {
                    if insert_popup_chat_text(state, &text) {
                        changed = true;
                        follow_cursor = true;
                    }
                }
            }
        }
        // Ctrl+Insert: copy
        KeyEvent {
            code: KeyCode::Insert,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            handled = true;
            copy_popup_chat_input_selection(state, clipboard);
        }
        // Shift+Insert: paste
        KeyEvent {
            code: KeyCode::Insert,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::SHIFT) => {
            handled = true;
            if let Some(cb) = clipboard.as_mut() {
                if let Ok(text) = cb.get_text() {
                    if insert_popup_chat_text(state, &text) {
                        changed = true;
                        follow_cursor = true;
                    }
                }
            }
        }
        // Ctrl+U: clear input
        KeyEvent {
            code: KeyCode::Char('u'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            handled = true;
            if !state.agents.artifacts_popup_chat_input.is_empty() {
                state.agents.artifacts_popup_chat_input.clear();
                state.agents.artifacts_popup_chat_cursor = 0;
                state.agents.artifacts_popup_chat_selection_anchor = None;
                changed = true;
                follow_cursor = true;
            }
        }
        // Ctrl+C: copy selection or clear input
        KeyEvent {
            code: KeyCode::Char('c'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            handled = true;
            if !copy_popup_chat_input_selection(state, clipboard)
                && !state.agents.artifacts_popup_chat_input.is_empty()
            {
                state.agents.artifacts_popup_chat_input.clear();
                state.agents.artifacts_popup_chat_cursor = 0;
                state.agents.artifacts_popup_chat_selection_anchor = None;
                changed = true;
                follow_cursor = true;
            }
        }
        // ETX literal
        KeyEvent {
            code: KeyCode::Char('\u{3}'),
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            handled = true;
            if !copy_popup_chat_input_selection(state, clipboard)
                && !state.agents.artifacts_popup_chat_input.is_empty()
            {
                state.agents.artifacts_popup_chat_input.clear();
                state.agents.artifacts_popup_chat_cursor = 0;
                state.agents.artifacts_popup_chat_selection_anchor = None;
                changed = true;
                follow_cursor = true;
            }
        }
        // Ctrl+Backspace / Alt+Backspace: delete word left
        KeyEvent {
            code: KeyCode::Backspace,
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
            handled = true;
            if delete_popup_chat_selection(state) {
                changed = true;
                follow_cursor = true;
            } else {
                let cursor = state.agents.artifacts_popup_chat_cursor;
                let remove_start = chat_cursor_move_word_left(
                    &state.agents.artifacts_popup_chat_input,
                    cursor,
                );
                if remove_start < cursor {
                    let start = chat_input_byte_index(
                        &state.agents.artifacts_popup_chat_input,
                        remove_start,
                    );
                    let end = chat_input_byte_index(
                        &state.agents.artifacts_popup_chat_input,
                        cursor,
                    );
                    state
                        .agents
                        .artifacts_popup_chat_input
                        .replace_range(start..end, "");
                    state.agents.artifacts_popup_chat_cursor = remove_start;
                    state.agents.artifacts_popup_chat_selection_anchor = None;
                    changed = true;
                    follow_cursor = true;
                }
            }
        }
        // Backspace: delete char left
        KeyEvent {
            code: KeyCode::Backspace,
            ..
        } => {
            handled = true;
            if delete_popup_chat_selection(state) {
                changed = true;
                follow_cursor = true;
            } else if state.agents.artifacts_popup_chat_cursor > 0 {
                let remove_start = chat_input_byte_index(
                    &state.agents.artifacts_popup_chat_input,
                    state.agents.artifacts_popup_chat_cursor - 1,
                );
                let remove_end = chat_input_byte_index(
                    &state.agents.artifacts_popup_chat_input,
                    state.agents.artifacts_popup_chat_cursor,
                );
                state
                    .agents
                    .artifacts_popup_chat_input
                    .replace_range(remove_start..remove_end, "");
                state.agents.artifacts_popup_chat_cursor -= 1;
                state.agents.artifacts_popup_chat_selection_anchor = None;
                changed = true;
                follow_cursor = true;
            }
        }
        // Ctrl+Delete / Alt+Delete: delete word right
        KeyEvent {
            code: KeyCode::Delete,
            modifiers,
            ..
        } if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
            handled = true;
            if delete_popup_chat_selection(state) {
                changed = true;
                follow_cursor = true;
            } else {
                let cursor = state.agents.artifacts_popup_chat_cursor;
                let remove_end = chat_cursor_move_word_right(
                    &state.agents.artifacts_popup_chat_input,
                    cursor,
                );
                if remove_end > cursor {
                    let start = chat_input_byte_index(
                        &state.agents.artifacts_popup_chat_input,
                        cursor,
                    );
                    let end = chat_input_byte_index(
                        &state.agents.artifacts_popup_chat_input,
                        remove_end,
                    );
                    state
                        .agents
                        .artifacts_popup_chat_input
                        .replace_range(start..end, "");
                    state.agents.artifacts_popup_chat_selection_anchor = None;
                    changed = true;
                    follow_cursor = true;
                }
            }
        }
        // Delete: delete char right
        KeyEvent {
            code: KeyCode::Delete,
            ..
        } => {
            handled = true;
            if delete_popup_chat_selection(state) {
                changed = true;
                follow_cursor = true;
            } else if state.agents.artifacts_popup_chat_cursor
                < state.agents.artifacts_popup_chat_input.chars().count()
            {
                let remove_start = chat_input_byte_index(
                    &state.agents.artifacts_popup_chat_input,
                    state.agents.artifacts_popup_chat_cursor,
                );
                let remove_end = chat_input_byte_index(
                    &state.agents.artifacts_popup_chat_input,
                    state.agents.artifacts_popup_chat_cursor + 1,
                );
                state
                    .agents
                    .artifacts_popup_chat_input
                    .replace_range(remove_start..remove_end, "");
                state.agents.artifacts_popup_chat_selection_anchor = None;
                changed = true;
                follow_cursor = true;
            }
        }
        // Left arrow
        KeyEvent {
            code: KeyCode::Left,
            modifiers,
            ..
        } => {
            handled = true;
            let total_chars = state.agents.artifacts_popup_chat_input.chars().count();
            let cursor = state.agents.artifacts_popup_chat_cursor.min(total_chars);
            let selecting = modifiers.contains(KeyModifiers::SHIFT);
            if selecting {
                if state.agents.artifacts_popup_chat_selection_anchor.is_none() {
                    state.agents.artifacts_popup_chat_selection_anchor = Some(cursor);
                }
            } else {
                state.agents.artifacts_popup_chat_selection_anchor = None;
            }
            let new_cursor =
                if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                    chat_cursor_move_word_left(
                        &state.agents.artifacts_popup_chat_input,
                        cursor,
                    )
                } else {
                    cursor.saturating_sub(1)
                };
            if new_cursor != cursor {
                state.agents.artifacts_popup_chat_cursor = new_cursor;
                changed = true;
                follow_cursor = true;
            }
            if selecting {
                copy_popup_chat_input_selection(state, clipboard);
            }
        }
        // Right arrow
        KeyEvent {
            code: KeyCode::Right,
            modifiers,
            ..
        } => {
            handled = true;
            let max = state.agents.artifacts_popup_chat_input.chars().count();
            let cursor = state.agents.artifacts_popup_chat_cursor.min(max);
            let selecting = modifiers.contains(KeyModifiers::SHIFT);
            if selecting {
                if state.agents.artifacts_popup_chat_selection_anchor.is_none() {
                    state.agents.artifacts_popup_chat_selection_anchor = Some(cursor);
                }
            } else {
                state.agents.artifacts_popup_chat_selection_anchor = None;
            }
            let new_cursor =
                if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                    chat_cursor_move_word_right(
                        &state.agents.artifacts_popup_chat_input,
                        cursor,
                    )
                } else {
                    cursor.saturating_add(1).min(max)
                };
            if new_cursor != cursor {
                state.agents.artifacts_popup_chat_cursor = new_cursor;
                changed = true;
                follow_cursor = true;
            }
            if selecting {
                copy_popup_chat_input_selection(state, clipboard);
            }
        }
        // Home
        KeyEvent {
            code: KeyCode::Home,
            modifiers,
            ..
        } => {
            handled = true;
            let total_chars = state.agents.artifacts_popup_chat_input.chars().count();
            let cursor = state.agents.artifacts_popup_chat_cursor.min(total_chars);
            let selecting = modifiers.contains(KeyModifiers::SHIFT);
            if selecting {
                if state.agents.artifacts_popup_chat_selection_anchor.is_none() {
                    state.agents.artifacts_popup_chat_selection_anchor = Some(cursor);
                }
            } else {
                state.agents.artifacts_popup_chat_selection_anchor = None;
            }
            let (line_start, _) = chat_current_line_bounds(
                &state.agents.artifacts_popup_chat_input,
                cursor,
            );
            let new_cursor = if modifiers.contains(KeyModifiers::CONTROL) {
                0
            } else {
                line_start
            };
            if new_cursor != cursor {
                state.agents.artifacts_popup_chat_cursor = new_cursor;
                changed = true;
                follow_cursor = true;
            }
            if selecting {
                copy_popup_chat_input_selection(state, clipboard);
            }
        }
        // End
        KeyEvent {
            code: KeyCode::End,
            modifiers,
            ..
        } => {
            handled = true;
            let max = state.agents.artifacts_popup_chat_input.chars().count();
            let cursor = state.agents.artifacts_popup_chat_cursor.min(max);
            let selecting = modifiers.contains(KeyModifiers::SHIFT);
            if selecting {
                if state.agents.artifacts_popup_chat_selection_anchor.is_none() {
                    state.agents.artifacts_popup_chat_selection_anchor = Some(cursor);
                }
            } else {
                state.agents.artifacts_popup_chat_selection_anchor = None;
            }
            let (_, line_end) = chat_current_line_bounds(
                &state.agents.artifacts_popup_chat_input,
                cursor,
            );
            let new_cursor = if modifiers.contains(KeyModifiers::CONTROL) {
                max
            } else {
                line_end
            };
            if new_cursor != cursor {
                state.agents.artifacts_popup_chat_cursor = new_cursor;
                changed = true;
                follow_cursor = true;
            }
            if selecting {
                copy_popup_chat_input_selection(state, clipboard);
            }
        }
        // Up arrow
        KeyEvent {
            code: KeyCode::Up,
            modifiers,
            ..
        } => {
            handled = true;
            let selecting = modifiers.contains(KeyModifiers::SHIFT);
            let cursor = state.agents.artifacts_popup_chat_cursor;
            let moved = chat_cursor_move_vertical(
                &state.agents.artifacts_popup_chat_input,
                cursor,
                -1,
            );
            if moved != cursor {
                if selecting {
                    if state.agents.artifacts_popup_chat_selection_anchor.is_none() {
                        state.agents.artifacts_popup_chat_selection_anchor = Some(cursor);
                    }
                } else {
                    state.agents.artifacts_popup_chat_selection_anchor = None;
                }
                state.agents.artifacts_popup_chat_cursor = moved;
                changed = true;
                follow_cursor = true;
                if selecting {
                    copy_popup_chat_input_selection(state, clipboard);
                }
            }
        }
        // Down arrow
        KeyEvent {
            code: KeyCode::Down,
            modifiers,
            ..
        } => {
            handled = true;
            let selecting = modifiers.contains(KeyModifiers::SHIFT);
            let cursor = state.agents.artifacts_popup_chat_cursor;
            let moved = chat_cursor_move_vertical(
                &state.agents.artifacts_popup_chat_input,
                cursor,
                1,
            );
            if moved != cursor {
                if selecting {
                    if state.agents.artifacts_popup_chat_selection_anchor.is_none() {
                        state.agents.artifacts_popup_chat_selection_anchor = Some(cursor);
                    }
                } else {
                    state.agents.artifacts_popup_chat_selection_anchor = None;
                }
                state.agents.artifacts_popup_chat_cursor = moved;
                changed = true;
                follow_cursor = true;
                if selecting {
                    copy_popup_chat_input_selection(state, clipboard);
                }
            }
        }
        // Tab
        KeyEvent {
            code: KeyCode::Tab,
            modifiers,
            ..
        } if modifiers.is_empty() => {
            handled = true;
            delete_popup_chat_selection(state);
            let insert_at = chat_input_byte_index(
                &state.agents.artifacts_popup_chat_input,
                state.agents.artifacts_popup_chat_cursor,
            );
            state.agents.artifacts_popup_chat_input.insert(insert_at, '\t');
            state.agents.artifacts_popup_chat_cursor += 1;
            state.agents.artifacts_popup_chat_selection_anchor = None;
            changed = true;
            follow_cursor = true;
        }
        // Regular character
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers,
            ..
        } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
            handled = true;
            delete_popup_chat_selection(state);
            let insert_at = chat_input_byte_index(
                &state.agents.artifacts_popup_chat_input,
                state.agents.artifacts_popup_chat_cursor,
            );
            state.agents.artifacts_popup_chat_input.insert(insert_at, c);
            state.agents.artifacts_popup_chat_cursor += 1;
            state.agents.artifacts_popup_chat_selection_anchor = None;
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

pub(super) fn artifacts_popup_scroll_metrics(
    state: &AppState,
    swarm: &SwarmRuntime,
    screen: Rect,
    theme: &Theme,
) -> (usize, usize) {
    let area = dynamic_popup_rect(screen, artifacts_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let content_height = artifacts_popup::content_area_height(
        state,
        swarm,
        area,
    ) as usize;
    let lines = artifacts_popup::build_lines(state, swarm, theme, text_area.width);
    (
        lines.len().saturating_sub(content_height),
        content_height.max(1),
    )
}

pub(super) fn close_artifacts_popup(state: &mut AppState) {
    state.agents.artifacts_popup_open = false;
    state.agents.artifacts_popup_scroll = 0;
    state.agents.artifacts_popup_chat_input.clear();
    state.agents.artifacts_popup_chat_cursor = 0;
    state.agents.artifacts_popup_chat_selection_anchor = None;
    state.agents.artifacts_popup_chat_scroll = usize::MAX;
    if let Some(selection) = state.ui_selection {
        if matches!(selection.pane, UiSelectionPane::ArtifactsPopup) {
            state.ui_selection = None;
        }
    }
}

/// Temporarily swap the artifacts popup chat fields into the main `chat_input` fields
/// so that shared helper functions (`handle_chat_input_editing_key`,
/// `submit_chat_input_and_dispatch`, etc.) operate on the popup's own state.
pub(super) fn swap_in_artifacts_popup_chat(state: &mut AppState) {
    std::mem::swap(
        &mut state.agents.chat_input,
        &mut state.agents.artifacts_popup_chat_input,
    );
    std::mem::swap(
        &mut state.agents.chat_input_cursor,
        &mut state.agents.artifacts_popup_chat_cursor,
    );
    std::mem::swap(
        &mut state.agents.chat_input_selection_anchor,
        &mut state.agents.artifacts_popup_chat_selection_anchor,
    );
    std::mem::swap(
        &mut state.agents.chat_input_scroll,
        &mut state.agents.artifacts_popup_chat_scroll,
    );
}

/// Swap the popup chat fields back out of the main `chat_input` fields.
/// Must be called after every `swap_in_artifacts_popup_chat`.
pub(super) fn swap_out_artifacts_popup_chat(state: &mut AppState) {
    // Same operation — swap is symmetric.
    swap_in_artifacts_popup_chat(state);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_artifacts_popup_key(
    key: &KeyEvent,
    state: &mut AppState,
    swarm: &mut SwarmRuntime,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    clipboard: &mut Option<Clipboard>,
    screen: Rect,
    theme: &Theme,
) -> bool {
    if !state.agents.artifacts_popup_open {
        return false;
    }
    if is_global_quit_key(key) {
        return false;
    }

    // Esc: close popup.
    if matches!(key.code, KeyCode::Esc) {
        close_artifacts_popup(state);
        return true;
    }

    // Content scrolling:
    //   Ctrl+Up / Ctrl+Down   — one line at a time
    //   PgUp / PgDown         — one page at a time
    //   Ctrl+Home / Ctrl+End  — jump to top / bottom
    // Plain Up/Down go to the text editor (cursor movement in the input).
    let (max_scroll, page_step) = artifacts_popup_scroll_metrics(state, swarm, screen, theme);
    match *key {
        KeyEvent {
            code: KeyCode::Up,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
            && !modifiers.contains(KeyModifiers::SHIFT) =>
        {
            bump_scroll_clamped(&mut state.agents.artifacts_popup_scroll, -1, max_scroll);
            return true;
        }
        KeyEvent {
            code: KeyCode::Down,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
            && !modifiers.contains(KeyModifiers::SHIFT) =>
        {
            bump_scroll_clamped(&mut state.agents.artifacts_popup_scroll, 1, max_scroll);
            return true;
        }
        KeyEvent {
            code: KeyCode::Home,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            state.agents.artifacts_popup_scroll = 0;
            return true;
        }
        KeyEvent {
            code: KeyCode::End,
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => {
            state.agents.artifacts_popup_scroll = max_scroll;
            return true;
        }
        KeyEvent {
            code: KeyCode::PageUp,
            ..
        } => {
            bump_scroll_clamped(
                &mut state.agents.artifacts_popup_scroll,
                -(page_step as i32),
                max_scroll,
            );
            return true;
        }
        KeyEvent {
            code: KeyCode::PageDown,
            ..
        } => {
            bump_scroll_clamped(
                &mut state.agents.artifacts_popup_scroll,
                page_step as i32,
                max_scroll,
            );
            return true;
        }
        _ => {}
    }

    // Enter (without Shift): submit prompt directly to the artifact's owning agent.
    // This bypasses the normal submit_chat_input_and_dispatch flow so that:
    // 1. The prompt goes to the artifact's agent (not the roster-selected agent).
    // 2. The agent dispatches immediately unless *it specifically* is busy — other
    //    agents being busy in the chat pane should not block this dispatch.
    // 3. The correct mission context (and thus thread/session ID) is used.
    if matches!(key.code, KeyCode::Enter) && !key.modifiers.contains(KeyModifiers::SHIFT) {
        let w = state.agents.ops_viewport_width;
        let artifact_agent = agent_ops_view::selected_artifact_agent_id(state, swarm, w);
        let artifact_mission = agent_ops_view::selected_artifact_mission_id(state, swarm, w);

        // Read the popup chat input.
        let prompt = state.agents.artifacts_popup_chat_input.trim().to_string();
        if prompt.is_empty() {
            return true;
        }

        if let Some(agent_id) = artifact_agent {
            // Push the prompt as a user message in the correct context.
            let prev_agent = state.agents.selected_agent.clone();
            let prev_mission = state.agents.selected_mission.clone();
            state.agents.selected_agent = Some(agent_id.clone());
            if artifact_mission.is_some() {
                state.agents.selected_mission = artifact_mission.clone();
            }
            swap_in_artifacts_popup_chat(state);
            let sent = push_chat_message(state);
            swap_out_artifacts_popup_chat(state);
            state.agents.selected_agent = prev_agent;
            state.agents.selected_mission = prev_mission;

            if let Some((_channel, prompt_text)) = sent {
                let prompt_msg_idx = state.agents.messages.len().saturating_sub(1);
                let mission_id = artifact_mission;

                // Dispatch directly to the artifact's agent. Only queue if
                // *this specific agent* is busy — other agents running in
                // the chat pane should not block this dispatch.
                let agent_busy = is_agent_busy(state, &agent_id);
                let is_claude = state
                    .agents
                    .agents
                    .iter()
                    .find(|lane| lane.id == agent_id)
                    .is_some_and(|lane| lane.is_claude());

                if agent_busy {
                    if is_claude {
                        enqueue_claude_turn(
                            state,
                            vitals,
                            Some(agent_id),
                            mission_id,
                            prompt_text,
                            Some(prompt_msg_idx),
                        );
                    } else {
                        enqueue_codex_turn(
                            state,
                            vitals,
                            Some(agent_id),
                            mission_id,
                            prompt_text,
                            Some(prompt_msg_idx),
                        );
                    }
                } else if is_claude {
                    state
                        .agents
                        .claude_turn_prompt_idx
                        .insert(agent_id.clone(), prompt_msg_idx);
                    maybe_dispatch_claude_turn(
                        state,
                        vitals,
                        claude,
                        Some(agent_id),
                        mission_id,
                        prompt_text,
                        true,
                    );
                } else {
                    state
                        .agents
                        .codex_turn_prompt_idx
                        .insert(agent_id.clone(), prompt_msg_idx);
                    maybe_dispatch_codex_turn(
                        state,
                        vitals,
                        codex,
                        Some(agent_id),
                        mission_id,
                        prompt_text,
                        true,
                    );
                }
            }

            close_artifacts_popup(state);
            state.agents.note_event();
            vitals.record_agent_event(Instant::now());
        } else {
            // No artifact agent resolved — fall back to normal submit path.
            swap_in_artifacts_popup_chat(state);
            if submit_chat_input_and_dispatch(state, vitals, codex, claude, swarm) {
                swap_out_artifacts_popup_chat(state);
                close_artifacts_popup(state);
                state.agents.note_event();
                vitals.record_agent_event(Instant::now());
            } else {
                swap_out_artifacts_popup_chat(state);
            }
        }
        return true;
    }

    // Decoupled text-editing handler — operates directly on popup fields.
    let edit = handle_artifacts_popup_chat_key(key, state, clipboard);

    if edit.changed {
        if edit.follow_cursor {
            state.agents.artifacts_popup_chat_scroll = usize::MAX;
        }
        state.agents.note_event();
        vitals.record_agent_event(Instant::now());
    }
    true // swallow all input while popup is open
}
