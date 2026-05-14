use nit_core::{AgentMessage, AppState, GlobalArchiveEntry, UiSelectionPane};
use nit_syntax::MappedLineSegment;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::swarm::SwarmRuntime;
use crate::theme::Theme;
use crate::widgets::agent_console_view;
use crate::widgets::agent_ops_view;
use crate::widgets::text_selection::apply_ui_selection;

mod markdown;
mod syntax;
mod table;
mod views;

use markdown::render_markdown_document;
use syntax::{is_json_code_lang, next_tab_width, syntax_highlighted_wrapped_segments};
use views::{
    run::{build_swarm_report_lines, build_swarm_verify_lines},
    task::build_swarm_task_lines,
};

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = (screen.width.saturating_sub(4)).clamp(40, 140);
    let height = (screen.height.saturating_sub(4)).clamp(12, 44);
    (width, height)
}

// Content area height excludes the chat input box. Prompts have no chat input and use the
// full inner height.
pub fn content_area_height(
    state: &AppState,
    swarm: &crate::swarm::SwarmRuntime,
    area: Rect,
) -> u16 {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let is_prompt =
        agent_ops_view::is_selected_artifact_prompt(state, swarm, area.width.max(1) as usize);
    if is_prompt || inner.height < 5 {
        return inner.height;
    }
    // Use the same dynamic input box height as the render function so that
    // scroll metrics stay in sync with the actual visible content area.
    let input_wrap_width = inner.width.saturating_sub(2) as usize;
    let popup_input = &state.agents.artifacts_popup_chat_input;
    let cursor_char_idx = state
        .agents
        .artifacts_popup_chat_cursor
        .min(popup_input.chars().count());
    let (input_lines_all, _, _) = agent_console_view::wrap_input_with_cursor(
        "",
        popup_input,
        cursor_char_idx,
        input_wrap_width,
    );
    let half_inner = (inner.height as usize) / 2;
    let max_inner_by_layout = inner.height.saturating_sub(4).max(1) as usize;
    let dynamic_max = half_inner
        .max(POPUP_CHAT_INPUT_MAX_INNER_LINES)
        .min(max_inner_by_layout);
    let input_inner_height = input_lines_all.len().max(1).min(dynamic_max);
    let input_box_height = (input_inner_height + 2) as u16;
    inner.height.saturating_sub(input_box_height)
}

// Source content from run.json via (run_path, artifact_index). Card-index
// lookups previously showed the wrong artifact (prompt vs reply).
fn build_lines_from_archive_entry(
    state: &AppState,
    entry: &GlobalArchiveEntry,
    theme: &Theme,
    width: usize,
) -> Option<Vec<Line<'static>>> {
    let path = std::path::PathBuf::from(&entry.run_path);
    let text = std::fs::read_to_string(&path).ok()?;
    let run: serde_json::Value = serde_json::from_str(&text).ok()?;
    let idx = entry.artifact_index;

    match entry.kind {
        "PROMPT" | "REPLY" | "PLAN" | "SYNTH" => {
            let messages = run.get("messages")?.as_array()?;
            let msg_val = messages.get(idx)?;
            let msg: AgentMessage = serde_json::from_value(msg_val.clone()).ok()?;
            Some(build_message_lines(state, &msg, theme, width))
        }
        "PATCH" => {
            let patches = run.get("patches")?.as_array()?;
            let patch_val = patches.get(idx)?;
            let patch: agent_ops_view::PersistedPatchRecord =
                serde_json::from_value(patch_val.clone()).ok()?;
            let diff_path = path
                .parent()
                .map(|p| p.join(&patch.id).to_string_lossy().to_string());
            Some(build_persisted_patch_lines(
                &patch,
                diff_path.as_deref(),
                theme,
                width,
            ))
        }
        "EVIDENCE" => {
            let evidence = run.get("evidence")?.as_array()?;
            let ev_val = evidence.get(idx)?;
            let item: nit_core::EvidenceItem = serde_json::from_value(ev_val.clone()).ok()?;
            Some(build_persisted_evidence_lines(&item, theme, width))
        }
        _ => None,
    }
}

