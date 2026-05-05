use arboard::Clipboard;
use nit_core::{
    AgentAlert, AgentAlertSeverity, AgentChannel, AgentMessage, AppState, MissionPhase,
    MissionRecord, PatchProposal, PatchStatus, YankKind,
};

use crate::syntax::SyntaxRuntime;

use super::*;

pub(crate) fn insert_chat_input_text(state: &mut AppState, text: &str) -> bool {
    let normalized = normalize_chat_input_text(text);
    if normalized.is_empty() {
        return false;
    }
    delete_chat_input_selection(state);
    let insert_at = chat_input_byte_index(&state.agents.chat_input, state.agents.chat_input_cursor);
    state.agents.chat_input.insert_str(insert_at, &normalized);
    state.agents.chat_input_cursor = state
        .agents
        .chat_input_cursor
        .saturating_add(normalized.chars().count());
    state.agents.chat_input_scroll = usize::MAX;
    state.agents.chat_input_selection_anchor = None;
    true
}

pub(super) fn normalize_chat_input_text(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if matches!(chars.peek(), Some('\n')) {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(ch);
        }
    }
    out
}

pub(super) fn normalize_buffer_input_text(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('\r') {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if matches!(chars.peek(), Some('\n')) {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(ch);
        }
    }
    std::borrow::Cow::Owned(out)
}

pub(super) fn chat_input_selection_range(state: &AppState) -> Option<(usize, usize)> {
    let total = state.agents.chat_input.chars().count();
    let cursor = state.agents.chat_input_cursor.min(total);
    let anchor = state.agents.chat_input_selection_anchor?.min(total);
    if anchor == cursor {
        return None;
    }
    Some((anchor.min(cursor), anchor.max(cursor)))
}

pub(super) fn delete_chat_input_selection(state: &mut AppState) -> bool {
    let Some((start, end)) = chat_input_selection_range(state) else {
        return false;
    };
    let remove_start = chat_input_byte_index(&state.agents.chat_input, start);
    let remove_end = chat_input_byte_index(&state.agents.chat_input, end);
    state
        .agents
        .chat_input
        .replace_range(remove_start..remove_end, "");
    state.agents.chat_input_cursor = start;
    state.agents.chat_input_selection_anchor = None;
    true
}

