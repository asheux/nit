use nit_core::{
    AgentConsoleRow as ThreadRow, AgentConsoleRowKind as ThreadRowKind, AgentConsoleRowsCacheKey,
    AgentLane, AgentLaneKind, AgentMessage, AgentStatus, AppState, PaneId, UiSelectionPane,
    CONSOLE_SCROLL_BOTTOM,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};
use std::time::{Duration, Instant};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::swarm::{chat_clone_base_id, SwarmRuntime};
use crate::theme::Theme;
use crate::widgets::{agent_ops_view, text_selection::apply_ui_selection};

pub struct ChatCursor {
    pub x: u16,
    pub y: u16,
}

pub struct ChatInputScrollMetrics {
    pub area: Rect,
    pub window_start: usize,
    pub visible_height: usize,
    pub max_scroll: usize,
    pub total_lines: usize,
}

const TAB_STOP: usize = 4;
const CHAT_INPUT_MAX_INNER_LINES: usize = 12;
const CHAT_INPUT_MAX_INNER_LINES_COMPACT: usize = 8;
const CHAT_INPUT_SCROLL_AUTO: usize = usize::MAX;
const AGENT_BADGE_MAX_CHARS: usize = 24;
const USER_PROMPT_BG_BACKGROUND_PCT: u8 = 80;