pub fn build_lines(
    state: &AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
    width: u16,
) -> Vec<Line<'static>> {
    let width_usize = width.max(1) as usize;

    // If opened from the global archive, load the exact artifact directly from
    // the run.json using the stored entry, bypassing card-index matching.
    if let Some(entry) = &state.agents.global_archive_opened_entry {
        if let Some(lines) = build_lines_from_archive_entry(state, entry, theme, width_usize) {
            return lines;
        }
    }

    if let Some(reference) = agent_ops_view::artifacts_popup_ref(state, swarm, width_usize) {
        match reference {
            agent_ops_view::ArtifactsPopupRef::Message { idx } => {
                if let Some(message) = state.agents.messages.get(idx) {
                    return build_message_lines(state, message, theme, width_usize);
                }
            }
            agent_ops_view::ArtifactsPopupRef::SwarmTask {
                mission_id,
                task_id,
            } => {
                if let Some(view) = swarm.swarm_persistence(mission_id.as_str()) {
                    if let Some(task) = view.tasks.iter().find(|task| task.id == task_id) {
                        return build_swarm_task_lines(&view, task, theme, width_usize);
                    }
                }
            }
            agent_ops_view::ArtifactsPopupRef::SwarmReport { mission_id } => {
                if let Some(view) = swarm.swarm_persistence(mission_id.as_str()) {
                    return build_swarm_report_lines(&view, theme, width_usize);
                }
            }
            agent_ops_view::ArtifactsPopupRef::SwarmVerify { mission_id } => {
                if let Some(view) = swarm.swarm_persistence(mission_id.as_str()) {
                    return build_swarm_verify_lines(&view, theme, width_usize);
                }
            }
            agent_ops_view::ArtifactsPopupRef::PersistedMessage { message } => {
                return build_message_lines(state, &message, theme, width_usize);
            }
            agent_ops_view::ArtifactsPopupRef::PersistedPatch { patch, path } => {
                return build_persisted_patch_lines(&patch, path.as_deref(), theme, width_usize);
            }
            agent_ops_view::ArtifactsPopupRef::PersistedEvidence { item } => {
                return build_persisted_evidence_lines(&item, theme, width_usize);
            }
            agent_ops_view::ArtifactsPopupRef::Patch { idx } => {
                if let Some(patch) = state.agents.patches.get(idx) {
                    let persisted = agent_ops_view::PersistedPatchRecord {
                        id: patch.id.clone(),
                        mission_id: patch.mission_id.clone(),
                        agent_id: patch.agent_id.clone(),
                        title: patch.title.clone(),
                        summary: patch.summary.clone(),
                        diff: patch.diff.clone(),
                        status: patch.status.label().to_string(),
                    };
                    return build_persisted_patch_lines(&persisted, None, theme, width_usize);
                }
            }
            agent_ops_view::ArtifactsPopupRef::Evidence { idx } => {
                if let Some(item) = state.agents.evidence.get(idx) {
                    return build_persisted_evidence_lines(item, theme, width_usize);
                }
            }
        }
    }

    let strings = agent_ops_view::artifacts_popup_strings(state, swarm, width_usize);
    strings
        .into_iter()
        .enumerate()
        .map(|(idx, line)| style_line(idx, line, theme))
        .collect()
}

// Chat input auto-grows with input length, capped so the content area stays usable.
const POPUP_CHAT_INPUT_MAX_INNER_LINES: usize = 6;

pub fn chat_input_rect(state: &AppState, swarm: &SwarmRuntime, popup_area: Rect) -> Option<Rect> {
    let is_prompt =
        agent_ops_view::is_selected_artifact_prompt(state, swarm, popup_area.width.max(1) as usize);
    if is_prompt {
        return None;
    }
    let inner_full = Block::default().borders(Borders::ALL).inner(popup_area);
    if inner_full.width < 4 || inner_full.height < 5 {
        return None;
    }

    let input_wrap_width = inner_full.width.saturating_sub(2) as usize;
    let popup_input = &state.agents.artifacts_popup_chat_input;
    let cursor_char_idx = state
        .agents
        .artifacts_popup_chat_cursor
        .min(popup_input.chars().count());
    let (input_lines_all, _, _) = agent_console_view::wrap_input_with_cursor(
        "",
        popup_input,
        cursor_char_idx,
        input_wrap_width,
    );
    let half_inner = (inner_full.height as usize) / 2;
    let max_inner_by_layout = inner_full.height.saturating_sub(4).max(1) as usize;
    let dynamic_max = half_inner
        .max(POPUP_CHAT_INPUT_MAX_INNER_LINES)
        .min(max_inner_by_layout);
    let input_inner_height = input_lines_all.len().max(1).min(dynamic_max);
    let input_box_height = (input_inner_height + 2) as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(input_box_height)])
        .split(inner_full);
    let input_chunk = chunks[1];
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded);
    Some(input_block.inner(input_chunk))
}

