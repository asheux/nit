use nit_core::{AgentLane, AgentMessage, AgentStatus, AppState, PaneId, UiSelectionPane};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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

#[derive(Copy, Clone)]
enum ThreadRowKind {
    User,
    Agent,
    Breather,
}

struct ThreadRow {
    text: String,
    kind: ThreadRowKind,
}

const TAB_STOP: usize = 4;
const CHAT_INPUT_MAX_INNER_LINES: usize = 12;
const CHAT_INPUT_MAX_INNER_LINES_COMPACT: usize = 8;
const CHAT_INPUT_SCROLL_AUTO: usize = usize::MAX;
const AGENT_BADGE_MAX_CHARS: usize = 24;

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

    let Some(layout) = compute_console_layout(area, state) else {
        return None;
    };
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
    let label_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let mission_style = if mission.is_some() {
        Style::default()
            .fg(theme.accent)
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
        Span::styled("context ", label_style),
        Span::styled("mission=", label_style),
        Span::styled(mission.unwrap_or("--"), mission_style),
        Span::styled("  ", label_style),
        Span::styled("agent=", label_style),
        Span::styled(agent.unwrap_or("--"), agent_style),
    ]);
    frame.render_widget(Paragraph::new(context_line), chunks[0]);

    let pulse_on = pulse_on(state);
    let thread_width = layout.thread_area.width.max(1) as usize;
    let rows = thread_rows(state, thread_width, pulse_on);
    let thread_height = layout.thread_area.height.max(1) as usize;
    let max_scroll = rows.len().saturating_sub(thread_height);
    state.agents.console_scroll = if state.agents.console_scroll == usize::MAX {
        max_scroll
    } else {
        state.agents.console_scroll.min(max_scroll)
    };
    let scroll_usize = state.agents.console_scroll;
    let lines = thread_lines(rows, theme, pulse_on);
    let visible: Vec<Line<'static>> = lines
        .into_iter()
        .skip(scroll_usize)
        .take(thread_height)
        .collect();
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
        let input_block = Block::default()
            .borders(Borders::ALL)
            .title("CHAT BOX")
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
    thread_rows(state, width.max(1), pulse_on(state))
        .into_iter()
        .map(|row| row.text)
        .collect()
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

fn thread_lines(rows: Vec<ThreadRow>, theme: &Theme, _pulse_on: bool) -> Vec<Line<'static>> {
    rows.into_iter()
        .map(|row| match row.kind {
            ThreadRowKind::User => user_line_with_cyan_edges(row.text, theme),
            ThreadRowKind::Agent => Line::from(Span::styled(
                row.text,
                Style::default().fg(theme.foreground),
            )),
            ThreadRowKind::Breather => {
                let mut parts = row.text.splitn(2, ' ');
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
                            "".to_string()
                        } else {
                            format!(" {rest}")
                        },
                        Style::default().fg(theme.foreground),
                    ),
                ])
            }
        })
        .collect()
}

fn user_line_with_cyan_edges(text: String, theme: &Theme) -> Line<'static> {
    let leading_spaces = text.chars().take_while(|ch| *ch == ' ').count();
    let mut spans = Vec::new();
    if leading_spaces > 0 {
        spans.push(Span::styled(
            " ".repeat(leading_spaces),
            Style::default().fg(theme.foreground),
        ));
    }
    let body = text[leading_spaces..].to_string();
    if body.starts_with('+') && body.ends_with('+') && body.chars().all(|ch| ch == '+' || ch == '-')
    {
        spans.push(Span::styled(body, Style::default().fg(Color::Cyan)));
        return Line::from(spans);
    }
    if body.starts_with('|') && body.ends_with('|') && body.len() >= 2 {
        spans.push(Span::styled("|", Style::default().fg(Color::Cyan)));
        let inner = body[1..body.len().saturating_sub(1)].to_string();
        let inner_style = if inner.trim() == "You" {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };
        spans.push(Span::styled(inner, inner_style));
        spans.push(Span::styled("|", Style::default().fg(Color::Cyan)));
        return Line::from(spans);
    }
    spans.push(Span::styled(body, Style::default().fg(theme.foreground)));
    Line::from(spans)
}

fn thread_rows(state: &AppState, width: usize, pulse_on: bool) -> Vec<ThreadRow> {
    let mission = state.agents.selected_context_mission();
    let agent = state.agents.selected_context_agent();
    let filtered = state
        .agents
        .messages
        .iter()
        .filter(|msg| message_matches_context(msg, mission, agent))
        .collect::<Vec<_>>();
    let mut rows = filtered
        .iter()
        .flat_map(|msg| format_message_rows(state, msg, width, pulse_on))
        .collect::<Vec<_>>();
    if should_show_breather_after_prompt(&filtered) {
        if let Some(row) = breather_row_for_user_prompt(state, pulse_on) {
            rows.push(row);
        }
    }
    rows
}

