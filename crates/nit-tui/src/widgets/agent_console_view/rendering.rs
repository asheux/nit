//! Pane rendering split out of `agent_console_view`.
//!
//! `render_pane` paints one multipane chat pane: a one-line agent /
//! ctx header, the scrollable thread area (rows produced by
//! `build_pane_thread_rows_with_breathers_for_pane`), and the chat
//! input box. This bypasses the global `console_rows_cache` (cache
//! key is shared across panes) and rebuilds rows from
//! `state.agents.messages` filtered by the pane's lane id every frame.

use nit_core::{AgentConsoleRow as ThreadRow, AppState, PaneSession};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};

use super::breather::build_pane_thread_rows_with_breathers_for_pane;
use super::{
    chat_input_display_pos_for_char_idx, compute_pane_layout, cursor_visible, dim_bg_towards,
    format_context_text, highlight_plain_line, paint_pane_thread_selection, resolve_context_pct,
    resolve_context_used, thread_lines, ChatCursor,
};
use crate::swarm::SwarmRuntime;
use crate::theme::Theme;

pub fn render_pane(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    swarm: Option<&SwarmRuntime>,
    theme: &Theme,
    pane: &PaneSession,
    focused: bool,
) -> Option<ChatCursor> {
    let layout = compute_pane_layout(area, &pane.chat_input, pane.chat_input_cursor)?;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(layout.input_chunk.height),
        ])
        .split(area);

    let agent = Some(pane.agent_id.as_str());
    // Render-side mission falls back to the synthetic chat id so the
    // pane filter sees a non-None mission for fresh panes — closes the
    // (agent_id.is_none() && mission_id.is_none()) leak in the matcher
    // for default-chat user prompts in another pane.
    let synthetic = (!pane.chat_mission_id.is_empty()).then_some(pane.chat_mission_id.as_str());
    let mission = pane.mission_id.as_deref().or(synthetic);
    let codex_ctx_pct = agent.and_then(|id| resolve_context_pct(state, id, mission));
    let codex_ctx_used = agent.and_then(|id| resolve_context_used(state, id, mission));
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
    // BOLD mission style only when a real swarm mission is attached;
    // the synthetic chat id should not light up the agent= label.
    let mission_style = if pane.mission_id.is_some() {
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD)
    } else {
        label_style
    };
    let ctx_text = format_context_text(codex_ctx_pct, codex_ctx_used, codex_ctx_max);
    let ctx_any = codex_ctx_pct.is_some() || codex_ctx_used.is_some() || codex_ctx_max.is_some();
    let ctx_style = if ctx_any {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        label_style
    };
    let context_line = Line::from(vec![
        Span::styled("agent=", label_style),
        Span::styled(pane.agent_id.clone(), mission_style),
        Span::styled("     ", label_style),
        Span::styled("ctx=", label_style),
        Span::styled(ctx_text, ctx_style),
    ]);
    frame.render_widget(Paragraph::new(context_line), chunks[0]);

    let thread_width = layout.thread_area.width.max(1) as usize;
    let thread_height = layout.thread_area.height.max(1) as usize;

    let thread_rows = build_pane_thread_rows_with_breathers_for_pane(
        state,
        swarm,
        Some(pane.pane_id),
        agent,
        mission,
        thread_width,
        !pane.has_run_mission,
    );

    let total_rows = thread_rows.len();
    let max_scroll = total_rows.saturating_sub(thread_height);
    // Cache the true max_scroll so the wheel / PgUp / PgDn handlers clamp to the
    // exact bound the renderer uses (PaneSession::chat_thread_last_max_scroll).
    if let Some(p) = state
        .multipane
        .as_mut()
        .and_then(|mp| mp.panes.iter_mut().find(|p| p.pane_id == pane.pane_id))
    {
        p.chat_thread_last_max_scroll = max_scroll;
    }
    let scroll = pane.chat_thread_scroll.min(max_scroll);
    let visible_row_refs: Vec<&ThreadRow> = thread_rows
        .iter()
        .skip(scroll)
        .take(thread_height)
        .collect();
    let visible_styled: Vec<Line<'static>> = thread_lines(visible_row_refs.iter().copied(), theme);
    let visible = match pane.selection.as_ref() {
        Some(sel) => paint_pane_thread_selection(
            visible_styled,
            &visible_row_refs,
            sel,
            scroll,
            theme.selection_bg,
        ),
        None => visible_styled,
    };
    frame.render_widget(
        Paragraph::new(visible).style(Style::default().bg(theme.background)),
        layout.thread_area,
    );

    let input_visible_text: Vec<String> = layout
        .input_lines_all
        .iter()
        .skip(layout.input_window_start)
        .take(layout.input_inner_height)
        .cloned()
        .collect();
    let input_len_chars = pane.chat_input.chars().count();
    let input_cursor = pane.chat_input_cursor.min(input_len_chars);
    let input_selection_range = pane
        .chat_input_selection_anchor
        .map(|anchor| anchor.min(input_len_chars))
        .and_then(|anchor| {
            if anchor == input_cursor {
                None
            } else {
                Some((anchor.min(input_cursor), anchor.max(input_cursor)))
            }
        });
    let (in_sel_start_line, in_sel_start_col, in_sel_end_line, in_sel_end_col) =
        input_selection_range
            .map(|(start, end)| {
                let wrap_width = layout.input_area.width.max(1) as usize;
                let (s_line, s_col) =
                    chat_input_display_pos_for_char_idx(&pane.chat_input, wrap_width, start);
                let (e_line, e_col) =
                    chat_input_display_pos_for_char_idx(&pane.chat_input, wrap_width, end);
                (s_line, s_col, e_line, e_col)
            })
            .unwrap_or((0, 0, 0, 0));
    let input_visible: Vec<Line<'static>> = input_visible_text
        .into_iter()
        .enumerate()
        .map(|(idx, text)| {
            if input_selection_range.is_none() {
                return Line::from(text);
            }
            let line_idx = layout.input_window_start.saturating_add(idx);
            if line_idx < in_sel_start_line || line_idx > in_sel_end_line {
                return Line::from(text);
            }
            let line_len = text.chars().count();
            let (sel_start, sel_end) = if in_sel_start_line == in_sel_end_line {
                (in_sel_start_col, in_sel_end_col)
            } else if line_idx == in_sel_start_line {
                (in_sel_start_col, line_len)
            } else if line_idx == in_sel_end_line {
                (0, in_sel_end_col)
            } else {
                (0, line_len)
            };
            let sel_start = sel_start.min(line_len);
            let sel_end = sel_end.min(line_len);
            highlight_plain_line(&text, sel_start, sel_end, theme.selection_bg)
        })
        .collect();
    let input_max_row = layout.input_inner_height.saturating_sub(1);
    if layout.input_boxed {
        let queued_count = state
            .agents
            .agents
            .iter()
            .find(|a| a.id == pane.agent_id)
            .map(|agent_lane| {
                let running = state.agents.active_turns.contains_key(&agent_lane.id);
                agent_lane.queue_len.saturating_sub(usize::from(running))
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
        // Match the single-pane chat-box badge colors so the operator
        // gets the same visual cue regardless of which UI surface
        // they're using. Single-pane uses `border_focused` for the
        // template badge and `hl.operator` for the mission badge.
        title_spans.push(Span::raw("  "));
        title_spans.push(Span::styled(
            format!(" t={} ", pane.swarm_template),
            Style::default()
                .fg(theme.background)
                .bg(theme.border_focused)
                .add_modifier(Modifier::BOLD),
        ));
        title_spans.push(Span::raw(" "));
        title_spans.push(Span::styled(
            format!(" m={} ", pane.swarm_mission),
            Style::default()
                .fg(theme.background)
                .bg(theme.hl.operator)
                .add_modifier(Modifier::BOLD),
        ));
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

    // Same blink gating as `render` (single-pane). `cursor_visible`
    // toggles every ~6 frames via the global frame counter, so the
    // pane caret pulses in lockstep with the editor caret.
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