pub fn map_chat_input_point_to_cursor(
    state: &AppState,
    swarm: &SwarmRuntime,
    popup_area: Rect,
    column: u16,
    row: u16,
    clamp: bool,
) -> Option<usize> {
    let input_area = chat_input_rect(state, swarm, popup_area)?;
    if !clamp
        && (column < input_area.x
            || column >= input_area.x.saturating_add(input_area.width)
            || row < input_area.y
            || row >= input_area.y.saturating_add(input_area.height))
    {
        return None;
    }

    let input_wrap_width = input_area.width as usize;
    if input_wrap_width == 0 {
        return Some(0);
    }

    let popup_input = &state.agents.artifacts_popup_chat_input;
    let cursor_char_idx = state
        .agents
        .artifacts_popup_chat_cursor
        .min(popup_input.chars().count());
    let (input_lines_all, cursor_line_all, _) = agent_console_view::wrap_input_with_cursor(
        "",
        popup_input,
        cursor_char_idx,
        input_wrap_width,
    );
    let input_inner_height = input_area.height as usize;
    let popup_scroll = state.agents.artifacts_popup_chat_scroll;
    let input_window_start = agent_console_view::chat_input_window_start(
        popup_scroll,
        input_lines_all.len(),
        input_inner_height,
        cursor_line_all,
    );

    let rel_row = if clamp {
        (row.saturating_sub(input_area.y) as usize).min(input_inner_height.saturating_sub(1))
    } else {
        row.saturating_sub(input_area.y) as usize
    };
    let rel_col = column.saturating_sub(input_area.x) as usize;
    let line_idx = input_window_start.saturating_add(rel_row);
    let max_col = input_lines_all
        .get(line_idx)
        .map(|l| unicode_width::UnicodeWidthStr::width(l.as_str()))
        .unwrap_or(0);
    let visual_col = rel_col.min(max_col);
    Some(agent_console_view::chat_input_char_index_for_display_pos(
        popup_input,
        input_wrap_width,
        line_idx,
        visual_col,
    ))
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
) -> Option<(u16, u16)> {
    let is_prompt =
        agent_ops_view::is_selected_artifact_prompt(state, swarm, area.width.max(1) as usize);
    let popup_title = if is_prompt { "PROMPT" } else { "ARTIFACT" };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused))
        .title(Span::styled(
            popup_title,
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(theme.background).fg(theme.foreground));

    let inner_full = block.inner(area);
    if inner_full.width < 4 || inner_full.height < 5 {
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);
        return None;
    }

    // For prompt artifacts, render without chat input box.
    if is_prompt {
        return render_content_only(frame, area, inner_full, block, state, swarm, theme);
    }

    // Compute input layout using popup-specific chat state. Clone `popup_input`
    // up-front so we don't hold an immutable borrow on `state` while we later
    // need to mutate `state.agents.artifacts_popup_scroll` / `last_max_scroll`.
    let input_wrap_width = inner_full.width.saturating_sub(2) as usize;
    let popup_input = state.agents.artifacts_popup_chat_input.clone();
    let popup_cursor = state.agents.artifacts_popup_chat_cursor;
    let popup_sel_anchor = state.agents.artifacts_popup_chat_selection_anchor;
    let popup_scroll = state.agents.artifacts_popup_chat_scroll;
    let cursor_char_idx = popup_cursor.min(popup_input.chars().count());
    let (input_lines_all, cursor_line_all, cursor_col_all) =
        agent_console_view::wrap_input_with_cursor(
            "",
            &popup_input,
            cursor_char_idx,
            input_wrap_width,
        );
    let half_inner = (inner_full.height as usize) / 2;
    let max_inner_by_layout = inner_full.height.saturating_sub(4).max(1) as usize;
    let dynamic_max = half_inner
        .max(POPUP_CHAT_INPUT_MAX_INNER_LINES)
        .min(max_inner_by_layout);
    let input_inner_height = input_lines_all.len().max(1).min(dynamic_max);
    let input_box_height = (input_inner_height + 2) as u16; // +2 for borders

    // Split: content area (scrollable) + input box (sticky footer).
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(input_box_height)])
        .split(inner_full);
    let content_area = chunks[0];
    let input_chunk = chunks[1];

    // --- Render content ---
    let lines = build_lines(state, swarm, theme, content_area.width);
    let content_height = content_area.height as usize;
    let max_scroll = lines.len().saturating_sub(content_height);
    let scroll = state.agents.artifacts_popup_scroll.min(max_scroll);
    // Cache max_scroll + clamped scroll back to state so wheel/keyboard input
    // handlers can scroll without calling the expensive `build_lines` path.
    state.agents.artifacts_popup_last_max_scroll = max_scroll;
    state.agents.artifacts_popup_scroll = scroll;
    let visible: Vec<Line> = lines
        .into_iter()
        .skip(scroll)
        .take(content_height)
        .collect();
    let visible = apply_ui_selection(
        visible,
        state.ui_selection.as_ref(),
        UiSelectionPane::ArtifactsPopup,
        theme.cursor_line_bg,
        scroll,
    );

    let para = Paragraph::new(visible).style(Style::default().bg(theme.background));
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(para, content_area);

    // --- Render chat input box ---
    let input_block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(vec![
            Span::styled(
                "CHAT BOX",
                Style::default()
                    .fg(theme.border_focused)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  [ Enter send | Ctrl+\u{2191}\u{2193} scroll ]",
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM),
            ),
        ]))
        .border_style(Style::default().fg(theme.border_focused))
        .border_type(ratatui::widgets::BorderType::Rounded)
        .style(Style::default().bg(theme.background));
    let input_area = input_block.inner(input_chunk);
    frame.render_widget(input_block, input_chunk);

    // Use the actual render-area width for display position calculations so
    // wrap boundaries stay in sync with the visual output.
    let render_wrap_width = input_area.width.max(1) as usize;

    let input_window_start = agent_console_view::chat_input_window_start(
        popup_scroll,
        input_lines_all.len(),
        input_inner_height,
        cursor_line_all,
    );

    let input_len_chars = popup_input.chars().count();
    let input_cursor = popup_cursor.min(input_len_chars);
    let selection_range = popup_sel_anchor
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
            let (start_line, start_col) = agent_console_view::chat_input_display_pos_for_char_idx(
                &popup_input,
                render_wrap_width,
                start,
            );
            let (end_line, end_col) = agent_console_view::chat_input_display_pos_for_char_idx(
                &popup_input,
                render_wrap_width,
                end,
            );
            (start_line, start_col, end_line, end_col)
        })
        .unwrap_or((0, 0, 0, 0));

    let input_visible: Vec<Line<'static>> = input_lines_all
        .iter()
        .skip(input_window_start)
        .take(input_inner_height)
        .enumerate()
        .map(|(idx, text)| {
            if selection_range.is_none() {
                return Line::from(text.clone());
            }
            let line_idx = input_window_start.saturating_add(idx);
            if line_idx < sel_start_line || line_idx > sel_end_line {
                return Line::from(text.clone());
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
            agent_console_view::highlight_plain_line(text, sel_start, sel_end, theme.selection_bg)
        })
        .collect();

    let input_bg = {
        let mut bg = agent_console_view::dim_bg_towards(theme.cursor_line_bg, theme.background, 75);
        if bg == theme.selection_bg {
            bg = theme.cursor_line_bg;
        }
        if bg == theme.selection_bg {
            bg = theme.background;
        }
        bg
    };
    frame.render_widget(
        Paragraph::new(input_visible).style(Style::default().fg(theme.foreground).bg(input_bg)),
        input_area,
    );

    // Cursor position. Gate on `cursor_visible` so the popup caret
    // blinks at the same cadence as the chat-pane caret instead of
    // sitting solid (which the operator perceived as a frozen UI).
    let cursor_in_window = cursor_line_all >= input_window_start
        && cursor_line_all < input_window_start.saturating_add(input_inner_height);
    if cursor_in_window
        && input_area.width > 0
        && input_area.height > 0
        && agent_console_view::cursor_visible(state)
    {
        let cursor_line_visible = cursor_line_all.saturating_sub(input_window_start);
        let max_col = input_area.width.saturating_sub(1) as usize;
        let col = cursor_col_all.min(max_col) as u16;
        let row = cursor_line_visible.min(input_inner_height.saturating_sub(1)) as u16;
        return Some((input_area.x + col, input_area.y + row));
    }
    None
}