struct ConsoleLayout {
    thread_area: Rect,
    input_chunk: Rect,
    input_area: Rect,
    input_boxed: bool,
    input_lines_all: Vec<String>,
    cursor_line_all: usize,
    cursor_col_all: usize,
    input_inner_height: usize,
    input_window_start: usize,
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
) -> Option<ChatCursor> {
    let focused = state.focus == PaneId::Notes;
    let border_style = if focused {
        Style::default().fg(theme.border_focused)
    } else {
        Style::default().fg(theme.border)
    };
    let border_type = if focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    let title_color = if focused {
        theme.title_focused
    } else {
        theme.title
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            "AGENT CHAT  [ Enter send ]",
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(border_style)
        .border_type(border_type)
        .style(Style::default().bg(theme.background));
    frame.render_widget(block.clone(), area);

    let layout = compute_console_layout(area, state)?;
    let inner = block.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(layout.input_chunk.height),
        ])
        .split(inner);

    let mission = state.agents.selected_context_mission();
    let agent = state.agents.selected_context_agent();
    let codex_ctx_pct = agent.and_then(|agent_id| {
        let lane_kind = state
            .agents
            .agents
            .iter()
            .find(|lane| lane.id == agent_id)
            .map(|lane| lane.kind);
        match lane_kind {
            Some(AgentLaneKind::Codex) => {
                let pct = if let Some(mission_id) = mission {
                    state
                        .agents
                        .codex_mission_context_remaining_pct
                        .get(mission_id)
                        .and_then(|m| m.get(agent_id))
                        .copied()
                } else {
                    state
                        .agents
                        .codex_context_remaining_pct
                        .get(agent_id)
                        .copied()
                };
                Some(pct.unwrap_or(100))
            }
            Some(AgentLaneKind::Claude) => {
                let pct = if let Some(mission_id) = mission {
                    state
                        .agents
                        .claude_mission_context_remaining_pct
                        .get(mission_id)
                        .and_then(|m| m.get(agent_id))
                        .copied()
                } else {
                    state
                        .agents
                        .claude_context_remaining_pct
                        .get(agent_id)
                        .copied()
                };
                Some(pct.unwrap_or(100))
            }
            _ => None,
        }
    });
    let codex_ctx_used = agent.and_then(|agent_id| {
        let lane_kind = state
            .agents
            .agents
            .iter()
            .find(|lane| lane.id == agent_id)
            .map(|lane| lane.kind);
        match lane_kind {
            Some(AgentLaneKind::Codex) => {
                if let Some(mission_id) = mission {
                    state
                        .agents
                        .codex_mission_used_tokens
                        .get(mission_id)
                        .and_then(|m| m.get(agent_id))
                        .copied()
                } else {
                    state.agents.codex_used_tokens.get(agent_id).copied()
                }
            }
            Some(AgentLaneKind::Claude) => {
                if let Some(mission_id) = mission {
                    state
                        .agents
                        .claude_mission_used_tokens
                        .get(mission_id)
                        .and_then(|m| m.get(agent_id))
                        .copied()
                } else {
                    state.agents.claude_used_tokens.get(agent_id).copied()
                }
            }
            _ => None,
        }
    });
    let codex_ctx_max = agent.and_then(|agent_id| {
        state
            .agents
            .codex_effective_context_window_tokens
            .get(agent_id)
            .or_else(|| {
                state
                    .agents
                    .claude_effective_context_window_tokens
                    .get(agent_id)
            })
            .copied()
    });
    let label_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let mission_style = if mission.is_some() {
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD)
    } else {
        label_style
    };
    let context_line = Line::from(vec![
        Span::styled("mission=", label_style),
        Span::styled(mission.unwrap_or("--"), mission_style),
        Span::styled("     ", label_style),
        Span::styled("ctx=", label_style),
        Span::styled(
            if let Some(pct) = codex_ctx_pct {
                if let (Some(used), Some(max)) = (codex_ctx_used, codex_ctx_max) {
                    format!(
                        "{pct}% {}/{}",
                        format_token_count_short(used),
                        format_token_count_short(max)
                    )
                } else if let Some(max) = codex_ctx_max {
                    format!("{pct}%/{}", format_token_count_short(max))
                } else {
                    format!("{pct}%")
                }
            } else if let (Some(used), Some(max)) = (codex_ctx_used, codex_ctx_max) {
                format!(
                    "{}/{}",
                    format_token_count_short(used),
                    format_token_count_short(max)
                )
            } else if let Some(max) = codex_ctx_max {
                format_token_count_short(max)
            } else {
                "--".to_string()
            },
            if codex_ctx_pct.is_some() || codex_ctx_max.is_some() || codex_ctx_used.is_some() {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                label_style
            },
        ),
    ]);
    frame.render_widget(Paragraph::new(context_line), chunks[0]);

    let pulse_on = pulse_on(state);
    let thread_width = layout.thread_area.width.max(1) as usize;
    let (_cached_rows_len, _) = refresh_thread_rows_cache(state, Some(swarm), thread_width);
    let thread_height = layout.thread_area.height.max(1) as usize;

    // Build pending_by_prompt: prompt_msg_idx → agent_ids still working on it.
    let mut pending_by_prompt: std::collections::HashMap<usize, Vec<String>> =
        std::collections::HashMap::new();
    for (agent_id, &prompt_idx) in state
        .agents
        .codex_turn_prompt_idx
        .iter()
        .chain(state.agents.claude_turn_prompt_idx.iter())
    {
        let is_active = state.agents.active_turns.contains_key(agent_id)
            || state
                .agents
                .queued_codex_turns
                .iter()
                .any(|t| t.agent_id == *agent_id)
            || state
                .agents
                .queued_claude_turns
                .iter()
                .any(|t| t.agent_id == *agent_id);
        if is_active {
            pending_by_prompt
                .entry(prompt_idx)
                .or_default()
                .push(agent_id.clone());
        }
    }

    // Merge cached message rows with per-prompt inline breathers.
    let mut inline_shown = std::collections::HashSet::<String>::new();
    let mut combined_rows: Vec<ThreadRow> = Vec::new();
    {
        let slots = &state.agents.console_rows_cache.breather_slots;
        let cached = &state.agents.console_rows_cache.rows;
        let mut slot_iter = slots.iter().peekable();
        for (row_idx, row) in cached.iter().enumerate() {
            combined_rows.push(row.clone());
            // After each breather slot, inject the inline breather if agents are pending.
            while slot_iter
                .peek()
                .is_some_and(|&&(pos, _)| pos == row_idx + 1)
            {
                let Some(&(_, prompt_msg_idx)) = slot_iter.next() else {
                    break;
                };
                if let Some(agent_ids) = pending_by_prompt.get(&prompt_msg_idx) {
                    combined_rows.extend(inline_breather_rows(
                        state,
                        agent_ids,
                        pulse_on,
                        thread_width,
                    ));
                    for id in agent_ids {
                        inline_shown.insert(id.clone());
                    }
                }
            }
        }
        // Drain any remaining slots past the end of cached rows.
        for &(_, prompt_msg_idx) in slot_iter {
            if let Some(agent_ids) = pending_by_prompt.get(&prompt_msg_idx) {
                combined_rows.extend(inline_breather_rows(
                    state,
                    agent_ids,
                    pulse_on,
                    thread_width,
                ));
                for id in agent_ids {
                    inline_shown.insert(id.clone());
                }
            }
        }
    }

    // Global breather for agents not shown inline (swarm, legacy, etc.).
    let any_remaining = state.agents.agents.iter().any(|a| {
        !inline_shown.contains(&a.id)
            && (state.agents.active_turns.contains_key(&a.id)
                || state
                    .agents
                    .queued_codex_turns
                    .iter()
                    .any(|t| t.agent_id == a.id)
                || state
                    .agents
                    .queued_claude_turns
                    .iter()
                    .any(|t| t.agent_id == a.id))
    });
    let mission_ctx = state.agents.selected_context_mission();
    let has_swarm_context =
        mission_ctx.is_some_and(|mid| state.agents.missions.iter().any(|m| m.id == mid && m.swarm));
    if any_remaining || (has_swarm_context && inline_shown.is_empty()) {
        combined_rows.extend(breather_rows_for_user_prompt(
            state,
            Some(swarm),
            pulse_on,
            thread_width,
        ));
    }

    let total_rows = combined_rows.len();
    let max_scroll = total_rows.saturating_sub(thread_height);
    state.agents.console_max_scroll = max_scroll;
    state.agents.console_scroll = if state.agents.console_scroll == CONSOLE_SCROLL_BOTTOM {
        max_scroll
    } else {
        state.agents.console_scroll.min(max_scroll)
    };
    let scroll_usize = state.agents.console_scroll;
    let visible_rows = combined_rows.iter().skip(scroll_usize).take(thread_height);
    let visible: Vec<Line<'static>> = thread_lines(visible_rows, theme);
    let visible = apply_ui_selection(
        visible,
        state.ui_selection.as_ref(),
        UiSelectionPane::AgentConsole,
        theme.selection_bg,
        scroll_usize,
    );
    frame.render_widget(
        Paragraph::new(visible).style(Style::default().bg(theme.background)),
        layout.thread_area,
    );

    let input_visible = layout
        .input_lines_all
        .iter()
        .skip(layout.input_window_start)
        .take(layout.input_inner_height)
        .cloned()
        .collect::<Vec<_>>();
    let input_len_chars = state.agents.chat_input.chars().count();
    let input_cursor = state.agents.chat_input_cursor.min(input_len_chars);
    let selection_range = state
        .agents
        .chat_input_selection_anchor
        .map(|anchor| anchor.min(input_len_chars))
        .and_then(|anchor| {
            if anchor == input_cursor {
                None
            } else {
                Some((anchor.min(input_cursor), anchor.max(input_cursor)))
            }
        });
    let (sel_start_line, sel_start_col, sel_end_line, sel_end_col) = selection_range
        .map(|(start, end)| {
            let wrap_width = layout.input_area.width.max(1) as usize;
            let (start_line, start_col) =
                chat_input_display_pos_for_char_idx(&state.agents.chat_input, wrap_width, start);
            let (end_line, end_col) =
                chat_input_display_pos_for_char_idx(&state.agents.chat_input, wrap_width, end);
            (start_line, start_col, end_line, end_col)
        })
        .unwrap_or((0, 0, 0, 0));
    let input_visible = input_visible
        .into_iter()
        .enumerate()
        .map(|(idx, text)| {
            if selection_range.is_none() {
                return Line::from(text);
            }
            let line_idx = layout.input_window_start.saturating_add(idx);
            if line_idx < sel_start_line || line_idx > sel_end_line {
                return Line::from(text);
            }
            let line_len = text.chars().count();
            let (sel_start, sel_end) = if sel_start_line == sel_end_line {
                (sel_start_col, sel_end_col)
            } else if line_idx == sel_start_line {
                (sel_start_col, line_len)
            } else if line_idx == sel_end_line {
                (0, sel_end_col)
            } else {
                (0, line_len)
            };
            let sel_start = sel_start.min(line_len);
            let sel_end = sel_end.min(line_len);
            highlight_plain_line(&text, sel_start, sel_end, theme.selection_bg)
        })
        .collect::<Vec<_>>();
    let input_max_row = layout.input_inner_height.saturating_sub(1);
    if layout.input_boxed {
        let queued_count = state
            .agents
            .selected_context_agent()
            .and_then(|agent_id| state.agents.agents.iter().find(|a| a.id == agent_id))
            .map(|agent| {
                let running = state.agents.active_turns.contains_key(&agent.id);
                agent.queue_len.saturating_sub(usize::from(running))
            })
            .unwrap_or(0);
        let mut title_spans = Vec::new();
        title_spans.push(Span::styled(
            "CHAT BOX".to_string(),
            Style::default()
                .fg(if focused {
                    theme.border_focused
                } else {
                    theme.border
                })
                .add_modifier(Modifier::BOLD),
        ));
        let swarm_template = state.agents.swarm_default_template.trim();
        if !swarm_template.is_empty() {
            title_spans.push(Span::raw("  "));
            title_spans.push(Span::styled(
                format!(" t={swarm_template} "),
                Style::default()
                    .fg(theme.background)
                    .bg(theme.border_focused)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        let swarm_mission = state.agents.swarm_default_mission.trim();
        if !swarm_mission.is_empty() {
            title_spans.push(Span::raw("  "));
            title_spans.push(Span::styled(
                format!(" m={swarm_mission} "),
                Style::default()
                    .fg(theme.background)
                    .bg(theme.hl.operator)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if queued_count > 0 {
            title_spans.push(Span::raw("  "));
            let label = if queued_count > 1 {
                format!(" Queued {queued_count} ")
            } else {
                " Queued ".to_string()
            };
            title_spans.push(Span::styled(
                label,
                Style::default()
                    .fg(theme.background)
                    .bg(theme.seed.accent_2)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        let input_block = Block::default()
            .borders(Borders::ALL)
            .title(Line::from(title_spans))
            .border_style(if focused {
                Style::default().fg(theme.border_focused)
            } else {
                Style::default().fg(theme.border)
            })
            .border_type(if focused {
                BorderType::Rounded
            } else {
                BorderType::Plain
            })
            .style(Style::default().bg(theme.background));
        frame.render_widget(input_block, layout.input_chunk);
    }
    let input_bg = if focused {
        let mut bg = dim_bg_towards(theme.cursor_line_bg, theme.background, 75);
        if bg == theme.selection_bg {
            bg = theme.cursor_line_bg;
        }
        if bg == theme.selection_bg {
            bg = theme.background;
        }
        bg
    } else {
        let mut bg = dim_bg_towards(theme.cursor_line_bg, theme.background, 85);
        if bg == theme.selection_bg {
            bg = theme.background;
        }
        bg
    };
    frame.render_widget(
        Paragraph::new(input_visible)
            .style(Style::default().fg(theme.foreground).bg(input_bg))
            .wrap(Wrap { trim: false }),
        layout.input_area,
    );

    if focused && cursor_visible(state) {
        let cursor_visible_in_window = layout.cursor_line_all >= layout.input_window_start
            && layout.cursor_line_all
                < layout
                    .input_window_start
                    .saturating_add(layout.input_inner_height);
        if cursor_visible_in_window {
            let cursor_line_visible = layout
                .cursor_line_all
                .saturating_sub(layout.input_window_start);
            let max_col = layout.input_area.width.saturating_sub(1) as usize;
            let col = layout.cursor_col_all.min(max_col) as u16;
            let row = cursor_line_visible.min(input_max_row) as u16;
            return Some(ChatCursor {
                x: layout.input_area.x + col,
                y: layout.input_area.y + row,
            });
        }
    }
    None
}

pub fn thread_text_area(area: Rect, state: &AppState) -> Option<Rect> {
    compute_console_layout(area, state).map(|layout| layout.thread_area)
}

pub fn chat_input_text_area(area: Rect, state: &AppState) -> Option<Rect> {
    compute_console_layout(area, state).map(|layout| layout.input_area)
}

pub fn chat_input_scroll_metrics(area: Rect, state: &AppState) -> Option<ChatInputScrollMetrics> {
    let layout = compute_console_layout(area, state)?;
    let total_lines = layout.input_lines_all.len();
    let max_scroll = total_lines.saturating_sub(layout.input_inner_height);
    Some(ChatInputScrollMetrics {
        area: layout.input_area,
        window_start: layout.input_window_start.min(max_scroll),
        visible_height: layout.input_inner_height,
        max_scroll,
        total_lines,
    })
}

pub fn map_chat_input_point_to_cursor(
    area: Rect,
    state: &AppState,
    column: u16,
    row: u16,
    clamp: bool,
) -> Option<usize> {
    let layout = compute_console_layout(area, state)?;
    if !point_in_rect(column, row, layout.input_area) && !clamp {
        return None;
    }
    let total_lines = layout.input_lines_all.len();
    if total_lines == 0 {
        return Some(0);
    }
    let rel_row = if clamp {
        row.saturating_sub(layout.input_area.y)
            .min(layout.input_area.height.saturating_sub(1))
    } else {
        row.saturating_sub(layout.input_area.y)
    } as usize;
    let line_idx = layout
        .input_window_start
        .saturating_add(rel_row)
        .min(total_lines.saturating_sub(1));
    let rel_col = if clamp {
        column
            .saturating_sub(layout.input_area.x)
            .min(layout.input_area.width.saturating_sub(1))
    } else {
        column.saturating_sub(layout.input_area.x)
    } as usize;
    let max_col = UnicodeWidthStr::width(layout.input_lines_all[line_idx].as_str());
    let visual_col = rel_col.min(max_col);
    Some(chat_input_char_index_for_display_pos(
        &state.agents.chat_input,
        layout.input_area.width as usize,
        line_idx,
        visual_col,
    ))
}

pub fn thread_lines_for_selection(state: &AppState, width: usize) -> Vec<String> {
    thread_rows(state, None, width.max(1), pulse_on(state))
        .into_iter()
        .map(|row| row.text)
        .collect()
}

pub fn thread_lines_for_selection_with_swarm(
    state: &AppState,
    swarm: &SwarmRuntime,
    width: usize,
) -> Vec<String> {
    thread_rows(state, Some(swarm), width.max(1), pulse_on(state))
        .into_iter()
        .map(|row| row.text)
        .collect()
}

pub fn artifact_message_index_for_line(
    state: &AppState,
    width: usize,
    line_idx: usize,
) -> Option<usize> {
    artifact_message_index_for_line_with_swarm(state, None, width, line_idx)
}

pub fn artifact_message_index_for_line_with_swarm(
    state: &AppState,
    swarm: Option<&SwarmRuntime>,
    width: usize,
    line_idx: usize,
) -> Option<usize> {
    let width = width.max(1);
    let mission = state.agents.selected_context_mission();
    let agent = state.agents.selected_context_agent();

    // Must iterate messages in the same grouped order as thread_rows() so that
    // `line_idx` (which comes from the rendered output) lines up with our row count.
    let ordered = visible_messages_grouped(state, mission, agent);

    // Also account for inline breather rows that thread_rows() inserts after
    // each user prompt.
    let mut pending_by_prompt: std::collections::HashMap<usize, Vec<String>> =
        std::collections::HashMap::new();
    for (agent_id, &prompt_idx) in state
        .agents
        .codex_turn_prompt_idx
        .iter()
        .chain(state.agents.claude_turn_prompt_idx.iter())
    {
        let is_active = state.agents.active_turns.contains_key(agent_id)
            || state
                .agents
                .queued_codex_turns
                .iter()
                .any(|t| t.agent_id == *agent_id)
            || state
                .agents
                .queued_claude_turns
                .iter()
                .any(|t| t.agent_id == *agent_id);
        if is_active {
            pending_by_prompt
                .entry(prompt_idx)
                .or_default()
                .push(agent_id.clone());
        }
    }

    let mut row_cursor = 0usize;
    for &(msg_idx, msg) in &ordered {
        let rows = format_message_rows(state, swarm, msg, width);
        if let Some(artifact_offset) = rows
            .iter()
            .position(|row| matches!(row.kind, ThreadRowKind::ArtifactLink))
        {
            if row_cursor + artifact_offset == line_idx {
                return Some(msg_idx);
            }
        }
        row_cursor = row_cursor.saturating_add(rows.len());

        // After a user prompt, account for inline breather rows that thread_rows()
        // would insert (they shift all subsequent line indices).
        if msg.agent_id.is_none() {
            if let Some(agent_ids) = pending_by_prompt.get(&msg_idx) {
                let breather = inline_breather_rows(state, agent_ids, false, width);
                row_cursor = row_cursor.saturating_add(breather.len());
            }
        }
    }
    None
}

fn refresh_thread_rows_cache(
    state: &mut AppState,
    swarm: Option<&SwarmRuntime>,
    width: usize,
) -> (usize, bool) {
    let width = width.max(1);
    let mission_ref = state.agents.selected_context_mission();
    let agent_ref = if mission_ref.is_some() {
        None
    } else {
        state.agents.selected_context_agent()
    };
    let messages_len = state.agents.messages.len();

    if state
        .agents
        .console_rows_cache
        .key
        .as_ref()
        .is_some_and(|key| {
            key.width == width
                && key.messages_len == messages_len
                && key.mission.as_deref() == mission_ref
                && key.agent.as_deref() == agent_ref
        })
    {
        return (
            state.agents.console_rows_cache.rows.len(),
            state.agents.console_rows_cache.last_message_was_user,
        );
    }

    let ordered = visible_messages_grouped(state, mission_ref, agent_ref);

    let mut rows = Vec::new();
    let mut breather_slots: Vec<(usize, usize)> = Vec::new();
    let mut last_message_was_user = false;
    for &(msg_idx, msg) in &ordered {
        last_message_was_user = msg.agent_id.is_none();
        rows.extend(format_message_rows(state, swarm, msg, width));
        // Record the row position where an inline breather can be inserted.
        if msg.agent_id.is_none() {
            breather_slots.push((rows.len(), msg_idx));
        }
    }

    let key = AgentConsoleRowsCacheKey {
        width,
        mission: mission_ref.map(str::to_string),
        agent: agent_ref.map(str::to_string),
        messages_len,
        event_epoch: state.agents.event_epoch,
    };
    state.agents.console_rows_cache.key = Some(key);
    state.agents.console_rows_cache.rows = rows;
    state.agents.console_rows_cache.last_message_was_user = last_message_was_user;
    state.agents.console_rows_cache.breather_slots = breather_slots;
    (
        state.agents.console_rows_cache.rows.len(),
        state.agents.console_rows_cache.last_message_was_user,
    )
}

fn compute_console_layout(area: Rect, state: &AppState) -> Option<ConsoleLayout> {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    if inner.width < 4 || inner.height < 3 {
        return None;
    }
    let input_boxed = inner.height >= 5 && inner.width >= 8;
    let input_wrap_width = if input_boxed {
        inner.width.saturating_sub(2) as usize
    } else {
        inner.width as usize
    };
    let cursor_char_idx = state
        .agents
        .chat_input_cursor
        .min(state.agents.chat_input.chars().count());
    let (input_lines_all, cursor_line_all, cursor_col_all) = wrap_input_with_cursor(
        "",
        &state.agents.chat_input,
        cursor_char_idx,
        input_wrap_width,
    );
    let input_inner_height = input_inner_height_for(inner, input_boxed, input_lines_all.len());
    let input_height = if input_boxed {
        input_inner_height + 2
    } else {
        input_inner_height
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(input_height as u16),
        ])
        .split(inner);
    let input_area = if input_boxed {
        Block::default().borders(Borders::ALL).inner(chunks[2])
    } else {
        chunks[2]
    };
    let input_window_start = chat_input_window_start(
        state.agents.chat_input_scroll,
        input_lines_all.len(),
        input_inner_height,
        cursor_line_all,
    );
    Some(ConsoleLayout {
        thread_area: chunks[1],
        input_chunk: chunks[2],
        input_area,
        input_boxed,
        input_lines_all,
        cursor_line_all,
        cursor_col_all,
        input_inner_height,
        input_window_start,
    })
}

fn input_inner_height_for(inner: Rect, input_boxed: bool, input_lines_len: usize) -> usize {
    let input_lines_len = input_lines_len.max(1);
    if input_boxed {
        // Reserve one line for context and at least one for transcript.
        let max_inner_by_layout = inner.height.saturating_sub(4).max(1) as usize;
        let cap = CHAT_INPUT_MAX_INNER_LINES.min(max_inner_by_layout);
        input_lines_len.min(cap).max(1)
    } else {
        let max_inner_by_layout = inner.height.saturating_sub(2).max(1) as usize;
        let cap = CHAT_INPUT_MAX_INNER_LINES_COMPACT.min(max_inner_by_layout);
        input_lines_len.min(cap).max(1)
    }
}

pub(crate) fn chat_input_window_start(
    scroll: usize,
    total_lines: usize,
    visible_height: usize,
    cursor_line: usize,
) -> usize {
    let max_scroll = total_lines.saturating_sub(visible_height.max(1));
    if scroll == CHAT_INPUT_SCROLL_AUTO {
        cursor_line
            .saturating_sub(visible_height.saturating_sub(1))
            .min(max_scroll)
    } else {
        scroll.min(max_scroll)
    }
}

fn point_in_rect(x: u16, y: u16, rect: Rect) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

pub fn chat_input_char_index_for_display_pos(
    input: &str,
    width: usize,
    target_line: usize,
    target_col: usize,
) -> usize {
    let width = width.max(1);
    let mut boundaries = Vec::with_capacity(input.chars().count().saturating_add(1));
    let mut line = 0usize;
    let mut col = 0usize;
    let mut char_idx = 0usize;
    boundaries.push((line, col, char_idx));
    for ch in input.chars() {
        match ch {
            '\n' | '\r' => {
                line = line.saturating_add(1);
                col = 0;
            }
            '\t' => {
                let tab_width = next_tab_width(col, width);
                if col + tab_width > width {
                    line = line.saturating_add(1);
                    col = 0;
                }
                col += tab_width;
            }
            _ => {
                let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
                if col + ch_width > width {
                    line = line.saturating_add(1);
                    col = 0;
                }
                col += ch_width;
            }
        }
        char_idx += 1;
        boundaries.push((line, col, char_idx));
    }
    for (line, col, idx) in boundaries {
        if line > target_line || (line == target_line && col >= target_col) {
            return idx;
        }
    }
    input.chars().count()
}

pub(crate) fn chat_input_display_pos_for_char_idx(
    input: &str,
    width: usize,
    target_char_idx: usize,
) -> (usize, usize) {
    let width = width.max(1);
    let total_chars = input.chars().count();
    let target = target_char_idx.min(total_chars);

    let mut line = 0usize;
    let mut cell_col = 0usize;
    let mut char_col = 0usize;

    for (idx, ch) in input.chars().enumerate() {
        if idx == target {
            return (line, char_col);
        }
        match ch {
            '\n' | '\r' => {
                line = line.saturating_add(1);
                cell_col = 0;
                char_col = 0;
            }
            '\t' => {
                let tab_width = next_tab_width(cell_col, width);
                if cell_col + tab_width > width {
                    line = line.saturating_add(1);
                    cell_col = 0;
                    char_col = 0;
                }
                cell_col += tab_width;
                char_col += tab_width;
            }
            _ => {
                let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
                if cell_col + ch_width > width {
                    line = line.saturating_add(1);
                    cell_col = 0;
                    char_col = 0;
                }
                cell_col += ch_width;
                char_col += 1;
            }
        }
    }
    (line, char_col)
}

pub(crate) fn highlight_plain_line(
    text: &str,
    sel_start: usize,
    sel_end: usize,
    selection_bg: Color,
) -> Line<'static> {
    if sel_start >= sel_end {
        return Line::from(text.to_string());
    }
    let (left, rest) = split_at_char(text, sel_start);
    let (mid, right) = split_at_char(&rest, sel_end.saturating_sub(sel_start));
    let mut spans = Vec::new();
    if !left.is_empty() {
        spans.push(Span::raw(left));
    }
    if !mid.is_empty() {
        spans.push(Span::styled(mid, Style::default().bg(selection_bg)));
    }
    if !right.is_empty() {
        spans.push(Span::raw(right));
    }
    Line::from(spans)
}

fn split_at_char(input: &str, idx: usize) -> (String, String) {
    if idx == 0 {
        return ("".into(), input.to_string());
    }
    for (count, (byte_idx, _)) in input.char_indices().enumerate() {
        if count == idx {
            return (input[..byte_idx].to_string(), input[byte_idx..].to_string());
        }
    }
    (input.to_string(), "".into())
}

pub(crate) fn wrap_input_with_cursor(
    prefix: &str,
    input: &str,
    cursor_char_idx: usize,
    width: usize,
) -> (Vec<String>, usize, usize) {
    let width = width.max(1);
    let prefix_chars = prefix.chars().count();
    let cursor_abs_idx = prefix_chars.saturating_add(cursor_char_idx);
    let mut chars = Vec::with_capacity(prefix_chars + input.chars().count());
    chars.extend(prefix.chars());
    chars.extend(input.chars());

    let mut lines = vec![String::new()];
    let mut line_idx = 0usize;
    let mut col = 0usize;
    let mut cursor_line = 0usize;
    let mut cursor_col = 0usize;

    for (idx, ch) in chars.iter().enumerate() {
        if idx == cursor_abs_idx {
            cursor_line = line_idx;
            cursor_col = col;
        }
        match *ch {
            '\n' | '\r' => {
                lines.push(String::new());
                line_idx += 1;
                col = 0;
            }
            '\t' => {
                let tab_width = next_tab_width(col, width);
                if col + tab_width > width {
                    lines.push(String::new());
                    line_idx += 1;
                    col = 0;
                }
                lines[line_idx].push_str(&" ".repeat(tab_width));
                col += tab_width;
            }
            _ => {
                let ch_width = UnicodeWidthChar::width(*ch).unwrap_or(1).max(1);
                if col + ch_width > width {
                    lines.push(String::new());
                    line_idx += 1;
                    col = 0;
                }
                lines[line_idx].push(*ch);
                col += ch_width;
            }
        }
    }
    if cursor_abs_idx >= chars.len() {
        cursor_line = line_idx;
        cursor_col = col;
    }
    (lines, cursor_line, cursor_col)
}

fn next_tab_width(col: usize, width: usize) -> usize {
    let width = width.max(1);
    let to_stop = TAB_STOP - (col % TAB_STOP);
    to_stop.max(1).min(width)
}

fn thread_lines<'a>(
    rows: impl IntoIterator<Item = &'a ThreadRow>,
    theme: &Theme,
) -> Vec<Line<'static>> {
    rows.into_iter()
        .map(|row| match row.kind {
            ThreadRowKind::User => user_line_with_prompt_bg(&row.text, theme),
            ThreadRowKind::Agent => agent_line_with_accent_ecg(&row.text, theme),
            ThreadRowKind::ArtifactLink => artifact_link_line(&row.text, theme),
            ThreadRowKind::Breather => breather_line(&row.text, theme),
            ThreadRowKind::StatusHeader => status_header_line(&row.text, theme),
            ThreadRowKind::StatusRow => status_row_line(&row.text, theme),
            ThreadRowKind::StatusSubRow => status_sub_row_line(&row.text, theme),
        })
        .collect()
}

fn breather_line(text: &str, theme: &Theme) -> Line<'static> {
    let mut parts = text.splitn(2, ' ');
    let ecg = parts.next().unwrap_or_default();
    let rest = parts.next().unwrap_or_default();
    Line::from(vec![
        Span::styled(
            ecg.to_string(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if rest.is_empty() {
                String::new()
            } else {
                format!(" {rest}")
            },
            Style::default().fg(theme.foreground),
        ),
    ])
}

fn agent_line_with_accent_ecg(text: &str, theme: &Theme) -> Line<'static> {
    let mut spans = Vec::new();
    // Keep agent output distinct from user bubbles, but don't over-dim (hard to read in many
    // terminals). Use a brighter cyan tone from the theme.
    let agent_style = Style::default().fg(theme.title);
    let badge_style = Style::default()
        .fg(theme.warning)
        .add_modifier(Modifier::BOLD);

    let leading_spaces = text.bytes().take_while(|b| *b == b' ').count();
    if leading_spaces > 0 {
        spans.push(Span::styled(" ".repeat(leading_spaces), agent_style));
    }
    let body = text[leading_spaces..].to_string();
    let mut parts = body.splitn(2, ' ');
    let first = parts.next().unwrap_or_default();
    let rest = parts.next().unwrap_or_default();
    if looks_like_ecg(first) {
        spans.push(Span::styled(
            first.to_string(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
        if rest.is_empty() {
            return Line::from(spans);
        }

        // Highlight the agent/model badge (e.g. "[gpt-5.3-codex]") in orange so it's easy to
        // spot who spoke. Keep the surrounding text cyan so headers remain scannable.
        let rest = format!(" {rest}");
        let badge_start = rest.find('[');
        let badge_end =
            badge_start.and_then(|start| rest[start..].find(']').map(|rel| start + rel + 1));
        if let (Some(start), Some(end)) = (badge_start, badge_end) {
            let pre = rest[..start].to_string();
            let badge = rest[start..end].to_string();
            let post = rest[end..].to_string();
            if !pre.is_empty() {
                push_agent_text_with_inline_highlights(&mut spans, &pre, agent_style, theme);
            }
            spans.push(Span::styled(badge, badge_style));
            if !post.is_empty() {
                push_agent_text_with_inline_highlights(&mut spans, &post, agent_style, theme);
            }
        } else {
            push_agent_text_with_inline_highlights(&mut spans, &rest, agent_style, theme);
        }
    } else if first.starts_with('[') && first.ends_with(']') {
        spans.push(Span::styled(first.to_string(), badge_style));
        if rest.is_empty() {
            return Line::from(spans);
        }
        let rest = format!(" {rest}");
        push_agent_text_with_inline_highlights(&mut spans, &rest, agent_style, theme);
    } else {
        push_agent_text_with_inline_highlights(&mut spans, &body, agent_style, theme);
    }
    Line::from(spans)
}

fn artifact_link_line(text: &str, theme: &Theme) -> Line<'static> {
    let leading_spaces = text.bytes().take_while(|b| *b == b' ').count();
    let body = &text[leading_spaces..];
    let prompt_bg = user_prompt_bg(theme);
    let body_style = Style::default()
        .fg(theme.foreground)
        .bg(prompt_bg)
        .add_modifier(Modifier::BOLD);
    let accent_style = Style::default()
        .fg(theme.background)
        .bg(theme.title_focused)
        .add_modifier(Modifier::BOLD);

    let mut spans = Vec::new();
    if leading_spaces > 0 {
        spans.push(Span::styled(" ".repeat(leading_spaces), body_style));
    }
    if let Some(start) = body.find("ARTIFACTS") {
        let end = start + "ARTIFACTS".len();
        if start > 0 {
            spans.push(Span::styled(body[..start].to_string(), body_style));
        }
        spans.push(Span::styled(body[start..end].to_string(), accent_style));
        if end < body.len() {
            spans.push(Span::styled(body[end..].to_string(), body_style));
        }
    } else {
        spans.push(Span::styled(body.to_string(), body_style));
    }

    Line::from(spans)
}

fn push_agent_text_with_inline_highlights(
    spans: &mut Vec<Span<'static>>,
    text: &str,
    base: Style,
    theme: &Theme,
) {
    if text.is_empty() {
        return;
    }
    // Avoid confusing the inline-code highlighter with fenced code blocks.
    if text.trim_start().starts_with("```") {
        spans.push(Span::styled(
            text.to_string(),
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::DIM),
        ));
        return;
    }
    if !text.contains('`') {
        push_agent_plain_text_with_token_highlights(spans, text, base, theme);
        return;
    }

    let tick_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let mut remaining = text;
    while let Some(start) = remaining.find('`') {
        let (before, after_tick) = remaining.split_at(start);
        if !before.is_empty() {
            push_agent_plain_text_with_token_highlights(spans, before, base, theme);
        }

        // `after_tick` begins with '`'
        let after_tick = &after_tick[1..];
        let Some(end) = after_tick.find('`') else {
            spans.push(Span::styled("`".to_string(), tick_style));
            push_agent_plain_text_with_token_highlights(spans, after_tick, base, theme);
            return;
        };

        let code = &after_tick[..end];
        let after = &after_tick[end + 1..];
        spans.push(Span::styled("`".to_string(), tick_style));
        spans.push(Span::styled(
            code.to_string(),
            inline_code_style(code, theme),
        ));
        spans.push(Span::styled("`".to_string(), tick_style));
        remaining = after;
    }
    if !remaining.is_empty() {
        push_agent_plain_text_with_token_highlights(spans, remaining, base, theme);
    }
}

fn push_agent_plain_text_with_token_highlights(
    spans: &mut Vec<Span<'static>>,
    text: &str,
    base: Style,
    theme: &Theme,
) {
    if text.is_empty() {
        return;
    }
    if push_verify_result_with_token_highlights(spans, text, base, theme) {
        return;
    }
    push_agent_plain_text_with_token_highlights_inner(spans, text, base, theme);
}

fn push_verify_result_with_token_highlights(
    spans: &mut Vec<Span<'static>>,
    text: &str,
    base: Style,
    theme: &Theme,
) -> bool {
    const VERIFY_PREFIX: &str = "VERIFY result:";
    let Some(prefix_pos) = text.find(VERIFY_PREFIX) else {
        return false;
    };

    let after_prefix = &text[prefix_pos + VERIFY_PREFIX.len()..];
    let Some((rel_start, _)) = after_prefix
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
    else {
        return false;
    };
    let outcome_start = prefix_pos + VERIFY_PREFIX.len() + rel_start;

    let mut outcome_end = outcome_start;
    for (idx, ch) in text[outcome_start..].char_indices() {
        if !ch.is_ascii_alphabetic() {
            break;
        }
        outcome_end = outcome_start + idx + ch.len_utf8();
    }
    if outcome_end == outcome_start {
        return false;
    }

    let before = &text[..outcome_start];
    let outcome = &text[outcome_start..outcome_end];
    let after = &text[outcome_end..];

    push_agent_plain_text_with_token_highlights_inner(spans, before, base, theme);
    spans.push(Span::styled(
        outcome.to_string(),
        verify_outcome_style(outcome, theme),
    ));
    push_agent_plain_text_with_token_highlights_inner(spans, after, base, theme);
    true
}

fn verify_outcome_style(outcome: &str, theme: &Theme) -> Style {
    let upper = outcome.trim().to_ascii_uppercase();
    let color = if matches!(upper.as_str(), "PASS" | "SUCCESS") {
        theme.success
    } else if matches!(upper.as_str(), "FAIL" | "FAILED") {
        theme.error
    } else {
        theme.warning
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn push_agent_plain_text_with_token_highlights_inner(
    spans: &mut Vec<Span<'static>>,
    text: &str,
    base: Style,
    theme: &Theme,
) {
    if text.is_empty() {
        return;
    }
    let mut idx = 0usize;
    while idx < text.len() {
        let ch = text[idx..].chars().next().unwrap_or(' ');
        let is_token = ch.is_ascii_alphanumeric()
            || matches!(ch, '_' | '-' | '.' | '/' | '\\' | ':' | '#' | '%');
        let start = idx;
        if is_token {
            while idx < text.len() {
                let ch = text[idx..].chars().next().unwrap_or(' ');
                let keep = ch.is_ascii_alphanumeric()
                    || matches!(ch, '_' | '-' | '.' | '/' | '\\' | ':' | '#' | '%');
                if !keep {
                    break;
                }
                idx += ch.len_utf8();
            }
            let token = &text[start..idx];
            let style = if looks_like_link_or_path_ref(token) {
                Style::default()
                    .fg(theme.hl.link)
                    .add_modifier(Modifier::UNDERLINED)
            } else if looks_like_numberish(token) {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else if looks_like_command(token) {
                Style::default().fg(theme.hl.operator)
            } else {
                base
            };
            spans.push(Span::styled(token.to_string(), style));
        } else {
            while idx < text.len() {
                let ch = text[idx..].chars().next().unwrap_or(' ');
                let is_token = ch.is_ascii_alphanumeric()
                    || matches!(ch, '_' | '-' | '.' | '/' | '\\' | ':' | '#' | '%');
                if is_token {
                    break;
                }
                idx += ch.len_utf8();
            }
            let sep = &text[start..idx];
            spans.push(Span::styled(sep.to_string(), base));
        }
    }
}

fn inline_code_style(code: &str, theme: &Theme) -> Style {
    let code = code.trim();
    if looks_like_link_or_path_ref(code) {
        Style::default()
            .fg(theme.hl.link)
            .add_modifier(Modifier::UNDERLINED)
    } else if looks_like_numberish(code) {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else if looks_like_command(code) {
        Style::default().fg(theme.hl.operator)
    } else {
        Style::default().fg(theme.foreground)
    }
}

fn looks_like_numberish(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    let text = text.trim_matches(|c: char| matches!(c, ',' | '.' | ')' | ']' | '}'));
    if text.is_empty() {
        return false;
    }

    let mut saw_digit = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            saw_digit = true;
            continue;
        }
        if matches!(ch, '.' | ',' | '%' | '+' | '-') {
            continue;
        }
        if saw_digit && ch.is_ascii_alphabetic() {
            continue;
        }
        return false;
    }
    saw_digit
}

fn looks_like_link_or_path_ref(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    if text.starts_with("http://") || text.starts_with("https://") {
        return true;
    }
    if let Some(pos) = text.find("#L") {
        if text[pos + 2..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
        {
            return true;
        }
    }

    if contains_colon_number(text)
        && (text.contains('/') || text.contains('\\') || text.contains('.'))
    {
        return true;
    }

    let has_path_sep = text.contains('/') || text.contains('\\');
    has_path_sep && looks_like_file_path(text)
}

fn contains_colon_number(text: &str) -> bool {
    for (idx, ch) in text.char_indices() {
        if ch != ':' {
            continue;
        }
        // Skip Windows drive letter colon (e.g. C:\foo\bar.txt:12).
        if idx == 1 && text.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            continue;
        }
        let after = &text[idx + 1..];
        let mut digits = 0usize;
        for ch in after.chars() {
            if ch.is_ascii_digit() {
                digits += 1;
            } else {
                break;
            }
        }
        if digits > 0 {
            return true;
        }
    }
    false
}

fn looks_like_file_path(text: &str) -> bool {
    let Some(sep) = text.rfind(['/', '\\']) else {
        return false;
    };
    let tail = &text[sep + 1..];
    tail.contains('.')
}

fn looks_like_command(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    if text.contains('\n') || text.contains('\r') {
        return true;
    }
    if text.contains(' ') || text.contains('\t') {
        return true;
    }
    matches!(
        text.to_ascii_lowercase().as_str(),
        "cargo"
            | "git"
            | "rg"
            | "sed"
            | "npm"
            | "yarn"
            | "pnpm"
            | "python"
            | "python3"
            | "node"
            | "go"
            | "make"
            | "docker"
            | "kubectl"
            | "codex"
            | "nit"
            | "curl"
            | "wget"
            | "bash"
            | "zsh"
    )
}

fn user_line_with_prompt_bg(text: &str, theme: &Theme) -> Line<'static> {
    if text.is_empty() {
        return Line::from(Span::styled(String::new(), Style::default()));
    }

    let prompt_bg = user_prompt_bg(theme);

    // User prompts are rendered as a padded block with a subtle background instead of ASCII
    // borders. Pad spaces are included in the string so the background fills the whole row.
    let base = Style::default().fg(Color::Gray).bg(prompt_bg);
    let label_style = base.add_modifier(Modifier::BOLD);

    let trimmed = text.trim();
    if trimmed != "You" {
        return Line::from(Span::styled(text.to_string(), base));
    }

    // Highlight just the "You" label, while keeping the rest of the row padded/backgrounded.
    let indent = text.bytes().take_while(|b| *b == b' ').count();
    let mut spans = Vec::new();
    if indent > 0 {
        spans.push(Span::styled(" ".repeat(indent), base));
    }
    spans.push(Span::styled("You".to_string(), label_style));
    let rest_len = text
        .chars()
        .count()
        .saturating_sub(indent)
        .saturating_sub(3);
    if rest_len > 0 {
        spans.push(Span::styled(" ".repeat(rest_len), base));
    }
    Line::from(spans)
}

fn user_prompt_bg(theme: &Theme) -> Color {
    dim_bg_towards(
        theme.cursor_line_bg,
        theme.background,
        USER_PROMPT_BG_BACKGROUND_PCT,
    )
}

fn status_header_line(text: &str, theme: &Theme) -> Line<'static> {
    let trimmed = text.trim();
    let is_rule = !trimmed.is_empty() && trimmed.chars().all(|ch| ch == '─');
    let bg = dim_bg_towards(theme.border, theme.background, 85);
    if is_rule {
        return Line::from(Span::styled(
            text.to_string(),
            Style::default()
                .fg(theme.border)
                .bg(bg)
                .add_modifier(Modifier::DIM),
        ));
    }
    Line::from(Span::styled(
        text.to_string(),
        Style::default()
            .fg(theme.border)
            .bg(bg)
            .add_modifier(Modifier::DIM),
    ))
}

fn status_row_line(text: &str, theme: &Theme) -> Line<'static> {
    if text.trim().is_empty() {
        let bg = dim_bg_towards(theme.border, theme.background, 85);
        return Line::from(Span::styled(text.to_string(), Style::default().bg(bg)));
    }
    let bg = dim_bg_towards(theme.border, theme.background, 85);
    Line::from(Span::styled(
        text.to_string(),
        Style::default().fg(theme.foreground).bg(bg),
    ))
}

fn status_sub_row_line(text: &str, theme: &Theme) -> Line<'static> {
    let trimmed = text.trim_start();
    let bg = dim_bg_towards(theme.border, theme.background, 85);
    let is_swarm_meta =
        trimmed.starts_with("Swarm") || trimmed.starts_with("Verify") || trimmed.starts_with("• ");
    if is_swarm_meta {
        return Line::from(Span::styled(
            text.to_string(),
            Style::default()
                .fg(theme.title)
                .bg(bg)
                .add_modifier(Modifier::DIM),
        ));
    }
    // Genome retry header: "↳ genome retry 1/10"
    if let Some(genome_rest) = trimmed.strip_prefix("\u{21b3} ") {
        if genome_rest.starts_with("genome retry") {
            let leading = text.len() - text.trim_start().len();
            let indent: String = text.chars().take(leading).collect();
            return Line::from(vec![
                Span::styled(
                    format!("{indent}\u{21b3} "),
                    Style::default().fg(theme.warning).bg(bg),
                ),
                Span::styled(
                    genome_rest.to_string(),
                    Style::default()
                        .fg(theme.warning)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
        }
    }
    // Genome retry file row: "  ↓ mod.rs III Failing c=0.33"
    if trimmed.starts_with("\u{2193} ")
        || trimmed.starts_with("\u{2191} ")
        || trimmed.starts_with("\u{2014} ")
        || trimmed.starts_with("+ ")
    {
        let leading = text.len() - text.trim_start().len();
        let indent: String = text.chars().take(leading).collect();
        let arrow = &trimmed[..trimmed.char_indices().nth(1).map(|(i, _)| i).unwrap_or(1)];
        let rest = trimmed[arrow.len()..].trim_start();
        let arrow_color = match arrow {
            "\u{2191}" => theme.success, // ↑ improved
            "\u{2193}" => theme.error,   // ↓ degraded
            "+" => theme.title_focused,  // + new
            _ => theme.border,           // — unchanged
        };
        // Color the quality label if present.
        let quality_color = if rest.contains("Failing") {
            theme.error
        } else if rest.contains("Minimum") {
            theme.warning
        } else if rest.contains("Standard") {
            theme.foreground
        } else if rest.contains("Excellent") {
            theme.title_focused
        } else if rest.contains("Exceptional") {
            theme.success
        } else {
            theme.foreground
        };
        return Line::from(vec![
            Span::styled(
                format!("{indent}{arrow} "),
                Style::default().fg(arrow_color).bg(bg),
            ),
            Span::styled(rest.to_string(), Style::default().fg(quality_color).bg(bg)),
        ]);
    }
    // Genome shadow stage: "↳ file.rs Quality delta (tier N, c=X.XX)"
    // Color-code the file name based on quality delta.
    if let Some(genome_rest) = trimmed.strip_prefix("↳ ") {
        if genome_rest.contains("(tier ") {
            let leading = text.len() - text.trim_start().len();
            let indent: String = text.chars().take(leading).collect();
            if let Some(space_idx) = genome_rest.find(' ') {
                let file_name = &genome_rest[..space_idx];
                let rest = &genome_rest[space_idx..];
                let file_color = if rest.contains("improved") {
                    theme.accent
                } else if rest.contains("degraded") {
                    theme.error
                } else if rest.contains("new") {
                    theme.title
                } else {
                    theme.foreground
                };
                return Line::from(vec![
                    Span::styled(
                        format!("{indent}↳ "),
                        Style::default()
                            .fg(theme.title)
                            .bg(bg)
                            .add_modifier(Modifier::DIM),
                    ),
                    Span::styled(
                        file_name.to_string(),
                        Style::default().fg(file_color).bg(bg),
                    ),
                    Span::styled(
                        rest.to_string(),
                        Style::default()
                            .fg(theme.title)
                            .bg(bg)
                            .add_modifier(Modifier::DIM),
                    ),
                ]);
            }
        }
    }
    Line::from(Span::styled(
        text.to_string(),
        Style::default()
            .fg(theme.title)
            .bg(bg)
            .add_modifier(Modifier::DIM),
    ))
}

fn looks_like_ecg(token: &str) -> bool {
    let mut count = 0usize;
    for ch in token.chars() {
        count += 1;
        if !matches!(ch, '▁' | '▂' | '▃' | '▄' | '▅' | '▆' | '▇' | '█') {
            return false;
        }
    }
    count == 6
}

pub(crate) fn dim_bg_towards(color: Color, background: Color, background_pct: u8) -> Color {
    let pct = background_pct.min(100) as u16;
    match (color, background) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r0, g0, b0)) => {
            let inv = 100u16.saturating_sub(pct);
            let mix = |top: u8, base: u8| -> u8 {
                let top = top as u16;
                let base = base as u16;
                ((top.saturating_mul(inv) + base.saturating_mul(pct) + 50) / 100) as u8
            };
            Color::Rgb(mix(r1, r0), mix(g1, g0), mix(b1, b0))
        }
        _ => color,
    }
}

fn thread_rows(
    state: &AppState,
    swarm: Option<&SwarmRuntime>,
    width: usize,
    pulse_on: bool,
) -> Vec<ThreadRow> {
    let mission = state.agents.selected_context_mission();
    let agent = state.agents.selected_context_agent();

    let ordered = visible_messages_grouped(state, mission, agent);

    // Build reverse map: prompt_msg_idx → list of agent_ids still working on it.
    let mut pending_by_prompt: std::collections::HashMap<usize, Vec<String>> =
        std::collections::HashMap::new();
    for (agent_id, &prompt_idx) in state
        .agents
        .codex_turn_prompt_idx
        .iter()
        .chain(state.agents.claude_turn_prompt_idx.iter())
    {
        let is_active = state.agents.active_turns.contains_key(agent_id)
            || state
                .agents
                .queued_codex_turns
                .iter()
                .any(|t| t.agent_id == *agent_id)
            || state
                .agents
                .queued_claude_turns
                .iter()
                .any(|t| t.agent_id == *agent_id);
        if is_active {
            pending_by_prompt
                .entry(prompt_idx)
                .or_default()
                .push(agent_id.clone());
        }
    }

    let mut inline_shown = std::collections::HashSet::<String>::new();
    let mut rows = Vec::new();

    for &(msg_idx, msg) in &ordered {
        rows.extend(format_message_rows(state, swarm, msg, width));

        // After a user prompt, show inline breather for pending agents.
        if msg.agent_id.is_none() {
            if let Some(prompt_agents) = pending_by_prompt.get(&msg_idx) {
                rows.extend(inline_breather_rows(state, prompt_agents, pulse_on, width));
                for id in prompt_agents {
                    inline_shown.insert(id.clone());
                }
            }
        }
    }

    // Only show the global breather for active agents NOT already shown inline
    // (e.g. swarm agents, or agents dispatched before prompt tracking was added).
    let any_remaining = state.agents.agents.iter().any(|a| {
        !inline_shown.contains(&a.id)
            && (state.agents.active_turns.contains_key(&a.id)
                || state
                    .agents
                    .queued_codex_turns
                    .iter()
                    .any(|t| t.agent_id == a.id)
                || state
                    .agents
                    .queued_claude_turns
                    .iter()
                    .any(|t| t.agent_id == a.id))
            && if let Some(sel) = agent {
                a.id == sel || chat_clone_base_id(&a.id) == Some(sel)
            } else {
                true
            }
    });
    // Also show global breather for swarm mission status (Done/Waiting/etc.) even
    // when no agents are active, so the completion state remains visible.
    let has_swarm_context =
        mission.is_some_and(|mid| state.agents.missions.iter().any(|m| m.id == mid && m.swarm));
    if any_remaining || has_swarm_context {
        rows.extend(breather_rows_for_user_prompt(state, swarm, pulse_on, width));
    }

    // Ensure artifact/done callouts never appear after the last breather.
    // This can happen when a turn completes and a queued prompt dequeues
    // simultaneously — the "done" reply gets pushed after the new prompt.
    hoist_stale_callouts_above_breather(&mut rows);

    rows
}

/// Move any ArtifactLink / StatusSubRow / Agent-done rows that appear after
/// the last breather block to just before the breather.
fn hoist_stale_callouts_above_breather(rows: &mut Vec<ThreadRow>) {
    // Find the start of the last breather block (StatusHeader row).
    let breather_start = rows
        .iter()
        .rposition(|r| matches!(r.kind, ThreadRowKind::StatusHeader));
    let Some(breather_idx) = breather_start else {
        return;
    };
    // Collect indices of callout rows that appear after the breather.
    let stale: Vec<usize> = (breather_idx + 1..rows.len())
        .filter(|&i| {
            matches!(
                rows[i].kind,
                ThreadRowKind::ArtifactLink | ThreadRowKind::Agent
            )
        })
        .collect();
    if stale.is_empty() {
        return;
    }
    // Extract stale rows (reverse order to preserve indices).
    let mut hoisted: Vec<ThreadRow> = Vec::with_capacity(stale.len());
    for &idx in stale.iter().rev() {
        hoisted.push(rows.remove(idx));
    }
    hoisted.reverse();
    // Insert before the breather.
    let insert_at = rows
        .iter()
        .rposition(|r| matches!(r.kind, ThreadRowKind::StatusHeader))
        .unwrap_or(rows.len());
    for (offset, row) in hoisted.into_iter().enumerate() {
        rows.insert(insert_at + offset, row);
    }
}

/// Returns visible messages in grouped order: each user prompt is immediately
/// followed by any agent responses whose `prompt_msg_idx` points to it.
/// Responses without a matching visible parent appear in their chronological position.
fn visible_messages_grouped<'a>(
    state: &'a AppState,
    mission: Option<&str>,
    agent: Option<&str>,
) -> Vec<(usize, &'a AgentMessage)> {
    let visible: Vec<(usize, &AgentMessage)> = state
        .agents
        .messages
        .iter()
        .enumerate()
        .filter(|(_, msg)| message_matches_context(msg, mission, agent))
        .collect();

    let visible_prompt_indices: std::collections::HashSet<usize> = visible
        .iter()
        .filter(|(_, msg)| msg.agent_id.is_none())
        .map(|(idx, _)| *idx)
        .collect();

    let mut responses_by_prompt: std::collections::HashMap<usize, Vec<(usize, &AgentMessage)>> =
        std::collections::HashMap::new();
    let mut grouped_indices: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for &(msg_idx, msg) in &visible {
        if msg.agent_id.is_some() {
            if let Some(parent_idx) = msg.prompt_msg_idx {
                if visible_prompt_indices.contains(&parent_idx) {
                    responses_by_prompt
                        .entry(parent_idx)
                        .or_default()
                        .push((msg_idx, msg));
                    grouped_indices.insert(msg_idx);
                }
            }
        }
    }

    let mut result = Vec::with_capacity(visible.len());
    for &(msg_idx, msg) in &visible {
        if grouped_indices.contains(&msg_idx) {
            continue;
        }
        result.push((msg_idx, msg));
        if msg.agent_id.is_none() {
            if let Some(responses) = responses_by_prompt.get(&msg_idx) {
                result.extend(responses.iter().copied());
            }
        }
    }
    result
}

fn message_matches_context(msg: &AgentMessage, mission: Option<&str>, agent: Option<&str>) -> bool {
    if let Some(mission_id) = mission {
        return msg.mission_id.as_deref() == Some(mission_id)
            || matches!(msg.channel, nit_core::AgentChannel::Broadcast);
    }
    if let Some(agent_id) = agent {
        return msg.agent_id.as_deref() == Some(agent_id)
            || msg
                .agent_id
                .as_deref()
                .is_some_and(|id| chat_clone_base_id(id) == Some(agent_id))
            || msg.agent_id.is_none()
            || matches!(msg.channel, nit_core::AgentChannel::Broadcast);
    }
    true
}

fn format_message_rows(
    state: &AppState,
    swarm: Option<&SwarmRuntime>,
    msg: &AgentMessage,
    width: usize,
) -> Vec<ThreadRow> {
    let width = width.max(1);
    let text_lines: Vec<&str> = if msg.text.is_empty() {
        vec![""]
    } else {
        msg.text.split('\n').collect()
    };
    if msg.agent_id.is_none() {
        let bubble = format_user_bubble_rows(msg, &text_lines, width);
        return bubble
            .into_iter()
            .map(|text| ThreadRow {
                text,
                kind: ThreadRowKind::User,
            })
            .collect::<Vec<_>>();
    }

    // Swarm meta is shown in the "Working ..." table footer when in swarm mission context, so
    // don't also render it as a transcript message.
    if msg.agent_id.as_deref() == Some("swarm") && msg.text.starts_with("Swarm ") {
        return Vec::new();
    }
    // Swarm broadcast messages are redundant when individual agent callouts
    // already cover each clone's completion.
    if msg.agent_id.as_deref() == Some("swarm")
        && matches!(msg.channel, nit_core::AgentChannel::Broadcast)
    {
        return Vec::new();
    }

    // Genome retry messages render as a compact multi-line table.
    if msg.kind.as_deref() == Some("genome-retry") {
        return msg
            .text
            .lines()
            .map(|line| ThreadRow {
                text: pad_line_right(line, width),
                kind: ThreadRowKind::StatusSubRow,
            })
            .collect();
    }

    let src = msg.agent_id.as_deref().unwrap_or("agent");
    let agent_badge = agent_identity_badge(state, src);
    let mut header = format!("[{agent_badge}]");
    if matches!(msg.channel, nit_core::AgentChannel::Broadcast) {
        header.push_str(" @all");
    }

    let indent_str = "";

    let mut out = Vec::new();
    let artifact_target = state
        .agents
        .messages
        .iter()
        .enumerate()
        .find_map(|(idx, candidate)| std::ptr::eq(candidate, msg).then_some(idx))
        .and_then(|message_idx| {
            agent_ops_view::artifacts_popup_ref_for_message(state, swarm, width, message_idx)
        });
    if artifact_target.is_some() {
        let callout = format!("{indent_str}\u{21b3} {header} done (see ARTIFACTS)");
        out.push(ThreadRow {
            text: pad_line_right(&callout, width),
            kind: ThreadRowKind::ArtifactLink,
        });
    } else {
        let callout = format!("{indent_str}\u{21b3} {header} done");
        out.push(ThreadRow {
            text: callout,
            kind: ThreadRowKind::Agent,
        });
    }
    // Spacer after agent reply to separate chat turns.
    out.push(ThreadRow {
        text: String::new(),
        kind: ThreadRowKind::Agent,
    });
    out
}

fn agent_identity_badge(state: &AppState, agent_id: &str) -> String {
    let id_full = agent_id.trim();
    if let Some(label) = compact_swarm_clone_label(id_full) {
        return truncate_label(&label, AGENT_BADGE_MAX_CHARS);
    }
    if let Some(base_id) = chat_clone_base_id(id_full) {
        return agent_identity_badge(state, base_id);
    }
    let id = truncate_label(id_full, 10);
    let Some(agent) = state
        .agents
        .agents
        .iter()
        .find(|agent| agent.id == agent_id)
    else {
        return id;
    };
    let role_full = agent.role.trim();
    if role_full.is_empty() {
        return id;
    }
    if role_full.eq_ignore_ascii_case(id_full) {
        return truncate_label(role_full, AGENT_BADGE_MAX_CHARS);
    }
    let role = truncate_label(role_full, 12);
    if role.is_empty() {
        return id;
    }
    truncate_label(&format!("{role}/{id}"), AGENT_BADGE_MAX_CHARS)
}

fn compact_swarm_clone_label(agent_id: &str) -> Option<String> {
    let (_base, rest) = agent_id.split_once("#swarm-")?;
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }

    let first_dash = rest.find('-')?;
    let second_dash_rel = rest[first_dash.saturating_add(1)..].find('-')?;
    let second_dash = first_dash.saturating_add(1).saturating_add(second_dash_rel);
    let (_mission_id, suffix) = rest.split_at(second_dash);
    let suffix = suffix.trim_start_matches('-').trim();
    if suffix.is_empty() {
        return Some(rest.to_string());
    }

    let parts = suffix
        .split('-')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        Some(suffix.to_string())
    } else {
        Some(parts.join(" "))
    }
}

