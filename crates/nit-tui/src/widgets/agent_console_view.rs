use nit_core::{
    AgentConsoleRow as ThreadRow, AgentConsoleRowKind as ThreadRowKind, AgentConsoleRowsCacheKey,
    AgentLane, AgentMessage, AgentStatus, AppState, PaneId, UiSelectionPane,
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

use crate::swarm::SwarmRuntime;
use crate::theme::Theme;
use crate::widgets::text_selection::apply_ui_selection;

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
    let codex_size = agent.and_then(|agent_id| {
        let is_codex = state
            .agents
            .agents
            .iter()
            .find(|lane| lane.id == agent_id)
            .is_some_and(|lane| lane.is_codex());
        if !is_codex {
            return None;
        }
        state
            .agents
            .codex_selected_reasoning_effort
            .get(agent_id)
            .or_else(|| state.agents.codex_default_reasoning_effort.get(agent_id))
            .map(|s| s.as_str())
    });
    let codex_ctx_pct = agent.and_then(|agent_id| {
        let is_codex = state
            .agents
            .agents
            .iter()
            .find(|lane| lane.id == agent_id)
            .is_some_and(|lane| lane.is_codex());
        if !is_codex {
            return None;
        }
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
    });
    let codex_ctx_used = agent.and_then(|agent_id| {
        let is_codex = state
            .agents
            .agents
            .iter()
            .find(|lane| lane.id == agent_id)
            .is_some_and(|lane| lane.is_codex());
        if !is_codex {
            return None;
        }
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
    });
    let codex_ctx_max = agent.and_then(|agent_id| {
        state
            .agents
            .codex_effective_context_window_tokens
            .get(agent_id)
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
    let agent_style = if agent.is_some() {
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD)
    } else {
        label_style
    };
    let context_line = Line::from(vec![
        Span::styled("mission=", label_style),
        Span::styled(mission.unwrap_or("--"), mission_style),
        Span::styled("  ", label_style),
        Span::styled("agent=", label_style),
        Span::styled(agent.unwrap_or("--"), agent_style),
        Span::styled("  ", label_style),
        Span::styled("size=", label_style),
        Span::styled(
            codex_size.unwrap_or("--"),
            if codex_size.is_some() {
                Style::default()
                    .fg(theme.title_focused)
                    .add_modifier(Modifier::BOLD)
            } else {
                label_style
            },
        ),
        Span::styled("  ", label_style),
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
    let (cached_rows_len, _) = refresh_thread_rows_cache(state, thread_width);
    let thread_height = layout.thread_area.height.max(1) as usize;
    let breather = breather_rows_for_user_prompt(state, Some(swarm), pulse_on, thread_width);
    let total_rows = cached_rows_len.saturating_add(breather.len());
    let max_scroll = total_rows.saturating_sub(thread_height);
    state.agents.console_scroll = if state.agents.console_scroll == usize::MAX {
        max_scroll
    } else {
        state.agents.console_scroll.min(max_scroll)
    };
    let scroll_usize = state.agents.console_scroll;
    let visible_rows = state
        .agents
        .console_rows_cache
        .rows
        .iter()
        .chain(breather.iter())
        .skip(scroll_usize)
        .take(thread_height);
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
        .map(Line::from)
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
        theme.cursor_line_bg
    } else {
        theme.background
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

fn refresh_thread_rows_cache(state: &mut AppState, width: usize) -> (usize, bool) {
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

    let mut rows = Vec::new();
    let mut last_message_was_user = false;
    for msg in state.agents.messages.iter() {
        if !message_matches_context(msg, mission_ref, agent_ref) {
            continue;
        }
        last_message_was_user = msg.agent_id.is_none();
        rows.extend(format_message_rows(state, msg, width, false));
    }

    let key = AgentConsoleRowsCacheKey {
        width,
        mission: mission_ref.map(str::to_string),
        agent: agent_ref.map(str::to_string),
        messages_len,
    };
    state.agents.console_rows_cache.key = Some(key);
    state.agents.console_rows_cache.rows = rows;
    state.agents.console_rows_cache.last_message_was_user = last_message_was_user;
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

fn chat_input_window_start(
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

fn chat_input_char_index_for_display_pos(
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

fn wrap_input_with_cursor(
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
            ThreadRowKind::Breather => breather_line(&row.text, theme),
            ThreadRowKind::StatusHeader => status_header_line(&row.text, theme),
            ThreadRowKind::StatusRow => status_row_line(&row.text, theme),
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
        .fg(theme.accent)
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

        // Highlight the agent/model badge (e.g. "[gpt-5.3-codex]") using the same accent color as
        // the breather ECG. Keep the surrounding text cyan so headers remain scannable.
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
    } else {
        push_agent_text_with_inline_highlights(&mut spans, &body, agent_style, theme);
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

    let prompt_bg = dim_bg_towards(
        theme.cursor_line_bg,
        theme.background,
        USER_PROMPT_BG_BACKGROUND_PCT,
    );

    // User prompts are rendered as a padded block with a subtle background instead of ASCII
    // borders. Pad spaces are included in the string so the background fills the whole row.
    let base = Style::default().fg(theme.foreground).bg(prompt_bg);
    let label_style = Style::default()
        .fg(theme.accent)
        .bg(prompt_bg)
        .add_modifier(Modifier::BOLD);

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

fn status_header_line(text: &str, theme: &Theme) -> Line<'static> {
    let bg = dim_bg_towards(theme.border, theme.background, 85);
    Line::from(Span::styled(
        text.to_string(),
        Style::default()
            .fg(theme.border)
            .bg(bg)
            .add_modifier(Modifier::DIM),
    ))
}

fn status_row_line(text: &str, theme: &Theme) -> Line<'static> {
    let bg = dim_bg_towards(theme.border, theme.background, 85);
    Line::from(Span::styled(
        text.to_string(),
        Style::default().fg(theme.foreground).bg(bg),
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

fn dim_bg_towards(color: Color, background: Color, background_pct: u8) -> Color {
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
    let mut rows = Vec::new();
    for msg in state.agents.messages.iter() {
        if !message_matches_context(msg, mission, agent) {
            continue;
        }
        rows.extend(format_message_rows(state, msg, width, pulse_on));
    }
    rows.extend(breather_rows_for_user_prompt(state, swarm, pulse_on, width));
    rows
}

fn message_matches_context(msg: &AgentMessage, mission: Option<&str>, agent: Option<&str>) -> bool {
    if let Some(mission_id) = mission {
        return msg.mission_id.as_deref() == Some(mission_id)
            || matches!(msg.channel, nit_core::AgentChannel::Broadcast);
    }
    if let Some(agent_id) = agent {
        return msg.agent_id.as_deref() == Some(agent_id)
            || msg.agent_id.is_none()
            || matches!(msg.channel, nit_core::AgentChannel::Broadcast);
    }
    true
}

fn format_message_rows(
    state: &AppState,
    msg: &AgentMessage,
    width: usize,
    pulse_on: bool,
) -> Vec<ThreadRow> {
    let width = width.max(1);
    let text_lines: Vec<&str> = if msg.text.is_empty() {
        vec![""]
    } else {
        msg.text.split('\n').collect()
    };
    if msg.agent_id.is_none() {
        let bubble = format_user_bubble_rows(msg, &text_lines, width);
        let mut out = bubble
            .into_iter()
            .map(|text| ThreadRow {
                text,
                kind: ThreadRowKind::User,
            })
            .collect::<Vec<_>>();
        // Spacer between chat turns to make prompts easier to scan.
        out.push(ThreadRow {
            text: String::new(),
            kind: ThreadRowKind::User,
        });
        return out;
    }

    // Swarm meta is shown in the "Working ..." table footer when in swarm mission context, so
    // don't also render it as a transcript message.
    if msg.agent_id.as_deref() == Some("swarm") && msg.text.starts_with("Swarm ") {
        return Vec::new();
    }

    let src = msg.agent_id.as_deref().unwrap_or("agent");
    let mission_ctx = state.agents.selected_context_mission();
    let agent_ctx = state.agents.selected_context_agent();
    let show_badge = if mission_ctx.is_some() {
        // Mission context can include multiple agents, so always label who spoke.
        true
    } else if let Some(selected) = agent_ctx {
        // In single-agent chat context, don't repeat the model name on every line.
        msg.agent_id.as_deref() != Some(selected)
    } else {
        true
    };
    let agent_badge = show_badge.then(|| agent_identity_badge(state, src));
    // Agent transcript entries should be stable (non-animated). The live "working" indicator is
    // represented by the dedicated breather row appended after the latest prompt.
    let ecg = ecg_indicator(state.metrics.frame_count, None, pulse_on, false);

    let mut header = ecg.to_string();
    if matches!(msg.channel, nit_core::AgentChannel::Broadcast) {
        header.push_str(" @all");
    }
    if let Some(agent_badge) = agent_badge.as_deref() {
        header.push_str(&format!(" [{agent_badge}]"));
    }

    let indent = 2usize.min(width.saturating_sub(1));
    // Keep at least one trailing column free so transcript text doesn't hug the right edge.
    let max_inner = width.saturating_sub(indent + 1).max(1);
    let indent_str = " ".repeat(indent);

    let mut out = Vec::new();
    for seg in wrap_visual_line(&header, max_inner) {
        // `wrap_visual_line` can leave trailing spaces when it wraps at a break point. If we
        // preserve those, selection logic may mistake agent rows for padded user prompt rows.
        let seg = seg.trim_end_matches(' ');
        out.push(ThreadRow {
            text: if seg.is_empty() {
                String::new()
            } else {
                format!("{indent_str}{seg}")
            },
            kind: ThreadRowKind::Agent,
        });
    }
    for line in text_lines {
        for segment in wrap_visual_line(line, max_inner) {
            let segment = segment.trim_end_matches(' ');
            out.push(ThreadRow {
                text: if segment.is_empty() {
                    String::new()
                } else {
                    format!("{indent_str}{segment}")
                },
                kind: ThreadRowKind::Agent,
            });
        }
    }
    out
}

fn agent_identity_badge(state: &AppState, agent_id: &str) -> String {
    let id_full = agent_id.trim();
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

fn breather_rows_for_user_prompt(
    state: &AppState,
    _swarm: Option<&SwarmRuntime>,
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
            .any(|turn| turn.agent_id == agent.id);
        if !has_active && !has_queued {
            continue;
        }
        let queued_in_mission = mission_ctx.is_some_and(|mission_id| {
            state.agents.queued_codex_turns.iter().any(|turn| {
                turn.agent_id == agent.id && turn.mission_id.as_deref() == Some(mission_id)
            })
        });
        let in_context = if let Some(mission_id) = mission_ctx {
            agent.current_mission.as_deref() == Some(mission_id) || queued_in_mission
        } else if let Some(selected_agent) = agent_ctx {
            agent.id == selected_agent
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
    let indent = 2usize.min(width.saturating_sub(1));
    let indent_str = " ".repeat(indent);
    let inner = width.saturating_sub(indent);

    let now = Instant::now();
    let mut swarm_assigned_ids: Vec<String> = Vec::new();
    let mut swarm_mission_id: Option<&str> = None;
    if let Some(mission_id) = mission_ctx {
        if let Some(mission) = state.agents.missions.iter().find(|m| m.id == mission_id) {
            let status = mission.status.to_ascii_uppercase();
            let is_final = matches!(status.as_str(), "DONE" | "FAILED" | "ERROR");
            if mission.swarm && !is_final {
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
    let label = if any_active {
        "Working ..."
    } else if any_queued {
        "Queued ..."
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

    // Table layout (stage gets the remaining space).
    let elap_w = 6usize;
    let hb_w = 4usize;
    let out_w = 4usize;
    let times_and_spacing = elap_w + hb_w + out_w + 4; // spaces between columns

    let desired_agent_w = ordered_ids
        .iter()
        .filter_map(|id| {
            state
                .agents
                .agents
                .iter()
                .find(|agent| agent.id == id.as_str())
                .map(|agent| agent_identity_badge(state, agent.id.as_str()))
        })
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(8)
        .clamp(6, 16);

    let max_agent_w = inner.saturating_sub(times_and_spacing + 10).max(1);
    let agent_w = desired_agent_w.clamp(1, max_agent_w);

    let fixed = agent_w + elap_w + hb_w + out_w + 4; // spaces between columns

    if inner.saturating_sub(fixed) < 10 {
        // Narrow fallback: keep it readable without a multi-column layout.
        for id in ordered_ids.iter() {
            let agent = state
                .agents
                .agents
                .iter()
                .find(|agent| agent.id == id.as_str());
            let badge = agent
                .map(|agent| agent_identity_badge(state, agent.id.as_str()))
                .unwrap_or_else(|| id.to_string());
            let turn = state.agents.active_turns.get(id.as_str());
            let queued_for_swarm = swarm_mission_id.is_some_and(|mid| {
                state.agents.queued_codex_turns.iter().any(|turn| {
                    turn.agent_id == id.as_str() && turn.mission_id.as_deref() == Some(mid)
                })
            });
            let queued_any = state
                .agents
                .queued_codex_turns
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
            let stage = agent
                .map(|agent| format_agent_stage_label(state, agent, stage_raw))
                .unwrap_or_else(|| stage_raw.to_string());

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

            rows.push(ThreadRow {
                text: pad_to_width(
                    &format!(
                        "{indent_str}{badge} stage={stage} elap={elapsed_s} hb={hb_s} out={out_s}"
                    ),
                    width,
                ),
                kind: ThreadRowKind::StatusRow,
            });
        }
        if let Some(mission_id) = swarm_mission_id {
            append_swarm_meta_footer_rows(&mut rows, state, mission_id, &indent_str, width, inner);
        }
        return rows;
    }

    let stage_w = inner.saturating_sub(fixed);
    rows.push(ThreadRow {
        text: pad_to_width(
            &format!(
                "{indent_str}{} {} {} {} {}",
                fit_left("AGENT", agent_w),
                fit_left("STAGE", stage_w),
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
            .map(|agent| agent_identity_badge(state, agent.id.as_str()))
            .unwrap_or_else(|| id.to_string());
        let turn = state.agents.active_turns.get(id.as_str());
        let queued_for_swarm =
            swarm_mission_id.is_some_and(|mid| {
                state.agents.queued_codex_turns.iter().any(|turn| {
                    turn.agent_id == id.as_str() && turn.mission_id.as_deref() == Some(mid)
                })
            });
        let queued_any = state
            .agents
            .queued_codex_turns
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
        let stage = agent
            .map(|agent| format_agent_stage_label(state, agent, stage_raw))
            .unwrap_or_else(|| stage_raw.to_string());

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

        rows.push(ThreadRow {
            text: pad_to_width(
                &format!(
                    "{indent_str}{} {} {} {} {}",
                    fit_left(&badge, agent_w),
                    fit_left(&stage, stage_w),
                    fit_right(&elapsed_s, elap_w),
                    fit_right(&hb_s, hb_w),
                    fit_right(&out_s, out_w),
                ),
                width,
            ),
            kind: ThreadRowKind::StatusRow,
        });
    }

    if let Some(mission_id) = swarm_mission_id {
        append_swarm_meta_footer_rows(&mut rows, state, mission_id, &indent_str, width, inner);
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
) {
    let metas = state
        .agents
        .messages
        .iter()
        .filter(|msg| {
            msg.mission_id.as_deref() == Some(mission_id)
                && msg.agent_id.as_deref() == Some("swarm")
                && msg.text.starts_with("Swarm ")
        })
        .map(|msg| msg.text.trim().to_string())
        .collect::<Vec<_>>();
    if metas.is_empty() {
        return;
    }
    let start = metas.len().saturating_sub(6);
    let metas = &metas[start..];

    let max_inner = inner.saturating_sub(1).max(1);
    rows.push(ThreadRow {
        text: pad_to_width(&format!("{indent_str}{}", "─".repeat(max_inner)), width),
        kind: ThreadRowKind::StatusHeader,
    });

    for meta in metas.iter() {
        for seg in wrap_visual_line(meta, max_inner) {
            let seg = seg.trim_end_matches(' ');
            let line = if seg.is_empty() {
                indent_str.to_string()
            } else {
                format!("{indent_str}{seg}")
            };
            rows.push(ThreadRow {
                text: pad_to_width(&line, width),
                kind: ThreadRowKind::StatusRow,
            });
        }
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

fn format_agent_stage_label(state: &AppState, agent: &AgentLane, stage: &str) -> String {
    if state.debug {
        return stage.to_string();
    }
    if stage == "token_count" {
        return format_token_count_stage(state, agent);
    }

    if let Some((prefix, inner_raw)) = split_stage_with_parens(stage) {
        match prefix {
            "item_started" | "item.started" => {
                return format!("Starting {}", humanize_stage_atom(inner_raw));
            }
            "item_completed" | "item.completed" => {
                return format!("Finished {}", humanize_stage_atom(inner_raw));
            }
            "tools/call" => {
                return match inner_raw {
                    "codex" => "Starting session".into(),
                    "codex-reply" => "Continuing session".into(),
                    _ => format!("Calling {}", humanize_stage_atom(inner_raw)),
                };
            }
            _ => {}
        }
    }

    match stage {
        "starting" => "Starting".into(),
        "queued" => "Queued".into(),
        "warning" => "Warning".into(),
        "error" => "Error".into(),
        "stream_error" | "stream.error" => "Stream error".into(),
        _ => sentence_case(&humanize_stage_atom(stage)),
    }
}

fn format_token_count_stage(state: &AppState, agent: &AgentLane) -> String {
    if !agent.is_codex() {
        return "Updating token usage".into();
    }

    let agent_id = agent.id.as_str();
    let mission_id = agent
        .current_mission
        .as_deref()
        .or_else(|| state.agents.selected_context_mission());

    let pct = if let Some(mission_id) = mission_id {
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
    let used = if let Some(mission_id) = mission_id {
        state
            .agents
            .codex_mission_used_tokens
            .get(mission_id)
            .and_then(|m| m.get(agent_id))
            .copied()
    } else {
        state.agents.codex_used_tokens.get(agent_id).copied()
    };
    let max = state
        .agents
        .codex_effective_context_window_tokens
        .get(agent_id)
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
mod tests {
    use super::{
        chat_input_scroll_metrics, chat_input_text_area, dim_bg_towards, ecg_indicator,
        format_message_rows, map_chat_input_point_to_cursor, thread_lines, thread_rows,
        wrap_input_with_cursor, wrap_visual_line, ThreadRow, ThreadRowKind,
        USER_PROMPT_BG_BACKGROUND_PCT,
    };
    use crate::theme::Theme;
    use nit_core::{
        AgentChannel, AgentLane, AgentMessage, AgentStatus, AppState, Buffer, MissionPhase,
        MissionRecord, QueuedCodexTurn,
    };
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;
    use std::path::PathBuf;
    use std::time::Instant;

    fn test_state() -> AppState {
        AppState::new(
            PathBuf::new(),
            Buffer::empty("editor", None),
            Buffer::empty("notes", None),
        )
    }

    #[test]
    fn wrap_input_with_cursor_expands_tabs_and_keeps_markdown_lines() {
        let markdown = "# Plan\n- item\tone\n```rust\n\tlet x = 1;\n```";
        let (lines, _, _) = wrap_input_with_cursor("", markdown, markdown.chars().count(), 80);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "# Plan");
        assert_eq!(lines[1], "- item  one");
        assert_eq!(lines[2], "```rust");
        assert_eq!(lines[3], "    let x = 1;");
        assert_eq!(lines[4], "```");
    }

    #[test]
    fn wrap_visual_line_handles_carriage_return_and_tabs() {
        let lines = wrap_visual_line("alpha\rbeta\tgamma", 80);
        assert_eq!(
            lines,
            vec!["alpha".to_string(), "beta    gamma".to_string()]
        );
    }

    #[test]
    fn user_message_renders_right_aligned_bubble() {
        let state = test_state();
        let msg = AgentMessage {
            at: "10:00:00".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: "line one\nline two".into(),
        };
        let rows = format_message_rows(&state, &msg, 48, true);
        assert!(rows.len() >= 6);
        assert!(matches!(rows[0].kind, ThreadRowKind::User));
        // Top padding row + label row.
        assert!(rows[0].text.trim().is_empty());
        assert_eq!(rows[1].text.trim(), "You");
        assert!(rows[2].text.trim_start().starts_with("line one"));
        assert!(rows[3].text.trim_start().starts_with("line two"));
        // Bottom padding row.
        assert!(rows[4].text.trim().is_empty());
        // Spacer row after the prompt to make turns easier to scan.
        assert!(rows.last().unwrap().text.is_empty());
    }

    #[test]
    fn ecg_indicator_freezes_when_agent_not_running() {
        let a = ecg_indicator(10, Some("coder"), true, false);
        let b = ecg_indicator(100, Some("coder"), false, false);
        assert_eq!(a, "▁▁▁▁▁▁");
        assert_eq!(b, "▁▁▁▁▁▁");
    }

    #[test]
    fn agent_messages_use_stable_ecg_header() {
        let mut state = test_state();
        state.agents.agents.push(AgentLane {
            id: "coder".into(),
            role: "Coder".into(),
            lane: "Lane B".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: AgentStatus::Running,
            heartbeat_age_secs: 1,
            queue_len: 1,
            current_mission: None,
            last_message: "active".into(),
        });
        let msg = AgentMessage {
            at: "10:00:00".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("coder".into()),
            mission_id: None,
            text: "working".into(),
        };
        state.metrics.frame_count = 3;
        let first_rows = format_message_rows(&state, &msg, 80, true);
        state.metrics.frame_count = 30;
        let second_rows = format_message_rows(&state, &msg, 80, true);
        // Stable header row (no animation on agent transcript lines).
        assert_eq!(first_rows[0].text, "  ▁▁▁▁▁▁");
        assert_eq!(second_rows[0].text, "  ▁▁▁▁▁▁");
        assert_eq!(first_rows[1].text, "  working");
        assert_eq!(second_rows[1].text, "  working");
        assert!(!first_rows[0].text.contains("[Coder]"));
        assert!(!second_rows[0].text.contains("[Coder]"));
    }

    #[test]
    fn agent_badge_hidden_when_single_agent_context_selected() {
        let mut state = test_state();
        state.agents.selected_mission = None;
        state.agents.selected_agent = Some("coder".into());
        state.agents.agents.push(AgentLane {
            id: "coder".into(),
            role: "Coder".into(),
            lane: "Lane B".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 1,
            queue_len: 0,
            current_mission: None,
            last_message: "idle".into(),
        });
        let msg = AgentMessage {
            at: "10:00:00".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("coder".into()),
            mission_id: None,
            text: "hello".into(),
        };
        let rows = format_message_rows(&state, &msg, 120, true);
        // Header row, then message.
        assert!(!rows[0].text.contains("[Coder]"));
        assert_eq!(rows[1].text, "  hello");
    }

    #[test]
    fn agent_header_includes_truncated_role_badge() {
        let mut state = test_state();
        // Force the badge to show even though we're rendering the selected agent.
        state.agents.selected_agent = Some("planner".into());
        state.agents.agents.push(AgentLane {
            id: "reviewer".into(),
            role: "UltraLongReviewerRoleName".into(),
            lane: "Lane C".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: AgentStatus::Running,
            heartbeat_age_secs: 1,
            queue_len: 0,
            current_mission: None,
            last_message: "active".into(),
        });
        let msg = AgentMessage {
            at: "10:00:00".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("reviewer".into()),
            mission_id: None,
            text: "ok".into(),
        };
        let row = format_message_rows(&state, &msg, 120, true)
            .into_iter()
            .find(|row| !row.text.trim().is_empty())
            .expect("row");
        assert!(row.text.contains("[UltraLongRe…/reviewer]"));
    }

    #[test]
    fn agent_ecg_renders_in_accent_color_and_text_is_cyan_theme() {
        let theme = Theme::default();
        let rows = [ThreadRow {
            text: "▁▁▁▁▁▁ hello".to_string(),
            kind: ThreadRowKind::Agent,
        }];
        let lines = thread_lines(rows.iter(), &theme);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans[0].style.fg, Some(theme.accent));
        assert_eq!(lines[0].spans[1].style.fg, Some(theme.title));
    }

    #[test]
    fn inline_command_style_is_light_gray_not_accent() {
        let theme = Theme::default();
        let rows = [ThreadRow {
            text: "  try `git status`".to_string(),
            kind: ThreadRowKind::Agent,
        }];
        let lines = thread_lines(rows.iter(), &theme);
        let code_span = lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "git status")
            .expect("expected inline code span");
        assert_eq!(code_span.style.fg, Some(theme.hl.operator));
        assert_ne!(code_span.style.fg, Some(theme.accent));
    }

    #[test]
    fn inline_number_style_uses_accent() {
        let theme = Theme::default();
        let rows = [ThreadRow {
            text: "  ctx=`600`".to_string(),
            kind: ThreadRowKind::Agent,
        }];
        let lines = thread_lines(rows.iter(), &theme);
        let num_span = lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "600")
            .expect("expected numeric inline code span");
        assert_eq!(num_span.style.fg, Some(theme.accent));
    }

    #[test]
    fn plain_text_paths_commands_and_numbers_are_highlighted() {
        let theme = Theme::default();
        let rows = [ThreadRow {
            text: "  see crates/nit-tui/src/widgets/agent_ops_view.rs:906; run cargo; wait 600s"
                .to_string(),
            kind: ThreadRowKind::Agent,
        }];
        let lines = thread_lines(rows.iter(), &theme);
        let line = &lines[0];

        let path_span = line
            .spans
            .iter()
            .find(|span| {
                span.content
                    .as_ref()
                    .contains("crates/nit-tui/src/widgets/agent_ops_view.rs:906")
            })
            .expect("expected path span");
        assert_eq!(path_span.style.fg, Some(theme.hl.link));
        assert!(path_span.style.add_modifier.contains(Modifier::UNDERLINED));

        let cargo_span = line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "cargo")
            .expect("expected command span");
        assert_eq!(cargo_span.style.fg, Some(theme.hl.operator));

        let num_span = line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "600s")
            .expect("expected number span");
        assert_eq!(num_span.style.fg, Some(theme.accent));
    }

    #[test]
    fn verify_result_outcome_is_color_coded() {
        let theme = Theme::default();
        let rows = [
            ThreadRow {
                text: "  VERIFY result: FAIL".to_string(),
                kind: ThreadRowKind::Agent,
            },
            ThreadRow {
                text: "  VERIFY result: SUCCESS".to_string(),
                kind: ThreadRowKind::Agent,
            },
            ThreadRow {
                text: "  VERIFY result: ERROR".to_string(),
                kind: ThreadRowKind::Agent,
            },
        ];
        let lines = thread_lines(rows.iter(), &theme);
        assert_eq!(lines.len(), 3);

        let fail_span = lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "FAIL")
            .expect("expected FAIL span");
        assert_eq!(fail_span.style.fg, Some(theme.error));

        let success_span = lines[1]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "SUCCESS")
            .expect("expected SUCCESS span");
        assert_eq!(success_span.style.fg, Some(theme.success));

        let other_span = lines[2]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "ERROR")
            .expect("expected ERROR span");
        assert_eq!(other_span.style.fg, Some(theme.warning));
    }

    #[test]
    fn thread_rows_keep_chronological_order() {
        let mut state = test_state();
        state.agents.selected_mission = None;
        state.agents.selected_agent = None;
        state.agents.messages.clear();
        state.agents.messages.push(AgentMessage {
            at: "10:00:00".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("planner".into()),
            mission_id: None,
            text: "older message".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "10:00:01".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("coder".into()),
            mission_id: None,
            text: "newest message".into(),
        });

        let rows = thread_rows(&state, None, 100, true);
        assert!(!rows.is_empty());
        let flattened = rows
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let newest_pos = flattened.find("newest message").expect("newest present");
        let older_pos = flattened.find("older message").expect("older present");
        assert!(newest_pos > older_pos);
    }

    #[test]
    fn breather_row_renders_below_user_prompt_when_agent_running() {
        let mut state = test_state();
        state.agents.selected_mission = None;
        state.agents.selected_agent = Some("planner".into());
        state.agents.messages.clear();
        state.agents.agents.clear();
        state.agents.agents.push(AgentLane {
            id: "planner".into(),
            role: "Planner".into(),
            lane: "Lane A".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: AgentStatus::Running,
            heartbeat_age_secs: 0,
            queue_len: 1,
            current_mission: None,
            last_message: "active".into(),
        });
        let now = Instant::now();
        state.agents.active_turns.insert(
            "planner".into(),
            nit_core::state::AgentTurnState {
                started_at: now,
                last_heartbeat_at: now,
                last_output_at: now,
                stage: Some("starting".into()),
            },
        );
        state.agents.messages.push(AgentMessage {
            at: "10:00:02".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: "please plan".into(),
        });

        let rows = thread_rows(&state, None, 100, true);
        let breather_idx = rows
            .iter()
            .position(|row| matches!(row.kind, ThreadRowKind::Breather))
            .expect("breather row");
        let breather = rows.get(breather_idx).expect("breather row");
        assert!(matches!(breather.kind, ThreadRowKind::Breather));
        assert!(breather.text.contains("Working ..."));
        assert!(!breather.text.contains("Planner"));
    }

    #[test]
    fn breather_row_hidden_when_latest_message_is_agent() {
        let mut state = test_state();
        state.agents.selected_mission = None;
        state.agents.selected_agent = Some("planner".into());
        state.agents.messages.clear();
        state.agents.agents.clear();
        state.agents.agents.push(AgentLane {
            id: "planner".into(),
            role: "Planner".into(),
            lane: "Lane A".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 1,
            current_mission: None,
            last_message: "active".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "10:00:01".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: "please plan".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "10:00:02".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("planner".into()),
            mission_id: None,
            text: "on it".into(),
        });

        let rows = thread_rows(&state, None, 100, true);
        assert!(!rows
            .iter()
            .any(|row| matches!(row.kind, ThreadRowKind::Breather)));
    }

    #[test]
    fn breather_rows_include_multiple_running_agents() {
        let mut state = test_state();
        state.agents.selected_mission = None;
        state.agents.selected_agent = Some("planner".into());
        state.agents.messages.clear();
        state.agents.agents.clear();
        state.agents.agents.push(AgentLane {
            id: "planner".into(),
            role: "Planner".into(),
            lane: "Lane A".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: AgentStatus::Running,
            heartbeat_age_secs: 0,
            queue_len: 1,
            current_mission: None,
            last_message: "active".into(),
        });
        state.agents.agents.push(AgentLane {
            id: "coder".into(),
            role: "Coder".into(),
            lane: "Lane B".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: AgentStatus::Running,
            heartbeat_age_secs: 0,
            queue_len: 1,
            current_mission: None,
            last_message: "active".into(),
        });

        let now = Instant::now();
        state.agents.active_turns.insert(
            "planner".into(),
            nit_core::state::AgentTurnState {
                started_at: now,
                last_heartbeat_at: now,
                last_output_at: now,
                stage: Some("starting".into()),
            },
        );
        state.agents.active_turns.insert(
            "coder".into(),
            nit_core::state::AgentTurnState {
                started_at: now,
                last_heartbeat_at: now,
                last_output_at: now,
                stage: Some("tools/call(codex)".into()),
            },
        );

        state.agents.messages.push(AgentMessage {
            at: "10:00:02".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: "do the work".into(),
        });

        let rows = thread_rows(&state, None, 120, true);
        let flattened = rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>();
        assert!(flattened.iter().any(|line| line.contains("Working ...")));
        assert!(flattened.iter().any(|line| line.contains("Planner")));
        assert!(flattened.iter().any(|line| line.contains("Coder")));
        assert!(flattened
            .iter()
            .any(|line| line.contains("Starting session")));
    }

    #[test]
    fn breather_rows_show_when_prompt_queued_but_not_yet_started() {
        let mut state = test_state();
        state.agents.selected_mission = None;
        state.agents.selected_agent = Some("planner".into());
        state.agents.messages.clear();
        state.agents.agents.clear();
        state.agents.agents.push(AgentLane {
            id: "planner".into(),
            role: "Planner".into(),
            lane: "Lane A".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: AgentStatus::Waiting,
            heartbeat_age_secs: 0,
            queue_len: 1,
            current_mission: None,
            last_message: "queued".into(),
        });
        state.agents.queued_codex_turns.push_back(QueuedCodexTurn {
            agent_id: "planner".into(),
            mission_id: None,
            prompt: "do the thing".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "10:00:01".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("planner".into()),
            mission_id: None,
            text: "finished previous turn".into(),
        });

        let rows = thread_rows(&state, None, 120, true);
        let flattened = rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>();
        assert!(flattened.iter().any(|line| line.contains("Queued ...")));
        assert!(flattened.iter().any(|line| line.contains("Queued")));
    }

    #[test]
    fn breather_rows_include_swarm_assigned_agents_even_when_idle() {
        let mut state = test_state();
        state.agents.messages.clear();
        state.agents.missions.clear();
        state.agents.agents.clear();
        state.agents.active_turns.clear();

        state.agents.missions.push(MissionRecord {
            id: "mis-001".into(),
            title: "Swarm: demo".into(),
            phase: MissionPhase::Plan,
            swarm: true,
            assigned_agents: vec!["planner".into(), "coder".into()],
            status: "PLAN".into(),
            updated_at: "t+0".into(),
        });
        state.agents.selected_mission = Some("mis-001".into());
        state.agents.mission_selected = 0;
        state.agents.selected_agent = Some("planner".into());

        state.agents.agents.push(AgentLane {
            id: "planner".into(),
            role: "Planner".into(),
            lane: "Lane A".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: AgentStatus::Running,
            heartbeat_age_secs: 0,
            queue_len: 1,
            current_mission: Some("mis-001".into()),
            last_message: "active".into(),
        });
        state.agents.agents.push(AgentLane {
            id: "coder".into(),
            role: "Coder".into(),
            lane: "Lane B".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: Some("mis-001".into()),
            last_message: "idle".into(),
        });

        let now = Instant::now();
        state.agents.active_turns.insert(
            "planner".into(),
            nit_core::state::AgentTurnState {
                started_at: now,
                last_heartbeat_at: now,
                last_output_at: now,
                stage: Some("starting".into()),
            },
        );

        state.agents.messages.push(AgentMessage {
            at: "10:00:02".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: Some("mis-001".into()),
            text: "do the work".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "10:00:03".into(),
            channel: AgentChannel::Broadcast,
            agent_id: Some("swarm".into()),
            mission_id: Some("mis-001".into()),
            text: "Swarm template: lab | integrator: planner | verifier: coder | gates: rust-ci"
                .into(),
        });

        let rows = thread_rows(&state, None, 120, true);
        let flattened = rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>();
        assert!(flattened.iter().any(|line| line.contains("Working ...")));
        assert!(flattened.iter().any(|line| line.contains("Planner")));
        assert!(flattened.iter().any(|line| line.contains("Coder")));
        assert!(flattened.iter().any(|line| line.contains("Swarm pending")));
        assert!(rows.iter().any(|row| {
            matches!(row.kind, ThreadRowKind::StatusRow) && row.text.contains("Swarm template:")
        }));
        assert!(!rows.iter().any(|row| {
            matches!(row.kind, ThreadRowKind::Agent) && row.text.contains("Swarm template:")
        }));
    }

    #[test]
    fn breather_rows_show_done_when_swarm_idle_and_all_assigned_reported() {
        let mut state = test_state();
        state.agents.messages.clear();
        state.agents.missions.clear();
        state.agents.agents.clear();
        state.agents.active_turns.clear();
        state.agents.queued_codex_turns.clear();

        state.agents.missions.push(MissionRecord {
            id: "mis-001".into(),
            title: "Swarm: demo".into(),
            phase: MissionPhase::Plan,
            swarm: true,
            assigned_agents: vec!["planner".into(), "coder".into()],
            status: "PLAN".into(),
            updated_at: "t+0".into(),
        });
        state.agents.selected_mission = Some("mis-001".into());
        state.agents.mission_selected = 0;
        state.agents.selected_agent = Some("planner".into());

        state.agents.agents.push(AgentLane {
            id: "planner".into(),
            role: "Planner".into(),
            lane: "Lane A".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: Some("mis-001".into()),
            last_message: "done".into(),
        });
        state.agents.agents.push(AgentLane {
            id: "coder".into(),
            role: "Coder".into(),
            lane: "Lane B".into(),
            kind: nit_core::AgentLaneKind::Mock,
            status: AgentStatus::Idle,
            heartbeat_age_secs: 0,
            queue_len: 0,
            current_mission: Some("mis-001".into()),
            last_message: "done".into(),
        });

        state.agents.messages.push(AgentMessage {
            at: "10:00:01".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("planner".into()),
            mission_id: Some("mis-001".into()),
            text: "planner output".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "10:00:02".into(),
            channel: AgentChannel::Agent,
            agent_id: Some("coder".into()),
            mission_id: Some("mis-001".into()),
            text: "coder output".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "10:00:03".into(),
            channel: AgentChannel::Broadcast,
            agent_id: Some("swarm".into()),
            mission_id: Some("mis-001".into()),
            text: "Swarm template: lab | integrator: planner | verifier: coder | gates: rust-ci"
                .into(),
        });

        let rows = thread_rows(&state, None, 120, true);
        assert!(rows.iter().any(|row| {
            matches!(row.kind, ThreadRowKind::Breather) && row.text.contains("▁▁▁▁▁▁ Done")
        }));
        assert!(!rows.iter().any(|row| row.text.contains("Working ...")));
    }

    #[test]
    fn chat_input_height_grows_with_text_but_stays_capped() {
        let mut state = test_state();
        state.agents.chat_input = (0..48).map(|i| format!("line-{i}\n")).collect::<String>();
        state.agents.chat_input_cursor = state.agents.chat_input.chars().count();
        let area = Rect {
            x: 0,
            y: 0,
            width: 90,
            height: 28,
        };
        let metrics = chat_input_scroll_metrics(area, &state).expect("chat metrics");
        assert!(metrics.visible_height >= 4);
        assert!(metrics.visible_height <= 12);
        assert!(metrics.visible_height < area.height as usize);
        assert!(metrics.max_scroll > 0);
    }

    #[test]
    fn map_chat_input_click_to_cursor_index() {
        let mut state = test_state();
        state.agents.chat_input = "hello\nworld".into();
        state.agents.chat_input_cursor = 0;
        let area = Rect {
            x: 0,
            y: 0,
            width: 60,
            height: 16,
        };
        let input_area = chat_input_text_area(area, &state).expect("input area");

        let top = map_chat_input_point_to_cursor(
            area,
            &state,
            input_area.x.saturating_add(4),
            input_area.y,
            false,
        )
        .expect("cursor from top row");
        assert_eq!(top, 4);

        let second = map_chat_input_point_to_cursor(
            area,
            &state,
            input_area.x.saturating_add(2),
            input_area.y.saturating_add(1),
            false,
        )
        .expect("cursor from second row");
        assert_eq!(second, 8);
    }

    #[test]
    fn user_bubble_rows_use_dim_prompt_bg_and_highlight_you_label() {
        let theme = Theme::default();
        let prompt_bg = dim_bg_towards(
            theme.cursor_line_bg,
            theme.background,
            USER_PROMPT_BG_BACKGROUND_PCT,
        );
        let rows = [
            ThreadRow {
                text: "  You      ".to_string(),
                kind: ThreadRowKind::User,
            },
            ThreadRow {
                text: "  hello    ".to_string(),
                kind: ThreadRowKind::User,
            },
        ];
        let lines = thread_lines(rows.iter(), &theme);

        assert!(lines[0]
            .spans
            .iter()
            .all(|span| span.style.bg == Some(prompt_bg)));
        assert!(lines[1]
            .spans
            .iter()
            .all(|span| span.style.bg == Some(prompt_bg)));
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|span| span.content.as_ref().contains("You")
                    && span.style.fg == Some(theme.accent)),
            "expected 'You' label span to use accent color"
        );
    }
}