// Prompt artifacts: render content without the chat input box.
fn render_content_only(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    block: Block<'_>,
    state: &mut AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
) -> Option<(u16, u16)> {
    let lines = build_lines(state, swarm, theme, inner.width);
    let height = inner.height as usize;
    let max_scroll = lines.len().saturating_sub(height);
    let scroll = state.agents.artifacts_popup_scroll.min(max_scroll);
    // Cache max_scroll + clamped scroll so scroll input handlers can skip
    // recomputing expensive `build_lines` on every wheel/keyboard event.
    state.agents.artifacts_popup_last_max_scroll = max_scroll;
    state.agents.artifacts_popup_scroll = scroll;
    let visible: Vec<Line> = lines.into_iter().skip(scroll).take(height).collect();
    let visible = apply_ui_selection(
        visible,
        state.ui_selection.as_ref(),
        UiSelectionPane::ArtifactsPopup,
        theme.cursor_line_bg,
        scroll,
    );

    let para = Paragraph::new(visible).style(Style::default().bg(theme.background));
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(para, inner);
    None
}

fn build_message_lines(
    _state: &AppState,
    message: &AgentMessage,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let kind = if message.agent_id.is_none() {
        "PROMPT"
    } else if message.kind.as_deref() == Some("synth") {
        "SYNTH"
    } else if message.kind.as_deref() == Some("plan") {
        "PLAN"
    } else {
        "REPLY"
    };
    let owner = message.agent_id.as_deref().unwrap_or("You");
    let at = if message.at.trim().is_empty() {
        "--"
    } else {
        message.at.as_str()
    };
    let rule = "─".repeat(width.min(240));

    let mut out = Vec::new();
    out.push(Line::from(Span::styled(
        format!(" {kind}  {owner}  {at}"),
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
    )));
    out.push(Line::from(Span::styled(
        rule.clone(),
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
    )));

    let label_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);
    out.push(kv_line(
        "at:",
        message.at.as_str(),
        label_style,
        value_style,
    ));
    out.push(kv_line(
        "mission:",
        message.mission_id.as_deref().unwrap_or("ad-hoc"),
        label_style,
        value_style,
    ));
    out.push(kv_line("agent:", owner, label_style, value_style));

    out.push(Line::from(Span::styled(
        rule,
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
    )));
    let body = message.text.trim();
    if body.is_empty() {
        let label_style = Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM);
        out.push(Line::from(Span::styled(" (no content)", label_style)));
    } else {
        out.extend(render_markdown_document(body, theme, width));
    }
    out
}