fn swarm_clone_source_label(agent_id: &str) -> Option<String> {
    let (base, rest) = agent_id.split_once("#swarm-")?;
    let base = base.trim();
    let rest = rest.trim();
    if base.is_empty() || rest.is_empty() {
        return None;
    }

    let first_dash = rest.find('-')?;
    let second_dash_rel = rest[first_dash.saturating_add(1)..].find('-')?;
    let second_dash = first_dash.saturating_add(1).saturating_add(second_dash_rel);
    let (_mission_id, suffix) = rest.split_at(second_dash);
    let suffix = suffix.trim_start_matches('-').trim();
    if suffix.is_empty() {
        return Some(base.to_string());
    }
    Some(format!("{base}#{suffix}"))
}

fn agent_roster_label(agent: &AgentLane) -> String {
    let id_full = agent.id.trim();
    if let Some(label) = swarm_clone_source_label(id_full) {
        return label;
    }
    if chat_clone_base_id(id_full).is_some() {
        let role_full = agent.role.trim();
        if !role_full.is_empty() {
            return role_full.to_string();
        }
    }
    let role_full = agent.role.trim();
    if role_full.is_empty() {
        return id_full.to_string();
    }
    if role_full.eq_ignore_ascii_case(id_full) {
        return role_full.to_string();
    }
    format!("{role_full}/{id_full}")
}