pub(crate) fn copy_chat_input_selection(
    state: &mut AppState,
    clipboard: &mut Option<Clipboard>,
) -> bool {
    let Some((start, end)) = chat_input_selection_range(state) else {
        return false;
    };
    let text = slice_by_char(&state.agents.chat_input, start, end);
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

pub(super) fn insert_text_into_focused_buffer(
    state: &mut AppState,
    syntax: &mut SyntaxRuntime,
    text: &str,
) -> bool {
    if text.is_empty() {
        return false;
    }
    let normalized = normalize_buffer_input_text(text);
    if normalized.is_empty() {
        return false;
    }
    let editor_id = state.active_editor_buffer_id;
    let notes_id = state.notes_buffer_id;
    let editor_version = state.editor_buffer().version();
    let notes_version = state.notes_buffer().version();
    {
        let Some(buffer) = state.focused_buffer_mut() else {
            return false;
        };
        buffer.break_undo_group();
        buffer.insert_str(normalized.as_ref());
    }
    if state.editor_buffer().version() != editor_version {
        let buf = state.editor_buffer_mut();
        syntax.note_buffer_change(editor_id, buf);
    }
    if state.notes_buffer().version() != notes_version {
        let buf = state.notes_buffer_mut();
        syntax.note_buffer_change(notes_id, buf);
    }
    true
}

pub(crate) fn chat_history_reset_nav(state: &mut AppState) {
    state.agents.chat_prompt_history_pos = None;
    state.agents.chat_prompt_history_draft = None;
}

pub(crate) fn chat_history_prev(state: &mut AppState) -> bool {
    if state.agents.chat_prompt_history.is_empty() {
        return false;
    }
    let next_pos = match state.agents.chat_prompt_history_pos {
        None => {
            state.agents.chat_prompt_history_draft = Some(state.agents.chat_input.clone());
            Some(state.agents.chat_prompt_history.len().saturating_sub(1))
        }
        Some(0) => None,
        Some(pos) => Some(pos.saturating_sub(1)),
    };
    let Some(pos) = next_pos else {
        return false;
    };
    if pos >= state.agents.chat_prompt_history.len() {
        chat_history_reset_nav(state);
        return false;
    }
    state.agents.chat_prompt_history_pos = Some(pos);
    state.agents.chat_input = state.agents.chat_prompt_history[pos].clone();
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    state.agents.chat_input_selection_anchor = None;
    true
}

pub(crate) fn chat_history_next(state: &mut AppState) -> bool {
    let Some(pos) = state.agents.chat_prompt_history_pos else {
        return false;
    };
    let history_len = state.agents.chat_prompt_history.len();
    if history_len == 0 || pos >= history_len {
        chat_history_reset_nav(state);
        return false;
    }
    if pos.saturating_add(1) < history_len {
        let next = pos.saturating_add(1);
        state.agents.chat_prompt_history_pos = Some(next);
        state.agents.chat_input = state.agents.chat_prompt_history[next].clone();
    } else {
        state.agents.chat_prompt_history_pos = None;
        state.agents.chat_input = state
            .agents
            .chat_prompt_history_draft
            .take()
            .unwrap_or_default();
    }
    state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
    state.agents.chat_input_selection_anchor = None;
    true
}

pub(super) fn chat_cursor_move_vertical(
    input: &str,
    cursor_char_idx: usize,
    direction: i8,
) -> usize {
    let total_chars = input.chars().count();
    let cursor = cursor_char_idx.min(total_chars);
    if input.is_empty() {
        return 0;
    }
    let line_starts = chat_line_starts(input);
    if line_starts.is_empty() {
        return cursor;
    }
    let current_line = line_starts
        .iter()
        .rposition(|start| *start <= cursor)
        .unwrap_or(0);
    let target_line = if direction < 0 {
        current_line.saturating_sub(1)
    } else {
        (current_line + 1).min(line_starts.len().saturating_sub(1))
    };
    if target_line == current_line {
        return cursor;
    }
    let current_start = line_starts[current_line];
    let current_len = chat_line_len(&line_starts, current_line, total_chars);
    let column = cursor.saturating_sub(current_start).min(current_len);
    let target_start = line_starts[target_line];
    let target_len = chat_line_len(&line_starts, target_line, total_chars);
    target_start + column.min(target_len)
}

pub(super) fn chat_line_starts(input: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (idx, ch) in input.chars().enumerate() {
        if ch == '\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

pub(super) fn chat_line_len(line_starts: &[usize], line_idx: usize, total_chars: usize) -> usize {
    let start = line_starts.get(line_idx).copied().unwrap_or(total_chars);
    let end = if let Some(next_start) = line_starts.get(line_idx + 1).copied() {
        next_start.saturating_sub(1)
    } else {
        total_chars
    };
    end.saturating_sub(start)
}

pub(super) fn chat_current_line_bounds(input: &str, cursor_char_idx: usize) -> (usize, usize) {
    let total_chars = input.chars().count();
    let cursor = cursor_char_idx.min(total_chars);
    if input.is_empty() {
        return (0, 0);
    }
    let line_starts = chat_line_starts(input);
    let line_idx = line_starts
        .iter()
        .rposition(|start| *start <= cursor)
        .unwrap_or(0);
    let start = line_starts.get(line_idx).copied().unwrap_or(0);
    let end = if let Some(next_start) = line_starts.get(line_idx + 1).copied() {
        next_start.saturating_sub(1)
    } else {
        total_chars
    };
    (start.min(total_chars), end.min(total_chars))
}

pub(super) fn chat_current_line_indent(input: &str, cursor_char_idx: usize) -> String {
    let (start, end) = chat_current_line_bounds(input, cursor_char_idx);
    let mut out = String::new();
    for (idx, ch) in input.chars().enumerate() {
        if idx >= end {
            break;
        }
        if idx >= start {
            if ch == ' ' || ch == '\t' {
                out.push(ch);
            } else {
                break;
            }
        }
    }
    out
}

pub(super) fn chat_is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-')
}

pub(super) fn chat_cursor_move_word_left(input: &str, cursor_char_idx: usize) -> usize {
    let chars: Vec<char> = input.chars().collect();
    let mut idx = cursor_char_idx.min(chars.len());
    while idx > 0 && chars[idx.saturating_sub(1)].is_whitespace() {
        idx = idx.saturating_sub(1);
    }
    let Some(prev) = idx.checked_sub(1).and_then(|pos| chars.get(pos).copied()) else {
        return idx;
    };
    if chat_is_word_char(prev) {
        while idx > 0 && chat_is_word_char(chars[idx.saturating_sub(1)]) {
            idx = idx.saturating_sub(1);
        }
        return idx;
    }
    while idx > 0
        && !chars[idx.saturating_sub(1)].is_whitespace()
        && !chat_is_word_char(chars[idx.saturating_sub(1)])
    {
        idx = idx.saturating_sub(1);
    }
    idx
}

pub(super) fn chat_cursor_move_word_right(input: &str, cursor_char_idx: usize) -> usize {
    let chars: Vec<char> = input.chars().collect();
    let mut idx = cursor_char_idx.min(chars.len());
    while idx < chars.len() && chars[idx].is_whitespace() {
        idx = idx.saturating_add(1);
    }
    let Some(next) = chars.get(idx).copied() else {
        return idx;
    };
    if chat_is_word_char(next) {
        while idx < chars.len() && chat_is_word_char(chars[idx]) {
            idx = idx.saturating_add(1);
        }
        return idx;
    }
    while idx < chars.len() && !chars[idx].is_whitespace() && !chat_is_word_char(chars[idx]) {
        idx = idx.saturating_add(1);
    }
    idx
}

pub(super) fn spawn_mock_mission(state: &mut AppState) {
    let mission_id = format!("mis-{:03}", state.agents.missions.len() + 1);
    let assigned_agents = if let Some(agent_id) = state.agents.selected_context_agent() {
        let mut agents = vec![agent_id.to_string()];
        for extra in state.agents.agents.iter().take(2) {
            if !agents.iter().any(|id| id == &extra.id) {
                agents.push(extra.id.clone());
            }
        }
        agents
    } else {
        state
            .agents
            .agents
            .iter()
            .take(2)
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>()
    };
    state.agents.missions.push(MissionRecord {
        id: mission_id.clone(),
        title: format!("Mission {}", state.agents.missions.len() + 1),
        phase: MissionPhase::Plan,
        swarm: assigned_agents.len() > 1,
        assigned_agents: assigned_agents.clone(),
        status: "QUEUED".into(),
        updated_at: timestamp_label(state),
    });
    state.agents.mission_selected = state.agents.missions.len().saturating_sub(1);
    state.agents.selected_mission = Some(mission_id.clone());
    let message_text = format!(
        "New mission queued with swarm agents: {}",
        assigned_agents.join(", ")
    );
    state.agents.messages.push(AgentMessage {
        at: timestamp_label(state),
        channel: AgentChannel::Broadcast,
        agent_id: None,
        mission_id: Some(mission_id.clone()),
        text: message_text.clone(),
        prompt_msg_idx: None,
        kind: None,
    });
    let delta = estimate_codex_context_tokens(&message_text);
    let entry = state
        .agents
        .codex_estimated_tokens_used_by_mission
        .entry(mission_id.clone())
        .or_insert(0);
    *entry = entry.saturating_add(delta);

    let patch_base = state.agents.patches.len() + 1;
    state.agents.patches.push(PatchProposal {
        id: format!("patch-{patch_base:03}"),
        mission_id: Some(mission_id.clone()),
        agent_id: assigned_agents
            .first()
            .cloned()
            .unwrap_or_else(|| "coder".into()),
        title: "Swarm proposal A".into(),
        summary: "Primary implementation candidate from lane A.".into(),
        diff: "diff --git a/src/lib.rs b/src/lib.rs\n@@ -1,2 +1,4 @@\n+// swarm proposal A\n"
            .into(),
        status: PatchStatus::New,
    });
    state.agents.patches.push(PatchProposal {
        id: format!("patch-{:03}", patch_base + 1),
        mission_id: Some(mission_id.clone()),
        agent_id: assigned_agents
            .get(1)
            .cloned()
            .unwrap_or_else(|| "reviewer".into()),
        title: "Swarm proposal B".into(),
        summary: "Alternative implementation from parallel lane.".into(),
        diff: "diff --git a/src/lib.rs b/src/lib.rs\n@@ -1,2 +1,4 @@\n+// swarm proposal B\n"
            .into(),
        status: PatchStatus::New,
    });
    state.agents.patch_selected = 0;
    state.agents.alerts.push(AgentAlert {
        severity: AgentAlertSeverity::Info,
        source: "mission".into(),
        message: format!("Created mission {mission_id}"),
        at: timestamp_label(state),
    });
    mark_mission_provenance_dirty(state, &mission_id);
}

pub(super) fn mark_mission_provenance_dirty(state: &mut AppState, mission_id: &str) {
    if state
        .agents
        .pending_provenance_mission_ids
        .iter()
        .all(|id| id != mission_id)
    {
        state
            .agents
            .pending_provenance_mission_ids
            .push(mission_id.to_string());
    }
}

pub(super) fn timestamp_label(state: &AppState) -> String {
    format!("t+{}", state.metrics.frame_count)
}