fn build_persisted_patch_lines(
    patch: &agent_ops_view::PersistedPatchRecord,
    path: Option<&str>,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let label_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);

    let mut out = Vec::new();
    out.push(popup_title_line(
        &format!(" PATCH  {}  {}", patch.agent_id, patch.status),
        theme,
    ));
    out.push(popup_rule_line(width, theme));
    out.push(kv_line("id:", &patch.id, label_style, value_style));
    out.push(kv_line("title:", &patch.title, label_style, value_style));
    if !patch.summary.is_empty() {
        out.push(kv_line(
            "summary:",
            &patch.summary,
            label_style,
            value_style,
        ));
    }
    out.push(kv_line(
        "mission:",
        patch.mission_id.as_deref().unwrap_or("ad-hoc"),
        label_style,
        value_style,
    ));
    out.push(popup_rule_line(width, theme));

    // Diff content.
    let diff_text = if !patch.diff.trim().is_empty() {
        patch.diff.clone()
    } else if let Some(p) = path {
        std::fs::read_to_string(p).unwrap_or_default()
    } else {
        String::new()
    };

    if diff_text.trim().is_empty() {
        out.push(Line::from(Span::styled(" (no diff content)", label_style)));
    } else {
        for line in diff_text.lines() {
            let style = diff_line_style(line, theme, value_style);
            out.push(Line::from(Span::styled(format!(" {line}"), style)));
        }
    }
    out
}

fn diff_line_style(line: &str, theme: &Theme, fallback: Style) -> Style {
    if line.starts_with('+') && !line.starts_with("+++") {
        Style::default().fg(theme.success)
    } else if line.starts_with('-') && !line.starts_with("---") {
        Style::default().fg(theme.error)
    } else if line.starts_with("@@") {
        Style::default().fg(theme.title_focused)
    } else {
        fallback
    }
}

fn build_persisted_evidence_lines(
    item: &nit_core::EvidenceItem,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let label_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(theme.foreground);

    let mut out = Vec::new();
    let owner = item.agent_id.as_deref().unwrap_or("system");
    out.push(popup_title_line(&format!(" EVIDENCE  {owner}"), theme));
    out.push(popup_rule_line(width, theme));
    out.push(kv_line("title:", &item.title, label_style, value_style));
    out.push(kv_line(
        "mission:",
        item.mission_id.as_deref().unwrap_or("ad-hoc"),
        label_style,
        value_style,
    ));
    if let Some(link) = item.link.as_deref() {
        if !link.is_empty() {
            out.push(kv_line("link:", link, label_style, value_style));
        }
    }
    out.push(popup_rule_line(width, theme));

    let body = item.detail.trim();
    if body.is_empty() {
        out.push(Line::from(Span::styled(" (no content)", label_style)));
    } else {
        for line in body.lines() {
            out.push(Line::from(Span::styled(format!(" {line}"), value_style)));
        }
    }
    out
}

pub(super) fn popup_title_line(text: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {text}"),
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
    ))
}