/// Compact inline working indicator for specific agents, shown right after their user prompt.
fn inline_breather_rows(
    state: &AppState,
    agent_ids: &[String],
    pulse_on: bool,
    width: usize,
) -> Vec<ThreadRow> {
    let now = Instant::now();
    let width = width.max(1);
    let elap_w = 6usize;
    let hb_w = 4usize;
    let out_w = 4usize;
    let times_and_spacing = elap_w + hb_w + out_w + 3;
    let agent_w = width.saturating_sub(times_and_spacing + 1).max(1);

    let seed_id = agent_ids.first().map(String::as_str);
    let ecg = ecg_indicator(state.metrics.frame_count, seed_id, pulse_on, true);

    let mut rows = Vec::new();
    rows.push(ThreadRow {
        text: format!("{ecg} Working ..."),
        kind: ThreadRowKind::Breather,
    });
    rows.push(ThreadRow {
        text: pad_to_width(
            &format!(
                "{} {} {} {}",
                fit_left("AGENT", agent_w),
                fit_right("ELAP", elap_w),
                fit_right("HB", hb_w),
                fit_right("OUT", out_w),
            ),
            width,
        ),
        kind: ThreadRowKind::StatusHeader,
    });
    for id in agent_ids {
        let agent = state.agents.agents.iter().find(|a| a.id == id.as_str());
        let badge = agent
            .map(agent_roster_label)
            .unwrap_or_else(|| id.to_string());
        let turn = state.agents.active_turns.get(id.as_str());
        let stage_raw = if let Some(turn) = turn {
            turn.stage.as_deref().unwrap_or("starting")
        } else {
            "queued"
        };
        let stage = agent
            .map(|a| format_agent_stage_label(state, a, stage_raw))
            .unwrap_or_else(|| stage_raw.to_string());
        let suppress = stage_raw == "queued";
        let (elapsed_s, hb_s, out_s) = if suppress {
            ("--".into(), "--".into(), "--".into())
        } else {
            let elapsed = turn.and_then(|t| now.checked_duration_since(t.started_at));
            let hb = turn
                .and_then(|t| now.checked_duration_since(t.last_heartbeat_at))
                .map(|d| d.as_secs());
            let out = turn
                .and_then(|t| now.checked_duration_since(t.last_output_at))
                .map(|d| d.as_secs());
            (
                elapsed
                    .map(format_duration_compact)
                    .unwrap_or_else(|| "--".into()),
                hb.map(|s| format!("{s}s")).unwrap_or_else(|| "--".into()),
                out.map(|s| format!("{s}s")).unwrap_or_else(|| "--".into()),
            )
        };
        rows.push(ThreadRow {
            text: pad_to_width(
                &format!(
                    "{} {} {} {}",
                    fit_left(&badge, agent_w),
                    fit_right(&elapsed_s, elap_w),
                    fit_right(&hb_s, hb_w),
                    fit_right(&out_s, out_w),
                ),
                width,
            ),
            kind: ThreadRowKind::StatusRow,
        });
        rows.push(ThreadRow {
            text: pad_to_width(
                &format!(
                    "{} {} {} {}",
                    fit_left(&format!("\u{21b3} {stage}"), agent_w),
                    fit_right("", elap_w),
                    fit_right("", hb_w),
                    fit_right("", out_w),
                ),
                width,
            ),
            kind: ThreadRowKind::StatusSubRow,
        });
    }
    rows
}

