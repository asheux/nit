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
                    .fg(theme.accent)
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
    let (cached_rows_len, last_message_was_user) = refresh_thread_rows_cache(state, thread_width);
    let thread_height = layout.thread_area.height.max(1) as usize;
    let breather = if last_message_was_user {
        breather_rows_for_user_prompt(state, pulse_on, thread_width)
    } else {
        Vec::new()
    };
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
                spans.push(Span::styled(pre, agent_style));
            }
            spans.push(Span::styled(badge, badge_style));
            if !post.is_empty() {
                spans.push(Span::styled(post, agent_style));
            }
        } else {
            spans.push(Span::styled(rest, agent_style));
        }
    } else {
        spans.push(Span::styled(body, agent_style));
    }
    Line::from(spans)
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

fn thread_rows(state: &AppState, width: usize, pulse_on: bool) -> Vec<ThreadRow> {
    let mission = state.agents.selected_context_mission();
    let agent = state.agents.selected_context_agent();
    let mut rows = Vec::new();
    let mut last_message_was_user = false;
    for msg in state.agents.messages.iter() {
        if !message_matches_context(msg, mission, agent) {
            continue;
        }
        last_message_was_user = msg.agent_id.is_none();
        rows.extend(format_message_rows(state, msg, width, pulse_on));
    }
    if last_message_was_user {
        rows.extend(breather_rows_for_user_prompt(state, pulse_on, width));
    }
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

fn breather_rows_for_user_prompt(state: &AppState, pulse_on: bool, width: usize) -> Vec<ThreadRow> {
    let Some(agent) = active_running_agent(state) else {
        return Vec::new();
    };
    let agent_type = agent.role.trim();
    let agent_type = if agent_type.is_empty() {
        agent.id.as_str()
    } else {
        agent_type
    };
    let ecg = ecg_indicator(state.metrics.frame_count, Some(&agent.id), pulse_on, true);
    let ctx_pct = if agent.is_codex() {
        let mission_id = agent.current_mission.as_deref();
        mission_id
            .and_then(|mid| {
                state
                    .agents
                    .codex_mission_context_remaining_pct
                    .get(mid)
                    .and_then(|m| m.get(&agent.id))
                    .copied()
            })
            .or_else(|| {
                state
                    .agents
                    .codex_context_remaining_pct
                    .get(&agent.id)
                    .copied()
            })
    } else {
        None
    };
    let ctx_suffix = ctx_pct
        .map(|pct| format!(" {pct}% ctx"))
        .unwrap_or_default();

    let mut rows = Vec::new();
    rows.push(ThreadRow {
        text: format!("{ecg} [{agent_type}] Working{ctx_suffix}"),
        kind: ThreadRowKind::Breather,
    });

    let Some(turn) = state.agents.active_turns.get(&agent.id) else {
        return rows;
    };

    let width = width.max(1);
    let indent = 2usize.min(width.saturating_sub(1));
    let indent_str = " ".repeat(indent);
    let inner = width.saturating_sub(indent);

    let now = Instant::now();
    let stage = turn.stage.as_deref().unwrap_or("starting");
    let elapsed = now.checked_duration_since(turn.started_at);
    let hb_age = now
        .checked_duration_since(turn.last_heartbeat_at)
        .map(|d| d.as_secs());
    let out_age = now
        .checked_duration_since(turn.last_output_at)
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

    // Table layout (stage gets the remaining space).
    let elap_w = 6usize;
    let hb_w = 4usize;
    let out_w = 4usize;
    let fixed = elap_w + hb_w + out_w + 3; // spaces between columns

    if inner.saturating_sub(fixed) < 10 {
        // Narrow fallback: keep it readable without a multi-column layout.
        rows.push(ThreadRow {
            text: pad_to_width(
                &format!("{indent_str}stage={stage} elap={elapsed_s} hb={hb_s} out={out_s}"),
                width,
            ),
            kind: ThreadRowKind::StatusRow,
        });
        return rows;
    }

    let stage_w = inner.saturating_sub(fixed);
    rows.push(ThreadRow {
        text: pad_to_width(
            &format!(
                "{indent_str}{} {} {} {}",
                fit_left("STAGE", stage_w),
                fit_right("ELAP", elap_w),
                fit_right("HB", hb_w),
                fit_right("OUT", out_w),
            ),
            width,
        ),
        kind: ThreadRowKind::StatusHeader,
    });
    rows.push(ThreadRow {
        text: pad_to_width(
            &format!(
                "{indent_str}{} {} {} {}",
                fit_left(stage, stage_w),
                fit_right(&elapsed_s, elap_w),
                fit_right(&hb_s, hb_w),
                fit_right(&out_s, out_w),
            ),
            width,
        ),
        kind: ThreadRowKind::StatusRow,
    });

    rows
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
        chat_input_scroll_metrics, chat_input_text_area, dim_bg_towards, ecg_indicator,
        format_message_rows, map_chat_input_point_to_cursor, thread_lines, thread_rows,
        wrap_input_with_cursor, wrap_visual_line, ThreadRow, ThreadRowKind,
        USER_PROMPT_BG_BACKGROUND_PCT,
    };
    use crate::theme::Theme;
    use nit_core::{AgentChannel, AgentLane, AgentMessage, AgentStatus, AppState, Buffer};
    use ratatui::layout::Rect;
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
        let rows = vec![ThreadRow {
            text: "▁▁▁▁▁▁ hello".to_string(),
            kind: ThreadRowKind::Agent,
        }];
        let lines = thread_lines(rows.iter(), &theme);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans[0].style.fg, Some(theme.accent));
        assert_eq!(lines[0].spans[1].style.fg, Some(theme.title));
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
            kind: nit_core::AgentLaneKind::Mock,
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
            kind: nit_core::AgentLaneKind::Mock,
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
    fn user_bubble_rows_use_dim_prompt_bg_and_highlight_you_label() {
        let theme = Theme::default();
        let prompt_bg = dim_bg_towards(
            theme.cursor_line_bg,
            theme.background,
            USER_PROMPT_BG_BACKGROUND_PCT,
        );
        let rows = vec![
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