pub(super) fn popup_rule_line(width: usize, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width.min(240)),
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
    ))
}

pub(super) fn popup_section_line(text: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
    ))
}

pub(super) fn popup_note_line(text: &str, color: Color, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default()
            .fg(color)
            .bg(dim_bg_towards(theme.border, theme.background, 88)),
    ))
}

pub(super) fn push_wrapped_detail_lines(
    out: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    theme: &Theme,
    width: usize,
) {
    let label_text = format!(" {label}: ");
    let label_width = UnicodeWidthStr::width(label_text.as_str());
    let available = width.saturating_sub(label_width).max(8);
    let segments = wrap_visual_line(value, available);
    let label_style = Style::default()
        .fg(theme.border)
        .add_modifier(Modifier::DIM);
    let indent = " ".repeat(label_width);

    for (idx, segment) in segments.iter().enumerate() {
        let mut spans = Vec::new();
        if idx == 0 {
            spans.push(Span::styled(label_text.clone(), label_style));
        } else {
            spans.push(Span::styled(indent.clone(), label_style));
        }
        spans.extend(styled_text_spans(
            segment,
            Style::default().fg(theme.foreground),
            theme,
        ));
        out.push(Line::from(spans));
    }
}

pub(super) fn push_wrapped_bullet(out: &mut Vec<Line<'static>>, value: &str, theme: &Theme, width: usize) {
    let bullet = " • ";
    let bullet_width = UnicodeWidthStr::width(bullet);
    let available = width.saturating_sub(bullet_width).max(8);
    let segments = wrap_visual_line(value, available);
    let indent = " ".repeat(bullet_width);

    for (idx, segment) in segments.iter().enumerate() {
        let mut spans = Vec::new();
        if idx == 0 {
            spans.push(Span::styled(
                bullet.to_string(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(indent.clone(), Style::default()));
        }
        spans.extend(styled_text_spans(
            segment,
            Style::default().fg(theme.foreground),
            theme,
        ));
        out.push(Line::from(spans));
    }
}

pub(super) fn render_code_block_line(text: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    render_numbered_code_line(None, text, "", None, theme, width, 1)
}

pub(super) fn render_numbered_code_line(
    line_no: Option<usize>,
    text: &str,
    code_lang: &str,
    mapped: Option<&[MappedLineSegment]>,
    theme: &Theme,
    width: usize,
    gutter_width: usize,
) -> Vec<Line<'static>> {
    let prefix = match line_no {
        Some(line_no) => format!(" {line_no:>gutter_width$} │ "),
        None => format!(" {:>gutter_width$} │ ", ""),
    };
    let continuation = format!(" {:>gutter_width$} │ ", "");
    let available = width
        .saturating_sub(UnicodeWidthStr::width(prefix.as_str()))
        .max(8);
    let code_style = Style::default().fg(theme.foreground).bg(dim_bg_towards(
        theme.cursor_line_bg,
        theme.background,
        35,
    ));
    let rendered_segments = if let Some(mapped) = mapped {
        syntax_highlighted_wrapped_segments(text, mapped, code_style, theme, available)
    } else {
        wrap_visual_line(text, available)
            .into_iter()
            .map(|segment| styled_code_spans(segment.as_str(), code_lang, code_style, theme))
            .collect::<Vec<_>>()
    };
    rendered_segments
        .into_iter()
        .enumerate()
        .map(|(idx, spans_for_segment)| {
            let line_prefix = if idx == 0 {
                prefix.as_str()
            } else {
                continuation.as_str()
            };
            let mut spans = vec![Span::styled(
                line_prefix.to_string(),
                Style::default()
                    .fg(theme.border)
                    .bg(dim_bg_towards(theme.cursor_line_bg, theme.background, 35))
                    .add_modifier(Modifier::DIM),
            )];
            spans.extend(spans_for_segment);
            Line::from(spans)
        })
        .collect()
}

pub(super) fn styled_text_spans(text: &str, base: Style, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if let Some(rest) = remaining.strip_prefix('`') {
            if let Some(end) = rest.find('`') {
                let code = &rest[..end];
                spans.push(Span::styled(
                    format!("`{code}`"),
                    inline_code_style(code, theme).patch(base),
                ));
                remaining = &rest[end + 1..];
                continue;
            }
        }
        if let Some(rest) = remaining.strip_prefix('$') {
            if let Some(end) = rest.find('$') {
                let math = &rest[..end];
                if looks_like_inline_math(math) {
                    spans.push(Span::styled(
                        format!("${math}$"),
                        inline_math_style(theme).patch(base),
                    ));
                    remaining = &rest[end + 1..];
                    continue;
                }
            }
        }
        if let Some(rest) = remaining.strip_prefix("**") {
            if let Some(end) = rest.find("**") {
                let strong = &rest[..end];
                spans.push(Span::styled(
                    strong.to_string(),
                    base.fg(theme.hl.heading).add_modifier(Modifier::BOLD),
                ));
                remaining = &rest[end + 2..];
                continue;
            }
        }
        if let Some(rest) = remaining.strip_prefix('*') {
            if let Some(end) = rest.find('*') {
                let emphasis = &rest[..end];
                spans.push(Span::styled(
                    emphasis.to_string(),
                    base.fg(theme.hl.emphasis).add_modifier(Modifier::ITALIC),
                ));
                remaining = &rest[end + 1..];
                continue;
            }
        }

        let next_idx = remaining.find(['`', '*', '$']).unwrap_or(remaining.len());
        let plain = &remaining[..next_idx];
        push_plain_token_spans(&mut spans, plain, base, theme);
        remaining = &remaining[next_idx..];
        if next_idx == 0 {
            let ch = remaining.chars().next().unwrap_or_default();
            spans.push(Span::styled(ch.to_string(), base));
            remaining = &remaining[ch.len_utf8()..];
        }
    }

    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

pub(super) fn styled_code_spans(
    text: &str,
    code_lang: &str,
    base: Style,
    theme: &Theme,
) -> Vec<Span<'static>> {
    if is_json_code_lang(code_lang) {
        styled_json_spans(text, base, theme)
    } else {
        styled_text_spans(text, base, theme)
    }
}