fn breather_rows_for_user_prompt(
    state: &AppState,
    swarm: Option<&SwarmRuntime>,
    pulse_on: bool,
    width: usize,
) -> Vec<ThreadRow> {
    let mission_ctx = state.agents.selected_context_mission();
    let agent_ctx = state.agents.selected_context_agent();
    let mut primary_ids = Vec::new();
    let mut secondary_ids = Vec::new();
    for agent in state.agents.agents.iter() {
        let has_active = state.agents.active_turns.contains_key(&agent.id);
        let has_queued = state
            .agents
            .queued_codex_turns
            .iter()
            .any(|turn| turn.agent_id == agent.id)
            || state
                .agents
                .queued_claude_turns
                .iter()
                .any(|turn| turn.agent_id == agent.id);
        if !has_active && !has_queued {
            continue;
        }
        let queued_in_mission = mission_ctx.is_some_and(|mission_id| {
            state.agents.queued_codex_turns.iter().any(|turn| {
                turn.agent_id == agent.id && turn.mission_id.as_deref() == Some(mission_id)
            }) || state.agents.queued_claude_turns.iter().any(|turn| {
                turn.agent_id == agent.id && turn.mission_id.as_deref() == Some(mission_id)
            })
        });
        let in_context = if let Some(mission_id) = mission_ctx {
            agent.current_mission.as_deref() == Some(mission_id) || queued_in_mission
        } else if let Some(selected_agent) = agent_ctx {
            agent.id == selected_agent || chat_clone_base_id(&agent.id) == Some(selected_agent)
        } else {
            true
        };
        if in_context {
            primary_ids.push(agent.id.clone());
        } else {
            secondary_ids.push(agent.id.clone());
        }
    }

    let width = width.max(1);
    // Keep the roster table flush to the pane's left edge so it matches the right side alignment.
    let indent_str = String::new();
    let inner = width;

    let now = Instant::now();
    let mut swarm_assigned_ids: Vec<String> = Vec::new();
    let mut swarm_mission_id: Option<&str> = None;
    if let Some(mission_id) = mission_ctx {
        if let Some(mission) = state.agents.missions.iter().find(|m| m.id == mission_id) {
            if mission.swarm {
                swarm_mission_id = Some(mission_id);
                for id in mission.assigned_agents.iter() {
                    if swarm_assigned_ids.iter().any(|existing| existing == id) {
                        continue;
                    }
                    swarm_assigned_ids.push(id.clone());
                }
            }
        }
    }

    let mut ordered_ids = Vec::new();
    ordered_ids.extend(swarm_assigned_ids.iter().cloned());
    for id in primary_ids.iter().chain(secondary_ids.iter()) {
        if ordered_ids.iter().any(|existing: &String| existing == id) {
            continue;
        }
        ordered_ids.push(id.clone());
    }
    if ordered_ids.is_empty() {
        return Vec::new();
    }

    let any_active = ordered_ids
        .iter()
        .any(|id| state.agents.active_turns.contains_key(id.as_str()));
    let any_queued = ordered_ids.iter().any(|id| {
        state
            .agents
            .queued_codex_turns
            .iter()
            .any(|turn| turn.agent_id == id.as_str())
            || state
                .agents
                .queued_claude_turns
                .iter()
                .any(|turn| turn.agent_id == id.as_str())
    });
    let all_swarm_done = swarm_mission_id.is_some_and(|mid| {
        !swarm_assigned_ids.is_empty()
            && swarm_assigned_ids.iter().all(|id| {
                state.agents.messages.iter().any(|msg| {
                    msg.mission_id.as_deref() == Some(mid)
                        && msg.agent_id.as_deref() == Some(id.as_str())
                })
            })
    });
    let working = any_active || any_queued;
    let swarm_phase = swarm_mission_id.and_then(|mid| swarm.and_then(|s| s.swarm_stage_label(mid)));
    let label = if any_active || any_queued {
        match swarm_phase {
            Some("VERIFY") => "Verifying ...",
            Some("SYNTH") => "Synthesizing ...",
            _ if any_active => "Working ...",
            _ => "Queued ...",
        }
    } else if swarm_mission_id.is_some() && all_swarm_done {
        "Done"
    } else if swarm_mission_id.is_some() {
        "Waiting ..."
    } else {
        "Working ..."
    };

    let seed_id = primary_ids
        .first()
        .or_else(|| secondary_ids.first())
        .or_else(|| ordered_ids.first())
        .map(String::as_str);
    let ecg = ecg_indicator(state.metrics.frame_count, seed_id, pulse_on, working);

    let mut rows = Vec::new();
    rows.push(ThreadRow {
        text: format!("{ecg} {label}"),
        kind: ThreadRowKind::Breather,
    });

    // Table layout: show the agent roster with a stable right-aligned timing column.
    let elap_w = 6usize;
    let hb_w = 4usize;
    let out_w = 4usize;
    let times_and_spacing = elap_w + hb_w + out_w + 3; // spaces between columns
    let agent_w = inner.saturating_sub(times_and_spacing + 1).max(1);

    if agent_w < 6 {
        // Narrow fallback: keep it readable without a strict column layout.
        for id in ordered_ids.iter() {
            let agent = state
                .agents
                .agents
                .iter()
                .find(|agent| agent.id == id.as_str());
            let badge = agent
                .map(agent_roster_label)
                .unwrap_or_else(|| id.to_string());
            let turn = state.agents.active_turns.get(id.as_str());
            let queued_for_swarm = swarm_mission_id.is_some_and(|mid| {
                state.agents.queued_codex_turns.iter().any(|turn| {
                    turn.agent_id == id.as_str() && turn.mission_id.as_deref() == Some(mid)
                }) || state.agents.queued_claude_turns.iter().any(|turn| {
                    turn.agent_id == id.as_str() && turn.mission_id.as_deref() == Some(mid)
                })
            });
            let queued_any = state
                .agents
                .queued_codex_turns
                .iter()
                .any(|turn| turn.agent_id == id.as_str())
                || state
                    .agents
                    .queued_claude_turns
                    .iter()
                    .any(|turn| turn.agent_id == id.as_str());
            let has_message = swarm_mission_id.is_some_and(|mid| {
                state.agents.messages.iter().any(|msg| {
                    msg.mission_id.as_deref() == Some(mid)
                        && msg.agent_id.as_deref() == Some(id.as_str())
                })
            });
            let stage_raw = if let Some(turn) = turn {
                turn.stage.as_deref().unwrap_or("starting")
            } else if matches!(agent.map(|agent| agent.status), Some(AgentStatus::Error)) {
                "error"
            } else if queued_for_swarm {
                "swarm_queued"
            } else if queued_any {
                "queued"
            } else if swarm_assigned_ids.iter().any(|assigned| assigned == id) {
                if has_message {
                    "swarm_done"
                } else {
                    "swarm_pending"
                }
            } else {
                "pending"
            };
            let suppress_times = matches!(stage_raw, "queued" | "swarm_queued");
            let stage = agent
                .map(|agent| format_agent_stage_label(state, agent, stage_raw))
                .unwrap_or_else(|| stage_raw.to_string());

            let (elapsed_s, hb_s, out_s) = if suppress_times {
                ("--".into(), "--".into(), "--".into())
            } else {
                let elapsed = turn.and_then(|turn| now.checked_duration_since(turn.started_at));
                let hb_age = turn
                    .and_then(|turn| now.checked_duration_since(turn.last_heartbeat_at))
                    .map(|d| d.as_secs());
                let out_age = turn
                    .and_then(|turn| now.checked_duration_since(turn.last_output_at))
                    .map(|d| d.as_secs());

                let elapsed_s = elapsed
                    .map(format_duration_compact)
                    .unwrap_or_else(|| "--".into());
                let hb_s = hb_age
                    .map(|s| format!("{s}s"))
                    .unwrap_or_else(|| "--".into());
                let out_s = out_age
                    .map(|s| format!("{s}s"))
                    .unwrap_or_else(|| "--".into());

                (elapsed_s, hb_s, out_s)
            };

            rows.push(ThreadRow {
                text: pad_to_width(
                    &format!("{indent_str}{badge} {elapsed_s} {hb_s} {out_s}"),
                    width,
                ),
                kind: ThreadRowKind::StatusRow,
            });
            rows.push(ThreadRow {
                text: pad_to_width(&format!("{indent_str}↳ {stage}"), width),
                kind: ThreadRowKind::StatusSubRow,
            });
        }
        if let Some(mission_id) = swarm_mission_id {
            append_swarm_meta_footer_rows(
                &mut rows,
                state,
                mission_id,
                &indent_str,
                width,
                inner,
                working,
            );
        }
        return rows;
    }

    rows.push(ThreadRow {
        text: pad_to_width(
            &format!(
                "{indent_str}{} {} {} {}",
                fit_left("AGENT", agent_w),
                fit_right("ELAP", elap_w),
                fit_right("HB", hb_w),
                fit_right("OUT", out_w),
            ),
            width,
        ),
        kind: ThreadRowKind::StatusHeader,
    });
    for id in ordered_ids.iter() {
        let agent = state
            .agents
            .agents
            .iter()
            .find(|agent| agent.id == id.as_str());
        let badge = agent
            .map(agent_roster_label)
            .unwrap_or_else(|| id.to_string());
        let turn = state.agents.active_turns.get(id.as_str());
        let queued_for_swarm =
            swarm_mission_id.is_some_and(|mid| {
                state.agents.queued_codex_turns.iter().any(|turn| {
                    turn.agent_id == id.as_str() && turn.mission_id.as_deref() == Some(mid)
                }) || state.agents.queued_claude_turns.iter().any(|turn| {
                    turn.agent_id == id.as_str() && turn.mission_id.as_deref() == Some(mid)
                })
            });
        let queued_any = state
            .agents
            .queued_codex_turns
            .iter()
            .any(|turn| turn.agent_id == id.as_str())
            || state
                .agents
                .queued_claude_turns
                .iter()
                .any(|turn| turn.agent_id == id.as_str());
        let has_message = swarm_mission_id.is_some_and(|mid| {
            state.agents.messages.iter().any(|msg| {
                msg.mission_id.as_deref() == Some(mid)
                    && msg.agent_id.as_deref() == Some(id.as_str())
            })
        });
        let stage_raw = if let Some(turn) = turn {
            turn.stage.as_deref().unwrap_or("starting")
        } else if matches!(agent.map(|agent| agent.status), Some(AgentStatus::Error)) {
            "error"
        } else if queued_for_swarm {
            "swarm_queued"
        } else if queued_any {
            "queued"
        } else if swarm_assigned_ids.iter().any(|assigned| assigned == id) {
            if has_message {
                "swarm_done"
            } else {
                "swarm_pending"
            }
        } else {
            "pending"
        };
        let suppress_times = matches!(stage_raw, "queued" | "swarm_queued");
        let stage = agent
            .map(|agent| format_agent_stage_label(state, agent, stage_raw))
            .unwrap_or_else(|| stage_raw.to_string());

        let (elapsed_s, hb_s, out_s) = if suppress_times {
            ("--".into(), "--".into(), "--".into())
        } else {
            let elapsed = turn.and_then(|turn| now.checked_duration_since(turn.started_at));
            let hb_age = turn
                .and_then(|turn| now.checked_duration_since(turn.last_heartbeat_at))
                .map(|d| d.as_secs());
            let out_age = turn
                .and_then(|turn| now.checked_duration_since(turn.last_output_at))
                .map(|d| d.as_secs());

            let elapsed_s = elapsed
                .map(format_duration_compact)
                .unwrap_or_else(|| "--".into());
            let hb_s = hb_age
                .map(|s| format!("{s}s"))
                .unwrap_or_else(|| "--".into());
            let out_s = out_age
                .map(|s| format!("{s}s"))
                .unwrap_or_else(|| "--".into());

            (elapsed_s, hb_s, out_s)
        };
        rows.push(ThreadRow {
            text: pad_to_width(
                &format!(
                    "{indent_str}{} {} {} {}",
                    fit_left(&badge, agent_w),
                    fit_right(&elapsed_s, elap_w),
                    fit_right(&hb_s, hb_w),
                    fit_right(&out_s, out_w),
                ),
                width,
            ),
            kind: ThreadRowKind::StatusRow,
        });
        rows.push(ThreadRow {
            text: pad_to_width(
                &format!(
                    "{indent_str}{} {} {} {}",
                    fit_left(&format!("↳ {stage}"), agent_w),
                    fit_right("", elap_w),
                    fit_right("", hb_w),
                    fit_right("", out_w),
                ),
                width,
            ),
            kind: ThreadRowKind::StatusSubRow,
        });
    }

    if let Some(mission_id) = swarm_mission_id {
        append_swarm_meta_footer_rows(
            &mut rows,
            state,
            mission_id,
            &indent_str,
            width,
            inner,
            working,
        );
    }

    rows
}