fn should_show_breather_after_prompt(messages: &[&AgentMessage]) -> bool {
    let Some(last) = messages.last() else {
        return false;
    };
    last.agent_id.is_none()
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

    let src = msg.agent_id.as_deref().unwrap_or("agent");
    let agent_badge = agent_identity_badge(state, src);
    let ecg = ecg_indicator(
        state.metrics.frame_count,
        msg.agent_id.as_deref(),
        pulse_on,
        agent_is_running(state, msg.agent_id.as_deref()),
    );
    let channel = if matches!(msg.channel, nit_core::AgentChannel::Broadcast) {
        "@all "
    } else {
        ""
    };
    text_lines
        .into_iter()
        .enumerate()
        .flat_map(|(idx, line)| {
            let rendered = if idx == 0 {
                format!("{ecg} [{}] {channel}[{agent_badge}]: {line}", msg.at)
            } else {
                format!("       {line}")
            };
            wrap_visual_line(&rendered, width)
                .into_iter()
                .map(|segment| ThreadRow {
                    text: segment,
                    kind: ThreadRowKind::Agent,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn agent_identity_badge(state: &AppState, agent_id: &str) -> String {
    let id = truncate_label(agent_id, 10);
    let Some(agent) = state
        .agents
        .agents
        .iter()
        .find(|agent| agent.id == agent_id)
    else {
        return id;
    };
    let role = truncate_label(agent.role.trim(), 12);
    if role.is_empty() {
        return id;
    }
    if role.eq_ignore_ascii_case(agent_id) {
        return role;
    }
    truncate_label(&format!("{role}/{id}"), AGENT_BADGE_MAX_CHARS)
}

fn breather_row_for_user_prompt(state: &AppState, pulse_on: bool) -> Option<ThreadRow> {
    let Some(agent) = active_running_agent(state) else {
        return None;
    };
    let agent_type = truncate_label(agent.role.trim(), 14);
    let agent_type = if agent_type.is_empty() {
        truncate_label(&agent.id, 14)
    } else {
        agent_type
    };
    let ecg = ecg_indicator(state.metrics.frame_count, Some(&agent.id), pulse_on, true);
    Some(ThreadRow {
        text: format!("{ecg} [{agent_type}] Working"),
        kind: ThreadRowKind::Breather,
    })
}

fn active_running_agent<'a>(state: &'a AppState) -> Option<&'a AgentLane> {
    if let Some(selected) = state.agents.selected_context_agent() {
        if let Some(agent) = state
            .agents
            .agents
            .iter()
            .find(|agent| agent.id == selected && matches!(agent.status, AgentStatus::Running))
        {
            return Some(agent);
        }
    }
    state
        .agents
        .agents
        .iter()
        .find(|agent| matches!(agent.status, AgentStatus::Running))
}

fn truncate_label(input: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut count = 0usize;
    for ch in input.chars() {
        if count >= max_chars {
            break;
        }
        out.push(ch);
        count += 1;
    }
    if input.chars().count() > max_chars {
        if max_chars == 1 {
            return "…".to_string();
        }
        out.pop();
        out.push('…');
    }
    out
}

fn format_user_bubble_rows(_msg: &AgentMessage, text_lines: &[&str], width: usize) -> Vec<String> {
    let width = width.max(1);
    if width < 8 {
        let plain = text_lines
            .iter()
            .enumerate()
            .flat_map(|(idx, line)| {
                let rendered = if idx == 0 {
                    format!("You: {}", line)
                } else {
                    (*line).to_string()
                };
                wrap_visual_line(&rendered, width)
            })
            .collect::<Vec<_>>();
        return right_align_block(plain, width);
    }

    let max_inner = width.saturating_sub(4).max(1);
    let mut body_lines = Vec::new();
    body_lines.extend(wrap_visual_line("You", max_inner));
    for line in text_lines {
        body_lines.extend(wrap_visual_line(line, max_inner));
    }
    let inner_width = body_lines
        .iter()
        .map(|line| UnicodeWidthStr::width(line.as_str()))
        .max()
        .unwrap_or(0)
        .max(1);
    let border = "-".repeat(inner_width + 2);
    let mut bubble = Vec::with_capacity(body_lines.len() + 2);
    bubble.push(format!("+{border}+"));
    for line in body_lines {
        let pad = inner_width.saturating_sub(UnicodeWidthStr::width(line.as_str()));
        bubble.push(format!("| {line}{} |", " ".repeat(pad)));
    }
    bubble.push(format!("+{border}+"));
    right_align_block(bubble, width)
}

fn agent_is_running(state: &AppState, agent_id: Option<&str>) -> bool {
    match agent_id {
        Some(agent_id) => state
            .agents
            .agents
            .iter()
            .any(|agent| agent.id == agent_id && matches!(agent.status, AgentStatus::Running)),
        None => state
            .agents
            .agents
            .iter()
            .any(|agent| matches!(agent.status, AgentStatus::Running)),
    }
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

fn right_align_block(lines: Vec<String>, width: usize) -> Vec<String> {
    if lines.is_empty() || width == 0 {
        return lines;
    }
    let block_width = lines
        .iter()
        .map(|line| UnicodeWidthStr::width(line.as_str()))
        .max()
        .unwrap_or(0);
    let left_pad = width.saturating_sub(block_width);
    lines
        .into_iter()
        .map(|line| {
            let mut out = String::with_capacity(left_pad + line.len());
            out.push_str(&" ".repeat(left_pad));
            out.push_str(&line);
            out
        })
        .collect()
}

fn pulse_on(state: &AppState) -> bool {
    (state.metrics.frame_count / 6) % 2 == 0
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
        chat_input_scroll_metrics, chat_input_text_area, ecg_indicator, format_message_rows,
        map_chat_input_point_to_cursor, thread_lines, thread_rows, wrap_input_with_cursor,
        wrap_visual_line, ThreadRow, ThreadRowKind,
    };
    use crate::theme::Theme;
    use nit_core::{AgentChannel, AgentLane, AgentMessage, AgentStatus, AppState, Buffer};
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use std::path::PathBuf;

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
        assert!(rows.len() >= 5);
        assert!(matches!(rows[0].kind, ThreadRowKind::User));
        assert!(rows[0].text.trim_start().starts_with('+'));
        assert!(rows[1].text.trim_start().starts_with("| You"));
        assert!(rows[rows.len().saturating_sub(2)]
            .text
            .trim_start()
            .starts_with('+'));
        let left_pad_first = rows[0].text.chars().take_while(|ch| *ch == ' ').count();
        let left_pad_mid = rows[1].text.chars().take_while(|ch| *ch == ' ').count();
        assert_eq!(left_pad_first, left_pad_mid);
    }

    #[test]
    fn ecg_indicator_freezes_when_agent_not_running() {
        let a = ecg_indicator(10, Some("coder"), true, false);
        let b = ecg_indicator(100, Some("coder"), false, false);
        assert_eq!(a, "▁▁▁▁▁▁");
        assert_eq!(b, "▁▁▁▁▁▁");
    }

    #[test]
    fn agent_indicator_animates_when_agent_running() {
        let mut state = test_state();
        state.agents.agents.push(AgentLane {
            id: "coder".into(),
            role: "Coder".into(),
            lane: "Lane B".into(),
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
        let first = format_message_rows(&state, &msg, 80, true)[0].text.clone();
        state.metrics.frame_count = 30;
        let second = format_message_rows(&state, &msg, 80, true)[0].text.clone();
        assert_ne!(first, second);
        assert!(first.contains("[10:00:00] [Coder]:"));
        assert!(second.contains("[10:00:00] [Coder]:"));
    }

    #[test]
    fn agent_header_includes_truncated_role_badge() {
        let mut state = test_state();
        state.agents.agents.push(AgentLane {
            id: "reviewer".into(),
            role: "UltraLongReviewerRoleName".into(),
            lane: "Lane C".into(),
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
            .next()
            .expect("row");
        assert!(row.text.contains("[UltraLongRe…/reviewer]:"));
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

        let rows = thread_rows(&state, 100, true);
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
            status: AgentStatus::Running,
            heartbeat_age_secs: 0,
            queue_len: 1,
            current_mission: None,
            last_message: "active".into(),
        });
        state.agents.messages.push(AgentMessage {
            at: "10:00:02".into(),
            channel: AgentChannel::Agent,
            agent_id: None,
            mission_id: None,
            text: "please plan".into(),
        });

        let rows = thread_rows(&state, 100, true);
        let breather_idx = rows
            .iter()
            .position(|row| matches!(row.kind, ThreadRowKind::Breather))
            .expect("breather row");
        let breather = rows.get(breather_idx).expect("breather row");
        assert!(matches!(breather.kind, ThreadRowKind::Breather));
        assert!(breather.text.contains("[Planner] Working"));
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
            status: AgentStatus::Running,
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

        let rows = thread_rows(&state, 100, true);
        assert!(!rows.iter().any(|row| row.text.contains("] Working")));
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
    fn user_bubble_edges_render_in_cyan() {
        let theme = Theme::default();
        let lines = thread_lines(
            vec![
                ThreadRow {
                    text: "  +------+".to_string(),
                    kind: ThreadRowKind::User,
                },
                ThreadRow {
                    text: "  | hello |".to_string(),
                    kind: ThreadRowKind::User,
                },
            ],
            &theme,
            false,
        );

        assert_eq!(lines[0].spans[1].style.fg, Some(Color::Cyan));
        assert_eq!(lines[1].spans[1].style.fg, Some(Color::Cyan));
        assert_eq!(lines[1].spans[3].style.fg, Some(Color::Cyan));
    }
}