pub(super) fn styled_json_spans(text: &str, base: Style, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let mut idx = 0usize;

    while idx < bytes.len() {
        let ch = text[idx..].chars().next().unwrap_or_default();
        match ch {
            '"' => {
                let Some((token, next_idx)) = json_string_token(text, idx) else {
                    spans.push(Span::styled(ch.to_string(), base));
                    idx += ch.len_utf8();
                    continue;
                };
                let style = if json_token_is_key(text, next_idx) {
                    base.fg(theme.hl.property).add_modifier(Modifier::BOLD)
                } else {
                    base.fg(theme.hl.string)
                };
                spans.push(Span::styled(token, style));
                idx = next_idx;
            }
            '{' | '}' | '[' | ']' | ':' | ',' => {
                spans.push(Span::styled(ch.to_string(), base.fg(theme.hl.punctuation)));
                idx += ch.len_utf8();
            }
            '-' | '0'..='9' => {
                let start = idx;
                idx += ch.len_utf8();
                while idx < bytes.len() {
                    let next = text[idx..].chars().next().unwrap_or_default();
                    if next.is_ascii_digit() || matches!(next, '.' | 'e' | 'E' | '+' | '-') {
                        idx += next.len_utf8();
                    } else {
                        break;
                    }
                }
                spans.push(Span::styled(
                    text[start..idx].to_string(),
                    base.fg(theme.hl.number).add_modifier(Modifier::BOLD),
                ));
            }
            't' if text[idx..].starts_with("true") => {
                spans.push(Span::styled(
                    "true".to_string(),
                    base.fg(theme.hl.boolean).add_modifier(Modifier::BOLD),
                ));
                idx += 4;
            }
            'f' if text[idx..].starts_with("false") => {
                spans.push(Span::styled(
                    "false".to_string(),
                    base.fg(theme.hl.boolean).add_modifier(Modifier::BOLD),
                ));
                idx += 5;
            }
            'n' if text[idx..].starts_with("null") => {
                spans.push(Span::styled("null".to_string(), base.fg(theme.hl.keyword)));
                idx += 4;
            }
            _ => {
                let start = idx;
                idx += ch.len_utf8();
                while idx < bytes.len() {
                    let next = text[idx..].chars().next().unwrap_or_default();
                    if matches!(
                        next,
                        '"' | '{' | '}' | '[' | ']' | ':' | ',' | '-' | '0'..='9'
                    ) {
                        break;
                    }
                    if text[idx..].starts_with("true")
                        || text[idx..].starts_with("false")
                        || text[idx..].starts_with("null")
                    {
                        break;
                    }
                    idx += next.len_utf8();
                }
                spans.push(Span::styled(text[start..idx].to_string(), base));
            }
        }
    }

    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

fn json_string_token(text: &str, start: usize) -> Option<(String, usize)> {
    let mut escaped = false;
    for (offset, ch) in text[start + 1..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            let end = start + 1 + offset + ch.len_utf8();
            return Some((text[start..end].to_string(), end));
        }
    }
    None
}