fn append_swarm_meta_footer_rows(
    rows: &mut Vec<ThreadRow>,
    state: &AppState,
    mission_id: &str,
    indent_str: &str,
    width: usize,
    inner: usize,
    working: bool,
) {
    let Some(meta) = collect_swarm_footer_meta(state, mission_id) else {
        return;
    };

    let max_inner = inner.max(1);
    let mut entries: Vec<(&str, String)> = Vec::new();
    // When agents are actively working, show only the essential fields.
    if !working {
        if let Some(value) = meta.template {
            entries.push(("Template", value));
        }
    }
    if let Some(value) = meta.mission {
        entries.push(("Mission", value));
    }
    if !working {
        if let Some(value) = meta.integrator {
            entries.push(("Integrator", value));
        }
        if let Some(value) = meta.verifier {
            entries.push(("Verifier", value));
        }
    }
    if let Some(value) = meta.gates {
        entries.push(("Gates", value));
    }
    if !working {
        if let Some(value) = meta.status {
            entries.push(("Status", value));
        }
    }
    if !meta.notes.is_empty() {
        entries.push(("Notes", meta.notes.join(" | ")));
    }

    if entries.is_empty() {
        return;
    }

    rows.push(ThreadRow {
        text: pad_to_width(indent_str, width),
        kind: ThreadRowKind::StatusRow,
    });
    rows.push(ThreadRow {
        text: pad_to_width(&format!("{indent_str}{}", "─".repeat(max_inner)), width),
        kind: ThreadRowKind::StatusHeader,
    });
    rows.push(ThreadRow {
        text: pad_to_width(&format!("{indent_str}Swarm"), width),
        kind: ThreadRowKind::StatusHeader,
    });

    for (label, value) in entries.iter() {
        append_swarm_footer_entry(rows, indent_str, width, max_inner, label, value);
    }

    rows.push(ThreadRow {
        text: pad_to_width(&format!("{indent_str}{}", "─".repeat(max_inner)), width),
        kind: ThreadRowKind::StatusHeader,
    });
    rows.push(ThreadRow {
        text: pad_to_width(indent_str, width),
        kind: ThreadRowKind::StatusRow,
    });
}

#[derive(Default)]
struct SwarmFooterMeta {
    template: Option<String>,
    mission: Option<String>,
    integrator: Option<String>,
    verifier: Option<String>,
    gates: Option<String>,
    status: Option<String>,
    notes: Vec<String>,
}

impl SwarmFooterMeta {
    fn is_empty(&self) -> bool {
        self.template.is_none()
            && self.mission.is_none()
            && self.integrator.is_none()
            && self.verifier.is_none()
            && self.gates.is_none()
            && self.status.is_none()
            && self.notes.is_empty()
    }
}

fn append_swarm_footer_entry(
    rows: &mut Vec<ThreadRow>,
    indent_str: &str,
    width: usize,
    max_inner: usize,
    label: &str,
    value: &str,
) {
    let prefix = format!("• {label}: ");
    let prefix_len = prefix.chars().count();
    let available = max_inner.saturating_sub(prefix_len).max(1);
    let segments = wrap_visual_line(value, available);
    if segments.is_empty() {
        rows.push(ThreadRow {
            text: pad_to_width(&format!("{indent_str}{prefix}"), width),
            kind: ThreadRowKind::StatusSubRow,
        });
        return;
    }

    for (idx, seg) in segments.iter().enumerate() {
        let line = if idx == 0 {
            format!("{indent_str}{prefix}{seg}")
        } else {
            format!("{indent_str}{}{}", " ".repeat(prefix_len), seg)
        };
        rows.push(ThreadRow {
            text: pad_to_width(&line, width),
            kind: ThreadRowKind::StatusSubRow,
        });
    }
}

fn collect_swarm_footer_meta(state: &AppState, mission_id: &str) -> Option<SwarmFooterMeta> {
    let mut meta = SwarmFooterMeta::default();
    for msg in state.agents.messages.iter().rev() {
        if msg.mission_id.as_deref() != Some(mission_id) {
            continue;
        }
        if msg.agent_id.as_deref() != Some("swarm") {
            continue;
        }
        let text = msg.text.trim();
        if let Some(template_line) = parse_swarm_template_meta(text) {
            if meta.template.is_none() {
                meta.template = Some(template_line.template);
            }
            if meta.mission.is_none() {
                meta.mission = template_line.mission;
            }
            if meta.integrator.is_none() {
                meta.integrator = template_line.integrator;
            }
            if meta.verifier.is_none() {
                meta.verifier = template_line.verifier;
            }
            if meta.gates.is_none() {
                meta.gates = template_line.gates;
            }
            continue;
        }
        if meta.gates.is_none() {
            if let Some(gates) = parse_swarm_gates_line(text) {
                meta.gates = Some(gates);
                continue;
            }
        }
        if meta.status.is_none() {
            if let Some(status) = parse_verify_status_line(text) {
                meta.status = Some(status);
                continue;
            }
        }
        if text.starts_with("Swarm ") || text.starts_with("VERIFY") {
            meta.notes.push(text.to_string());
        }
    }

    if meta.notes.len() > 3 {
        meta.notes.truncate(3);
    }

    if meta.is_empty() {
        None
    } else {
        Some(meta)
    }
}

struct SwarmTemplateMeta {
    template: String,
    mission: Option<String>,
    integrator: Option<String>,
    verifier: Option<String>,
    gates: Option<String>,
}

fn parse_swarm_template_meta(text: &str) -> Option<SwarmTemplateMeta> {
    let rest = text.trim().strip_prefix("Swarm template:")?.trim();
    let mut parts = rest.split(" | ");
    let template = parts.next().unwrap_or_default().trim();
    if template.is_empty() {
        return None;
    }

    let mut mission: Option<String> = None;
    let mut integrator: Option<String> = None;
    let mut verifier: Option<String> = None;
    let mut gates: Option<String> = None;
    for part in parts {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("mission:") {
            mission = Some(normalize_swarm_meta_value(value));
            continue;
        }
        if let Some(value) = part.strip_prefix("integrator:") {
            integrator = Some(normalize_swarm_meta_value(value));
            continue;
        }
        if let Some(value) = part.strip_prefix("verifier:") {
            verifier = Some(normalize_swarm_meta_value(value));
            continue;
        }
        if let Some(value) = part.strip_prefix("gates:") {
            gates = Some(normalize_swarm_meta_value(short_gate_bundle_label(value)));
        }
    }

    Some(SwarmTemplateMeta {
        template: normalize_swarm_meta_value(template),
        mission,
        integrator,
        verifier,
        gates,
    })
}

fn parse_swarm_gates_line(text: &str) -> Option<String> {
    let rest = text.trim().strip_prefix("Swarm gates:")?.trim();
    if rest.is_empty() {
        return None;
    }
    Some(normalize_swarm_meta_value(short_gate_bundle_label(rest)))
}

fn parse_verify_status_line(text: &str) -> Option<String> {
    let rest = text.trim().strip_prefix("VERIFY result:")?.trim();
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_string())
}

fn normalize_swarm_meta_value(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("(none)") {
        return "—".to_string();
    }
    if trimmed.eq_ignore_ascii_case("none") {
        return "none".to_string();
    }
    if trimmed.eq_ignore_ascii_case("none (config)") {
        return "none".to_string();
    }
    if trimmed.starts_with("(none)") {
        return "none".to_string();
    }
    trimmed.to_string()
}

fn short_gate_bundle_label(value: &str) -> &str {
    value
        .trim()
        .split_once(" (")
        .map(|(label, _)| label.trim())
        .unwrap_or_else(|| value.trim())
}