fn json_token_is_key(text: &str, idx: usize) -> bool {
    text[idx..].chars().find(|ch| !ch.is_whitespace()) == Some(':')
}

pub(super) fn styled_math_spans(text: &str, base: Style, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut idx = 0usize;

    while idx < text.len() {
        let ch = text[idx..].chars().next().unwrap_or_default();
        if ch.is_ascii_digit() {
            let start = idx;
            idx += ch.len_utf8();
            while idx < text.len() {
                let next = text[idx..].chars().next().unwrap_or_default();
                if next.is_ascii_digit() || matches!(next, '.' | ',' | '/') {
                    idx += next.len_utf8();
                } else {
                    break;
                }
            }
            spans.push(Span::styled(
                text[start..idx].to_string(),
                base.fg(theme.hl.number).add_modifier(Modifier::BOLD),
            ));
            continue;
        }
        if matches!(
            ch,
            '+' | '-' | '=' | '*' | '/' | '^' | '_' | '(' | ')' | '[' | ']'
        ) {
            spans.push(Span::styled(
                ch.to_string(),
                base.fg(theme.hl.operator).add_modifier(Modifier::BOLD),
            ));
            idx += ch.len_utf8();
            continue;
        }
        if ch == '\\' {
            let start = idx;
            idx += ch.len_utf8();
            while idx < text.len() {
                let next = text[idx..].chars().next().unwrap_or_default();
                if next.is_ascii_alphabetic() {
                    idx += next.len_utf8();
                } else {
                    break;
                }
            }
            spans.push(Span::styled(
                text[start..idx].to_string(),
                base.fg(theme.hl.keyword).add_modifier(Modifier::BOLD),
            ));
            continue;
        }
        spans.push(Span::styled(ch.to_string(), base));
        idx += ch.len_utf8();
    }

    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

fn push_plain_token_spans(spans: &mut Vec<Span<'static>>, text: &str, base: Style, theme: &Theme) {
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
                base.fg(theme.hl.link).add_modifier(Modifier::UNDERLINED)
            } else if looks_like_numberish(token) {
                base.fg(theme.accent).add_modifier(Modifier::BOLD)
            } else if looks_like_command(token) {
                base.fg(theme.hl.operator)
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
            spans.push(Span::styled(text[start..idx].to_string(), base));
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
        Style::default().fg(theme.foreground).bg(dim_bg_towards(
            theme.cursor_line_bg,
            theme.background,
            25,
        ))
    }
}

fn inline_math_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.accent)
        .bg(dim_bg_towards(theme.cursor_line_bg, theme.background, 22))
        .add_modifier(Modifier::ITALIC | Modifier::BOLD)
}

fn looks_like_inline_math(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.starts_with(' ') || text.ends_with(' ') {
        return false;
    }
    text.chars().any(|ch| {
        ch.is_ascii_digit()
            || matches!(
                ch,
                '=' | '+' | '-' | '*' | '/' | '^' | '_' | '\\' | '(' | ')' | '[' | ']'
            )
    })
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

pub(super) fn dim_bg_towards(color: Color, background: Color, background_pct: u8) -> Color {
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

pub(super) fn wrap_visual_line(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    let mut last_break: Option<(usize, usize)> = None;

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

fn kv_line(label: &str, value: &str, label_style: Style, value_style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {label} "), label_style),
        Span::styled(value.to_string(), value_style),
    ])
}

fn style_line(line_idx: usize, line: String, theme: &Theme) -> Line<'static> {
    if line.is_empty() {
        return Line::from("");
    }
    if line_idx == 0 {
        return Line::from(Span::styled(
            line,
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if line.chars().all(|ch| ch == '─') {
        return Line::from(Span::styled(
            line,
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ));
    }
    if matches!(line.as_str(), " Content" | " Diff (excerpt)") || line.starts_with(" Output") {
        return Line::from(Span::styled(
            line,
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ));
    }

    if let Some((label, value)) = line
        .trim_start_matches(' ')
        .split_once(' ')
        .filter(|(label, _)| label.ends_with(':'))
    {
        let label_text = format!(" {label} ");
        let value_trimmed = value.trim_start();
        let value_style =
            if value_trimmed.starts_with("http://") || value_trimmed.starts_with("https://") {
                Style::default()
                    .fg(theme.hl.link)
                    .add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default().fg(theme.foreground)
            };
        return Line::from(vec![
            Span::styled(
                label_text,
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM),
            ),
            Span::styled(value.to_string(), value_style),
        ]);
    }

    Line::from(Span::styled(line, Style::default().fg(theme.foreground)))
}

#[cfg(test)]
#[path = "tests/artifacts_popup.rs"]
mod tests;