fn format_agent_stage_label(state: &AppState, agent: &AgentLane, stage: &str) -> String {
    if state.debug {
        return stage.to_string();
    }
    if stage == "token_count" {
        return format_token_count_stage(state, agent);
    }

    let base = if let Some((prefix, inner_raw)) = split_stage_with_parens(stage) {
        match prefix {
            "item_started" | "item.started" => {
                format!("Starting {}", humanize_stage_atom(inner_raw))
            }
            "item_completed" | "item.completed" => {
                format!("Finished {}", humanize_stage_atom(inner_raw))
            }
            "tools/call" => match inner_raw {
                "codex" => "Starting session".into(),
                "codex-reply" => "Continuing session".into(),
                _ => format!("Calling {}", humanize_stage_atom(inner_raw)),
            },
            "assistant" => {
                format!("Assistant({})", humanize_stage_atom(inner_raw))
            }
            "tool_use" => {
                format!("Tool: {}", humanize_stage_atom(inner_raw))
            }
            "tool_result" => {
                format!("Result: {}", humanize_stage_atom(inner_raw))
            }
            "content" => {
                format!("Writing {}", humanize_stage_atom(inner_raw))
            }
            _ => sentence_case(&humanize_stage_atom(stage)),
        }
    } else {
        match stage {
            "starting" => "Starting".into(),
            "queued" => "Queued".into(),
            "warning" => "Warning".into(),
            "error" => "Error".into(),
            "stream_error" | "stream.error" => "Stream error".into(),
            _ => sentence_case(&humanize_stage_atom(stage)),
        }
    };

    // For Claude agents, append token usage to every stage label so the
    // user always sees context consumption alongside activity.
    if agent.is_claude() {
        if let Some(suffix) = format_token_count_suffix(state, agent) {
            return format!("{base} \u{2022} {suffix}");
        }
    }

    base
}

fn format_token_count_stage(state: &AppState, agent: &AgentLane) -> String {
    if !agent.is_codex() && !agent.is_claude() {
        return "Updating token usage".into();
    }

    let agent_id = agent.id.as_str();
    let mission_id = agent
        .current_mission
        .as_deref()
        .or_else(|| state.agents.selected_context_mission());

    let is_claude = agent.is_claude();
    let pct = if let Some(mission_id) = mission_id {
        if is_claude {
            state
                .agents
                .claude_mission_context_remaining_pct
                .get(mission_id)
                .and_then(|m| m.get(agent_id))
                .copied()
        } else {
            state
                .agents
                .codex_mission_context_remaining_pct
                .get(mission_id)
                .and_then(|m| m.get(agent_id))
                .copied()
        }
    } else if is_claude {
        state
            .agents
            .claude_context_remaining_pct
            .get(agent_id)
            .copied()
    } else {
        state
            .agents
            .codex_context_remaining_pct
            .get(agent_id)
            .copied()
    };
    let used = if let Some(mission_id) = mission_id {
        if is_claude {
            state
                .agents
                .claude_mission_used_tokens
                .get(mission_id)
                .and_then(|m| m.get(agent_id))
                .copied()
        } else {
            state
                .agents
                .codex_mission_used_tokens
                .get(mission_id)
                .and_then(|m| m.get(agent_id))
                .copied()
        }
    } else if is_claude {
        state.agents.claude_used_tokens.get(agent_id).copied()
    } else {
        state.agents.codex_used_tokens.get(agent_id).copied()
    };
    let max = state
        .agents
        .codex_effective_context_window_tokens
        .get(agent_id)
        .or_else(|| {
            state
                .agents
                .claude_effective_context_window_tokens
                .get(agent_id)
        })
        .copied();

    match (pct, used, max) {
        (Some(pct), Some(used), Some(max)) => format!(
            "Context: {pct}% left {}/{}",
            format_token_count_short(used),
            format_token_count_short(max)
        ),
        (None, Some(used), Some(max)) => format!(
            "Context: {}/{}",
            format_token_count_short(used),
            format_token_count_short(max)
        ),
        (Some(pct), None, _) => format!("Context: {pct}% left"),
        (None, Some(used), None) => format!("Tokens: {}", format_token_count_short(used)),
        _ => "Updating context usage".into(),
    }
}

/// Compact token usage suffix for inline display next to a stage label.
/// Returns `None` when no token data is available yet.
fn format_token_count_suffix(state: &AppState, agent: &AgentLane) -> Option<String> {
    let agent_id = agent.id.as_str();
    let mission_id = agent
        .current_mission
        .as_deref()
        .or_else(|| state.agents.selected_context_mission());

    let used = if let Some(mid) = mission_id {
        state
            .agents
            .claude_mission_used_tokens
            .get(mid)
            .and_then(|m| m.get(agent_id))
            .copied()
    } else {
        state.agents.claude_used_tokens.get(agent_id).copied()
    };
    let pct = if let Some(mid) = mission_id {
        state
            .agents
            .claude_mission_context_remaining_pct
            .get(mid)
            .and_then(|m| m.get(agent_id))
            .copied()
    } else {
        state
            .agents
            .claude_context_remaining_pct
            .get(agent_id)
            .copied()
    };

    match (pct, used) {
        (Some(pct), Some(used)) => Some(format!("{} ({pct}%)", format_token_count_short(used))),
        (Some(pct), None) => Some(format!("{pct}% left")),
        (None, Some(used)) => Some(format_token_count_short(used)),
        _ => None,
    }
}

fn split_stage_with_parens(stage: &str) -> Option<(&str, &str)> {
    if !stage.ends_with(')') {
        return None;
    }
    let open = stage.find('(')?;
    if open + 1 >= stage.len() {
        return None;
    }
    Some((&stage[..open], &stage[open + 1..stage.len() - 1]))
}

fn humanize_stage_atom(text: &str) -> String {
    let mut out = String::new();
    let mut last_was_space = true;
    for ch in text.chars() {
        let mapped = match ch {
            '_' | '.' | '/' | '-' => ' ',
            _ => ch,
        };
        if mapped == ' ' {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
            continue;
        }
        out.push(mapped);
        last_was_space = false;
    }
    out.trim().to_string()
}

fn sentence_case(text: &str) -> String {
    let trimmed = text.trim();
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = String::new();
    out.extend(first.to_uppercase());
    out.push_str(chars.as_str());
    out
}

fn truncate_label(input: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut chars = input.chars();
    out.extend(chars.by_ref().take(max_chars));
    if chars.next().is_some() {
        if max_chars == 1 {
            return "…".to_string();
        }
        out.pop();
        out.push('…');
    }
    out
}

fn fit_left(text: &str, width: usize) -> String {
    fit_cell(text, width, false)
}

fn fit_right(text: &str, width: usize) -> String {
    fit_cell(text, width, true)
}

fn fit_cell(text: &str, width: usize, right_align: bool) -> String {
    if width == 0 {
        return String::new();
    }
    let len = text.chars().count();
    if len == width {
        return text.to_string();
    }
    if len > width {
        if width == 1 {
            return "…".into();
        }
        let mut out = text.chars().take(width - 1).collect::<String>();
        out.push('…');
        return out;
    }
    let pad = " ".repeat(width - len);
    if right_align {
        format!("{pad}{text}")
    } else {
        format!("{text}{pad}")
    }
}

fn format_duration_compact(duration: Duration) -> String {
    let secs = duration.as_secs();
    let minutes = secs / 60;
    let seconds = secs % 60;
    let hours = minutes / 60;
    let minutes = minutes % 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

fn format_token_count_short(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        let whole = tokens / 1_000_000;
        let frac = (tokens % 1_000_000) / 100_000;
        if whole < 10 && frac > 0 {
            format!("{whole}.{frac}M")
        } else {
            format!("{whole}M")
        }
    } else if tokens >= 1_000 {
        let whole = tokens / 1_000;
        let frac = (tokens % 1_000) / 100;
        if whole < 100 && frac > 0 {
            format!("{whole}.{frac}K")
        } else {
            format!("{whole}K")
        }
    } else {
        tokens.to_string()
    }
}

fn pad_line_right(text: &str, width: usize) -> String {
    let text_width = UnicodeWidthStr::width(text);
    if text_width >= width {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + (width - text_width));
    out.push_str(text);
    out.push_str(&" ".repeat(width - text_width));
    out
}

fn format_user_bubble_rows(_msg: &AgentMessage, text_lines: &[&str], width: usize) -> Vec<String> {
    // Render user prompts as a background-tinted block (no ASCII borders).
    // We pad every line out to the full thread width so the background is continuous.
    let width = width.max(1);
    let indent = 2usize.min(width.saturating_sub(1));
    // Leave at least 1 trailing space so clipboard trimming and mouse drag snapping can reliably
    // detect these "prompt block" rows.
    let max_inner = width.saturating_sub(indent + 1).max(1);

    let mut out = Vec::new();
    // Add top/bottom padding so prompt blocks breathe vertically.
    out.push(pad_to_width(&" ".repeat(indent), width));
    out.extend(
        wrap_visual_line("You", max_inner)
            .into_iter()
            .map(|line| pad_to_width(&format!("{}{}", " ".repeat(indent), line), width)),
    );
    for line in text_lines {
        for seg in wrap_visual_line(line, max_inner) {
            out.push(pad_to_width(
                &format!("{}{}", " ".repeat(indent), seg),
                width,
            ));
        }
    }
    out.push(pad_to_width(&" ".repeat(indent), width));
    out
}

fn pad_to_width(input: &str, width: usize) -> String {
    let width = width.max(1);
    let current = UnicodeWidthStr::width(input);
    if current >= width {
        // The input is already wrapped to width; truncate by display width as a safety net.
        let mut out = String::new();
        let mut used = 0usize;
        for ch in input.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
            if used + ch_width > width {
                break;
            }
            out.push(ch);
            used += ch_width;
        }
        return out;
    }
    let mut out = String::with_capacity(input.len() + (width - current));
    out.push_str(input);
    out.push_str(&" ".repeat(width - current));
    out
}

fn ecg_indicator(
    frame_count: u64,
    agent_id: Option<&str>,
    pulse_on: bool,
    working: bool,
) -> &'static str {
    if !working {
        return "▁▁▁▁▁▁";
    }
    const ECG: [&str; 9] = [
        "▁▂▁▃▂▁",
        "▁▃▅▂▁▂",
        "▁▄▇▃▁▂",
        "▁▅█▄▂▁",
        "▁▄▆▃▁▂",
        "▁▃▅▂▁▃",
        "▁▂▄▂▁▂",
        "▁▃▆▃▁▂",
        "▁▂▅▂▁▂",
    ];
    let seed = agent_seed(agent_id.unwrap_or("agent"));
    let phase = (frame_count / 3).wrapping_add(seed.rotate_left(7));
    let mut x = phase ^ 0x9E37_79B9_7F4A_7C15_u64;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    if !pulse_on {
        x ^= 0xA24B_AED4_9C77_1E3D;
    }
    ECG[(x as usize) % ECG.len()]
}

fn agent_seed(agent_id: &str) -> u64 {
    let mut hash = 1469598103934665603_u64;
    for byte in agent_id.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

fn pulse_on(state: &AppState) -> bool {
    (state.metrics.frame_count / 6).is_multiple_of(2)
}

fn cursor_visible(state: &AppState) -> bool {
    pulse_on(state)
}

fn wrap_visual_line(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    let mut last_break: Option<(usize, usize)> = None; // (byte idx, width at break)

    let flush_line = |lines: &mut Vec<String>,
                      current: &mut String,
                      current_width: &mut usize,
                      last_break: &mut Option<(usize, usize)>| {
        lines.push(std::mem::take(current));
        *current_width = 0;
        *last_break = None;
    };

    let push_char = |lines: &mut Vec<String>,
                     current: &mut String,
                     current_width: &mut usize,
                     last_break: &mut Option<(usize, usize)>,
                     ch: char| {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        if *current_width + ch_width > width && !current.is_empty() {
            if let Some((break_byte, break_width)) = last_break.take() {
                let after = current[break_byte..].to_string();
                let before = current[..break_byte].to_string();
                lines.push(before);
                *current = after;
                *current_width = (*current_width).saturating_sub(break_width);
            } else {
                flush_line(lines, current, current_width, last_break);
            }
        }
        current.push(ch);
        *current_width += ch_width;
        if ch == ' ' {
            *last_break = Some((current.len(), *current_width));
        }
    };

    for ch in text.chars() {
        match ch {
            '\n' | '\r' => {
                flush_line(
                    &mut lines,
                    &mut current,
                    &mut current_width,
                    &mut last_break,
                );
            }
            '\t' => {
                let tab_width = next_tab_width(current_width, width);
                for _ in 0..tab_width {
                    push_char(
                        &mut lines,
                        &mut current,
                        &mut current_width,
                        &mut last_break,
                        ' ',
                    );
                }
            }
            _ => push_char(
                &mut lines,
                &mut current,
                &mut current_width,
                &mut last_break,
                ch,
            ),
        }
    }
    lines.push(current);
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

#[cfg(test)]
#[path = "tests/agent_console_view.rs"]
mod tests;
