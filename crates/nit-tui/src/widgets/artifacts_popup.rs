use nit_core::{AgentMessage, AppState, GlobalArchiveEntry, UiSelectionPane};
use nit_syntax::{
    map_line_segments_to_chars, HighlightRequest, HighlightSnapshot, LanguageId, LanguageRegistry,
    MappedLineSegment, SyntaxConfig, SyntaxEngine, SyntaxManager,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use std::{
    collections::{hash_map::DefaultHasher, VecDeque},
    hash::{Hash, Hasher},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::swarm::{
    SwarmPersistenceView, SwarmRuntime, SwarmTaskArtifacts, SwarmTaskPersistenceView,
};
use crate::theme::Theme;
use crate::widgets::agent_console_view;
use crate::widgets::agent_ops_view;
use crate::widgets::text_selection::apply_ui_selection;

const DOCUMENT_HIGHLIGHT_WAIT: Duration = Duration::from_millis(250);
const DOCUMENT_HIGHLIGHT_CACHE_LIMIT: usize = 96;

struct DocumentSyntaxHighlighter {
    manager: SyntaxManager,
    recent_buffer_ids: VecDeque<usize>,
}

impl DocumentSyntaxHighlighter {
    fn new() -> Self {
        Self {
            manager: SyntaxManager::new(SyntaxConfig::default()),
            recent_buffer_ids: VecDeque::new(),
        }
    }

    fn highlight(&mut self, language: LanguageId, text: &str) -> Option<HighlightSnapshot> {
        let (buffer_id, version) = syntax_cache_key(language, text);
        if let Some(snapshot) = self.manager.try_get_highlights(buffer_id, version) {
            self.touch_buffer_id(buffer_id);
            return Some(snapshot);
        }

        self.touch_buffer_id(buffer_id);
        self.manager.schedule_rehighlight(HighlightRequest {
            buffer_id,
            version,
            language,
            text: text.to_string(),
            edits: Vec::new(),
            full_reparse: true,
            max_spans_per_line: self.manager.config().max_spans_per_line,
            viewport: None,
        });
        wait_for_document_snapshot(
            &mut self.manager,
            buffer_id,
            version,
            DOCUMENT_HIGHLIGHT_WAIT,
        )
    }

    fn touch_buffer_id(&mut self, buffer_id: usize) {
        if let Some(pos) = self
            .recent_buffer_ids
            .iter()
            .position(|seen| *seen == buffer_id)
        {
            self.recent_buffer_ids.remove(pos);
        }
        self.recent_buffer_ids.push_back(buffer_id);
        if self.recent_buffer_ids.len() > DOCUMENT_HIGHLIGHT_CACHE_LIMIT {
            self.manager = SyntaxManager::new(SyntaxConfig::default());
            self.recent_buffer_ids.clear();
            self.recent_buffer_ids.push_back(buffer_id);
        }
    }
}

fn document_syntax_highlighter() -> &'static Mutex<DocumentSyntaxHighlighter> {
    static DOCUMENT_SYNTAX_HIGHLIGHTER: OnceLock<Mutex<DocumentSyntaxHighlighter>> =
        OnceLock::new();
    DOCUMENT_SYNTAX_HIGHLIGHTER.get_or_init(|| Mutex::new(DocumentSyntaxHighlighter::new()))
}

fn syntax_cache_key(language: LanguageId, text: &str) -> (usize, u64) {
    let mut hasher = DefaultHasher::new();
    language.hash(&mut hasher);
    text.hash(&mut hasher);
    let hash = hasher.finish();
    (hash as usize, hash)
}

fn wait_for_document_snapshot(
    manager: &mut SyntaxManager,
    buffer_id: usize,
    version: u64,
    timeout: Duration,
) -> Option<HighlightSnapshot> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(snapshot) = manager.try_get_highlights(buffer_id, version) {
            return Some(snapshot);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

pub fn preferred_size(screen: Rect) -> (u16, u16) {
    let width = (screen.width.saturating_sub(4)).clamp(40, 140);
    let height = (screen.height.saturating_sub(4)).clamp(12, 44);
    (width, height)
}

/// Returns the content area height after deducting the chat input box from the popup.
/// Prompts do not have a chat input box, so they use the full height.
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

/// Load artifact content directly from a run.json using the archive entry's
/// `run_path` and `artifact_index`.  This avoids card-index mismatches that
/// caused the wrong artifact (e.g. a prompt instead of a reply) to be shown.
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

/// Maximum inner lines for the chat input box inside the artifacts popup.
/// The box auto-grows as the user types and shrinks back when text is removed,
/// capped at this limit so the content area stays usable.
const POPUP_CHAT_INPUT_MAX_INNER_LINES: usize = 6;

/// Returns the absolute screen rect of the chat input's inner area (excluding
/// its border), or `None` when the popup shows a prompt-only artifact.
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

/// Map a screen position inside the popup's chat input to a character index.
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
    state: &AppState,
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

    // Compute input layout using popup-specific chat state.
    let input_wrap_width = inner_full.width.saturating_sub(2) as usize;
    let popup_input = &state.agents.artifacts_popup_chat_input;
    let popup_cursor = state.agents.artifacts_popup_chat_cursor;
    let popup_sel_anchor = state.agents.artifacts_popup_chat_selection_anchor;
    let popup_scroll = state.agents.artifacts_popup_chat_scroll;
    let cursor_char_idx = popup_cursor.min(popup_input.chars().count());
    let (input_lines_all, cursor_line_all, cursor_col_all) =
        agent_console_view::wrap_input_with_cursor(
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
                popup_input,
                render_wrap_width,
                start,
            );
            let (end_line, end_col) = agent_console_view::chat_input_display_pos_for_char_idx(
                popup_input,
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

    // Cursor position.
    let cursor_in_window = cursor_line_all >= input_window_start
        && cursor_line_all < input_window_start.saturating_add(input_inner_height);
    if cursor_in_window && input_area.width > 0 && input_area.height > 0 {
        let cursor_line_visible = cursor_line_all.saturating_sub(input_window_start);
        let max_col = input_area.width.saturating_sub(1) as usize;
        let col = cursor_col_all.min(max_col) as u16;
        let row = cursor_line_visible.min(input_inner_height.saturating_sub(1)) as u16;
        return Some((input_area.x + col, input_area.y + row));
    }
    None
}

/// Render the popup with content only (no chat input box). Used for prompt artifacts.
fn render_content_only(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    block: Block<'_>,
    state: &AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
) -> Option<(u16, u16)> {
    let lines = build_lines(state, swarm, theme, inner.width);
    let height = inner.height as usize;
    let max_scroll = lines.len().saturating_sub(height);
    let scroll = state.agents.artifacts_popup_scroll.min(max_scroll);
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
            let style = if line.starts_with('+') && !line.starts_with("+++") {
                Style::default().fg(theme.success)
            } else if line.starts_with('-') && !line.starts_with("---") {
                Style::default().fg(theme.error)
            } else if line.starts_with("@@") {
                Style::default().fg(theme.title_focused)
            } else {
                value_style
            };
            out.push(Line::from(Span::styled(format!(" {line}"), style)));
        }
    }
    out
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

fn build_swarm_task_lines(
    view: &SwarmPersistenceView,
    task: &SwarmTaskPersistenceView,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    out.push(popup_title_line(
        &format!(" TASK  {}  {}", task.agent_id, task.state),
        theme,
    ));
    out.push(popup_rule_line(width, theme));
    push_wrapped_detail_lines(
        &mut out,
        "task",
        &format!("{}  {}", task.id, task.title),
        theme,
        width,
    );
    push_wrapped_detail_lines(&mut out, "mission", &view.mission_id, theme, width);
    if let Some(role) = task.role.as_deref().filter(|role| !role.trim().is_empty()) {
        push_wrapped_detail_lines(&mut out, "role", role, theme, width);
    }
    push_wrapped_detail_lines(
        &mut out,
        "writes",
        if task.writes { "yes" } else { "no" },
        theme,
        width,
    );
    if let Some(done_when) = task
        .done_when
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        push_wrapped_detail_lines(&mut out, "done_when", done_when, theme, width);
    }
    if !task.deps.is_empty() {
        push_wrapped_detail_lines(&mut out, "deps", &task.deps.join(", "), theme, width);
    }
    if !task.blocked_on.is_empty() {
        push_wrapped_detail_lines(
            &mut out,
            "blocked_on",
            &task.blocked_on.join(", "),
            theme,
            width,
        );
    }
    if !task.expected_artifacts.is_empty() {
        push_wrapped_detail_lines(
            &mut out,
            "expected",
            &task.expected_artifacts.join(", "),
            theme,
            width,
        );
    }
    if task.expected_artifacts_missing {
        out.push(popup_note_line(
            " expected artifacts but no parseable swarm_artifacts JSON block was captured",
            theme.warning,
            theme,
        ));
    }
    push_wrapped_detail_lines(
        &mut out,
        "artifact",
        &format!(
            ".nit/swarm/{}/tasks/{}/artifacts.json",
            view.mission_id, task.id
        ),
        theme,
        width,
    );
    if let Some(artifacts) = task.artifacts.as_ref() {
        push_task_artifact_sections(&mut out, artifacts, theme, width);
    }

    out.push(Line::from(""));
    out.push(popup_section_line(" Document", theme));
    push_wrapped_detail_lines(
        &mut out,
        "output",
        &format!(".nit/swarm/{}/tasks/{}/output.md", view.mission_id, task.id),
        theme,
        width,
    );
    if let Some(output) = task.output.as_deref() {
        out.push(Line::from(""));
        out.extend(render_markdown_document(output, theme, width));
    } else {
        out.push(popup_note_line(
            " no captured task output",
            theme.border,
            theme,
        ));
    }
    out
}

fn build_swarm_report_lines(
    view: &SwarmPersistenceView,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let status = view.report_status.as_deref().unwrap_or("FINAL");
    let agent_id = view.report_agent_id.as_deref().unwrap_or("planner");

    let mut out = Vec::new();
    out.push(popup_title_line(
        &format!(" REPORT  {agent_id}  {status}"),
        theme,
    ));
    out.push(popup_rule_line(width, theme));
    push_wrapped_detail_lines(&mut out, "mission", &view.mission_id, theme, width);
    push_wrapped_detail_lines(
        &mut out,
        "artifact",
        &format!(".nit/swarm/{}/report/final.md", view.mission_id),
        theme,
        width,
    );

    out.push(Line::from(""));
    out.push(popup_section_line(" Document", theme));
    if let Some(output) = view.report_output.as_deref() {
        out.push(Line::from(""));
        out.extend(render_markdown_document(output, theme, width));
    } else {
        out.push(popup_note_line(
            " no final synthesis output captured",
            theme.border,
            theme,
        ));
    }
    out
}

fn build_swarm_verify_lines(
    view: &SwarmPersistenceView,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let status = if let Some(report) = view.gate_report.as_ref() {
        if report.overall_ok {
            "PASS"
        } else {
            "FAIL"
        }
    } else if view.gate_bundle.is_some() {
        "PENDING"
    } else {
        "--"
    };

    let mut out = Vec::new();
    out.push(popup_title_line(
        &format!(
            " VERIFY  {}  {status}",
            view.gate_bundle.as_deref().unwrap_or("none")
        ),
        theme,
    ));
    out.push(popup_rule_line(width, theme));
    push_wrapped_detail_lines(&mut out, "mission", &view.mission_id, theme, width);
    push_wrapped_detail_lines(&mut out, "template", &view.template, theme, width);
    push_wrapped_detail_lines(
        &mut out,
        "artifact",
        &format!(".nit/swarm/{}/gates/verify.md", view.mission_id),
        theme,
        width,
    );
    if view.gate_report.is_some() {
        push_wrapped_detail_lines(
            &mut out,
            "report",
            &format!(".nit/swarm/{}/gates/report.json", view.mission_id),
            theme,
            width,
        );
    }
    if view.gate_output.is_some() {
        push_wrapped_detail_lines(
            &mut out,
            "output",
            &format!(".nit/swarm/{}/gates/output.txt", view.mission_id),
            theme,
            width,
        );
    }

    if let Some(report) = view.gate_report.as_ref() {
        out.push(Line::from(""));
        out.push(popup_section_line(" Gates", theme));
        for gate in report.gates.iter() {
            push_wrapped_bullet(
                &mut out,
                &format!(
                    "{} [{}] {}",
                    gate.name,
                    if gate.ok { "PASS" } else { "FAIL" },
                    gate.command
                ),
                theme,
                width,
            );
            if let Some(notes) = gate.notes.as_deref().filter(|text| !text.trim().is_empty()) {
                push_wrapped_detail_lines(&mut out, "notes", notes, theme, width);
            }
        }
    }

    out.push(Line::from(""));
    out.push(popup_section_line(" Document", theme));
    if let Some(output) = view.gate_output.as_deref() {
        out.push(Line::from(""));
        out.extend(render_markdown_document(output, theme, width));
    } else if view.gate_bundle.is_some() {
        out.push(popup_note_line(
            " verification has not completed yet",
            theme.warning,
            theme,
        ));
    } else {
        out.push(popup_note_line(
            " no verification output captured",
            theme.border,
            theme,
        ));
    }
    out
}

fn push_task_artifact_sections(
    out: &mut Vec<Line<'static>>,
    artifacts: &SwarmTaskArtifacts,
    theme: &Theme,
    width: usize,
) {
    if artifacts.summary.is_none()
        && artifacts.files.is_empty()
        && artifacts.diffs.is_empty()
        && artifacts.commands.is_empty()
        && artifacts.risks.is_empty()
        && artifacts.notes.is_empty()
    {
        return;
    }

    out.push(Line::from(""));
    out.push(popup_section_line(" Artifacts", theme));
    if let Some(summary) = artifacts
        .summary
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        push_wrapped_detail_lines(out, "summary", summary, theme, width);
    }
    for file in artifacts.files.iter() {
        let text = match file.notes.as_deref().filter(|text| !text.trim().is_empty()) {
            Some(notes) => format!("{} ({})", file.path, notes.trim()),
            None => file.path.clone(),
        };
        push_wrapped_bullet(out, &text, theme, width);
    }
    for diff in artifacts.diffs.iter() {
        let text = match diff.path.as_deref().filter(|text| !text.trim().is_empty()) {
            Some(path) => format!("{} ({})", diff.summary, path.trim()),
            None => diff.summary.clone(),
        };
        push_wrapped_bullet(out, &text, theme, width);
    }
    for command in artifacts.commands.iter() {
        let text = match command
            .purpose
            .as_deref()
            .filter(|text| !text.trim().is_empty())
        {
            Some(purpose) => format!("{} ({})", command.cmd, purpose.trim()),
            None => command.cmd.clone(),
        };
        push_wrapped_bullet(out, &text, theme, width);
    }
    for risk in artifacts.risks.iter() {
        let prefix = risk
            .level
            .as_deref()
            .map(str::trim)
            .filter(|level| !level.is_empty())
            .map(|level| format!("[{level}] "))
            .unwrap_or_default();
        let text = match risk
            .mitigation
            .as_deref()
            .filter(|text| !text.trim().is_empty())
        {
            Some(mitigation) => format!("{prefix}{} -> {}", risk.item, mitigation.trim()),
            None => format!("{prefix}{}", risk.item),
        };
        push_wrapped_bullet(out, &text, theme, width);
    }
    for note in artifacts.notes.iter() {
        push_wrapped_bullet(out, note, theme, width);
    }
}

fn popup_title_line(text: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {text}"),
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
    ))
}

fn popup_rule_line(width: usize, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width.min(240)),
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
    ))
}

fn popup_section_line(text: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
    ))
}

fn popup_note_line(text: &str, color: Color, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default()
            .fg(color)
            .bg(dim_bg_towards(theme.border, theme.background, 88)),
    ))
}

fn push_wrapped_detail_lines(
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

fn push_wrapped_bullet(out: &mut Vec<Line<'static>>, value: &str, theme: &Theme, width: usize) {
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

fn render_markdown_document(text: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    if let Some(json_lines) = maybe_render_json_document(text, theme, width) {
        return json_lines;
    }

    let mut out = Vec::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_lines: Vec<String> = Vec::new();
    let mut math_block_end: Option<&'static str> = None;
    let mut math_lines: Vec<String> = Vec::new();
    let mut table_lines: Vec<String> = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();

        if in_code_block {
            if parse_code_fence(trimmed).is_some() {
                out.extend(render_fenced_code_block(
                    code_lang.as_str(),
                    code_lines.as_slice(),
                    theme,
                    width,
                ));
                out.push(Line::from(""));
                in_code_block = false;
                code_lang.clear();
                code_lines.clear();
            } else {
                code_lines.push(line.to_string());
            }
            continue;
        }

        if let Some(end_marker) = math_block_end {
            if trimmed == end_marker {
                out.extend(render_math_block(math_lines.as_slice(), theme, width));
                out.push(Line::from(""));
                math_block_end = None;
                math_lines.clear();
            } else {
                math_lines.push(line.to_string());
            }
            continue;
        }

        if !in_code_block && is_markdown_table_candidate(trimmed) {
            table_lines.push(trimmed.to_string());
            continue;
        }
        if !table_lines.is_empty() {
            flush_markdown_table(&mut out, &mut table_lines, theme, width);
        }

        if let Some(lang) = parse_code_fence(trimmed) {
            in_code_block = true;
            code_lang = lang.to_string();
            code_lines.clear();
            continue;
        }

        if let Some(end_marker) = parse_math_block_start(trimmed) {
            math_block_end = Some(end_marker);
            math_lines.clear();
            continue;
        }
        if let Some(single_line_math) = extract_single_line_math_block(trimmed) {
            out.extend(render_math_block(&[single_line_math], theme, width));
            out.push(Line::from(""));
            continue;
        }

        if trimmed.is_empty() {
            out.push(Line::from(""));
            continue;
        }
        if is_thematic_rule(trimmed) {
            out.push(popup_rule_line(width, theme));
            continue;
        }
        if let Some((level, heading)) = parse_markdown_heading(trimmed) {
            out.extend(render_markdown_heading(level, heading, theme, width));
            continue;
        }
        if let Some(heading) = strong_only_heading_text(trimmed) {
            out.extend(render_markdown_heading(2, &heading, theme, width));
            continue;
        }
        if let Some(quote) = trimmed.strip_prefix('>').map(str::trim_start) {
            out.extend(render_markdown_quote(quote, theme, width));
            continue;
        }
        if let Some((marker, item)) = parse_list_marker(trimmed) {
            out.extend(render_markdown_list_item(marker, item, theme, width));
            continue;
        }
        out.extend(render_markdown_paragraph(trimmed, theme, width));
    }

    if in_code_block {
        out.extend(render_fenced_code_block(
            code_lang.as_str(),
            code_lines.as_slice(),
            theme,
            width,
        ));
    }
    if math_block_end.is_some() {
        out.extend(render_math_block(math_lines.as_slice(), theme, width));
    }
    if !table_lines.is_empty() {
        flush_markdown_table(&mut out, &mut table_lines, theme, width);
    }
    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

fn maybe_render_json_document(
    text: &str,
    theme: &Theme,
    width: usize,
) -> Option<Vec<Line<'static>>> {
    let trimmed = text.trim();
    if !matches!(trimmed.chars().next(), Some('{') | Some('[')) {
        return None;
    }
    let value = serde_json::from_str::<serde_json::Value>(trimmed).ok()?;
    let pretty = serde_json::to_string_pretty(&value).ok()?;

    let mut out = Vec::new();
    out.push(popup_note_line(" json document", theme.accent, theme));
    out.extend(render_fenced_code_block(
        "json",
        &pretty.lines().map(str::to_string).collect::<Vec<_>>(),
        theme,
        width,
    ));
    Some(out)
}

fn flush_markdown_table(
    out: &mut Vec<Line<'static>>,
    table_lines: &mut Vec<String>,
    theme: &Theme,
    width: usize,
) {
    if table_lines.is_empty() {
        return;
    }
    out.extend(render_markdown_table(table_lines.as_slice(), theme, width));
    table_lines.clear();
}

fn render_markdown_table(lines: &[String], theme: &Theme, width: usize) -> Vec<Line<'static>> {
    if lines.len() < 2 {
        return lines
            .iter()
            .flat_map(|line| render_code_block_line(line, theme, width))
            .collect();
    }

    let rows = lines
        .iter()
        .map(|line| split_markdown_table_cells(line))
        .collect::<Vec<_>>();
    if rows.len() < 2 || !is_markdown_table_separator(rows[1].as_slice()) {
        return lines
            .iter()
            .flat_map(|line| render_code_block_line(line, theme, width))
            .collect();
    }

    let headers = rows.first().cloned().unwrap_or_default();
    let body = rows.iter().skip(2).cloned().collect::<Vec<Vec<String>>>();
    let cols = headers
        .len()
        .max(body.iter().map(|row| row.len()).max().unwrap_or(0));
    if cols == 0 {
        return vec![Line::from("")];
    }

    let mut widths = vec![3usize; cols];
    for (idx, cell) in headers.iter().enumerate() {
        widths[idx] = widths[idx].max(UnicodeWidthStr::width(cell.as_str()));
    }
    for row in body.iter() {
        for (idx, cell) in row.iter().enumerate() {
            if idx < widths.len() {
                widths[idx] = widths[idx].max(UnicodeWidthStr::width(cell.as_str()));
            }
        }
    }

    let chrome = cols.saturating_mul(3).saturating_add(1);
    let available = width.saturating_sub(chrome).max(cols);
    let max_col = available / cols;
    for cell_width in widths.iter_mut() {
        *cell_width = (*cell_width).min(max_col.max(6));
    }
    // Give remaining space to the last column so it fills the full width.
    let used: usize = widths.iter().sum();
    if used < available && cols > 0 {
        widths[cols - 1] += available - used;
    }

    let border = format_table_border(widths.as_slice());
    let mut out = Vec::new();
    out.push(Line::from(Span::styled(
        border.clone(),
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
    )));
    out.push(table_row_line(
        headers.as_slice(),
        widths.as_slice(),
        Style::default()
            .fg(theme.title_focused)
            .add_modifier(Modifier::BOLD),
        theme,
    ));
    out.push(Line::from(Span::styled(
        border.clone(),
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
    )));
    for row in body.iter() {
        out.push(table_row_line(
            row.as_slice(),
            widths.as_slice(),
            Style::default().fg(theme.foreground),
            theme,
        ));
    }
    out.push(Line::from(Span::styled(
        border,
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
    )));
    out
}

fn table_row_line(
    cells: &[String],
    widths: &[usize],
    cell_style: Style,
    theme: &Theme,
) -> Line<'static> {
    let mut spans = vec![Span::styled(
        "|".to_string(),
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::DIM),
    )];
    for (idx, width) in widths.iter().enumerate() {
        let cell = cells.get(idx).cloned().unwrap_or_default();
        let cell = truncate_to_width(cell.as_str(), *width);
        let pad = width.saturating_sub(UnicodeWidthStr::width(cell.as_str()));
        spans.push(Span::styled(" ".to_string(), cell_style));
        spans.push(Span::styled(cell, cell_style));
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), cell_style));
        }
        spans.push(Span::styled(" ".to_string(), cell_style));
        spans.push(Span::styled(
            "|".to_string(),
            Style::default()
                .fg(theme.border)
                .add_modifier(Modifier::DIM),
        ));
    }
    Line::from(spans)
}

fn render_markdown_heading(
    level: usize,
    heading: &str,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let (prefix, style) = match level {
        1 => (
            " § ",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ),
        2 => (
            " • ",
            Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::BOLD),
        ),
        _ => (
            " · ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
    };
    let available = width.saturating_sub(UnicodeWidthStr::width(prefix)).max(8);
    wrap_visual_line(heading, available)
        .into_iter()
        .enumerate()
        .map(|(idx, segment)| {
            let leading = if idx == 0 {
                Span::styled(prefix.to_string(), style)
            } else {
                Span::styled(" ".repeat(UnicodeWidthStr::width(prefix)), style)
            };
            Line::from(vec![leading, Span::styled(segment, style)])
        })
        .collect()
}

fn render_markdown_quote(text: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let prefix = " │ ";
    let available = width.saturating_sub(UnicodeWidthStr::width(prefix)).max(8);
    let quote_style = Style::default()
        .fg(theme.border_focused)
        .add_modifier(Modifier::ITALIC);
    wrap_visual_line(text, available)
        .into_iter()
        .map(|segment| {
            let mut spans = vec![Span::styled(
                prefix.to_string(),
                Style::default().fg(theme.border),
            )];
            spans.extend(styled_text_spans(segment.as_str(), quote_style, theme));
            Line::from(spans)
        })
        .collect()
}

fn render_markdown_list_item(
    marker: &str,
    text: &str,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let prefix = format!(" {marker} ");
    let available = width
        .saturating_sub(UnicodeWidthStr::width(prefix.as_str()))
        .max(8);
    let segments = wrap_visual_line(text, available);
    let indent = " ".repeat(UnicodeWidthStr::width(prefix.as_str()));
    let mut out = Vec::new();
    for (idx, segment) in segments.iter().enumerate() {
        let mut spans = Vec::new();
        if idx == 0 {
            spans.push(Span::styled(
                prefix.clone(),
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
    out
}

fn render_markdown_paragraph(text: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let prefix = " ";
    let available = width.saturating_sub(UnicodeWidthStr::width(prefix)).max(8);
    wrap_visual_line(text, available)
        .into_iter()
        .map(|segment| {
            let mut spans = vec![Span::styled(prefix.to_string(), Style::default())];
            spans.extend(styled_text_spans(
                segment.as_str(),
                Style::default().fg(theme.foreground),
                theme,
            ));
            Line::from(spans)
        })
        .collect()
}

fn render_fenced_code_block(
    code_lang: &str,
    lines: &[String],
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let normalized = normalize_code_block_lines(lines, code_lang);
    let code_lang = canonical_code_lang(code_lang);
    let snapshot = highlight_code_block(code_lang.as_str(), normalized.as_slice());
    let label = if code_lang.is_empty() {
        " code block".to_string()
    } else {
        format!(" code block ({code_lang})")
    };
    let gutter_width = normalized.len().max(1).to_string().len();

    let mut out = Vec::new();
    out.push(popup_note_line(&label, theme.accent, theme));
    for (idx, line) in normalized.iter().enumerate() {
        let mapped = snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.per_line.get(idx))
            .and_then(|segments| map_line_segments_to_chars(line, segments).ok());
        out.extend(render_numbered_code_line(
            Some(idx + 1),
            line,
            code_lang.as_str(),
            mapped.as_deref(),
            theme,
            width,
            gutter_width,
        ));
    }
    if normalized.is_empty() {
        out.extend(render_numbered_code_line(
            None,
            "",
            code_lang.as_str(),
            None,
            theme,
            width,
            gutter_width,
        ));
    }
    out
}

fn render_code_block_line(text: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    render_numbered_code_line(None, text, "", None, theme, width, 1)
}

fn normalize_code_block_lines(lines: &[String], code_lang: &str) -> Vec<String> {
    let code_text = lines.join("\n");
    if is_json_code_lang(code_lang)
        || (code_lang.is_empty()
            && matches!(code_text.trim().chars().next(), Some('{') | Some('[')))
    {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(code_text.trim()) {
            if let Ok(pretty) = serde_json::to_string_pretty(&value) {
                return pretty.lines().map(str::to_string).collect();
            }
        }
    }
    lines.to_vec()
}

fn canonical_code_lang(code_lang: &str) -> String {
    match code_lang.trim().to_ascii_lowercase().as_str() {
        "js" => "javascript".into(),
        "ts" => "typescript".into(),
        "rs" => "rust".into(),
        "py" => "python".into(),
        other => other.to_string(),
    }
}

fn is_json_code_lang(code_lang: &str) -> bool {
    matches!(
        canonical_code_lang(code_lang).as_str(),
        "json" | "jsonc" | "geojson"
    )
}

fn highlight_code_block(code_lang: &str, lines: &[String]) -> Option<HighlightSnapshot> {
    let text = lines.join("\n");
    let language = language_id_for_code_block(code_lang, text.as_str())?;
    let mut highlighter = document_syntax_highlighter().lock().ok()?;
    highlighter.highlight(language, text.as_str())
}

fn language_id_for_code_block(code_lang: &str, text: &str) -> Option<LanguageId> {
    let normalized = canonical_code_lang(code_lang);
    if normalized.is_empty() {
        let trimmed = text.trim_start();
        if matches!(trimmed.chars().next(), Some('{') | Some('[')) {
            return Some(LanguageId::Json);
        }
        return None;
    }

    match normalized.as_str() {
        "jsonc" | "geojson" => Some(LanguageId::Json),
        "zsh" | "fish" | "shell" | "shell-session" | "console" => Some(LanguageId::Bash),
        "text" | "txt" | "plaintext" | "plain" => Some(LanguageId::PlainText),
        _ => LanguageRegistry::from_injection_name(normalized.as_str()),
    }
}

fn syntax_highlighted_wrapped_segments(
    text: &str,
    mapped: &[MappedLineSegment],
    base: Style,
    theme: &Theme,
    width: usize,
) -> Vec<Vec<Span<'static>>> {
    if text.is_empty() {
        return vec![vec![Span::styled(String::new(), base)]];
    }

    let chars: Vec<char> = text.chars().collect();
    let mut styles = vec![base; chars.len()];
    for seg in mapped {
        if seg.start >= seg.end || seg.start >= styles.len() {
            continue;
        }
        let style = base.patch(theme.highlight_style(seg.group));
        for idx in seg.start..seg.end.min(styles.len()) {
            styles[idx] = styles[idx].patch(style);
        }
    }

    let width = width.max(1);
    let mut out: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut buffers: Vec<String> = vec![String::new()];
    let mut current_styles = vec![styles[0]];
    let mut current_width = 0usize;
    let mut line_idx = 0usize;

    let flush_buffer = |out: &mut Vec<Vec<Span<'static>>>,
                        buffers: &mut Vec<String>,
                        current_styles: &[Style],
                        line_idx: usize| {
        if !buffers[line_idx].is_empty() {
            out[line_idx].push(Span::styled(
                std::mem::take(&mut buffers[line_idx]),
                current_styles[line_idx],
            ));
        }
    };

    let push_styled_char = |out: &mut Vec<Vec<Span<'static>>>,
                            buffers: &mut Vec<String>,
                            current_styles: &mut Vec<Style>,
                            line_idx: usize,
                            ch: char,
                            style: Style| {
        if style != current_styles[line_idx] && !buffers[line_idx].is_empty() {
            out[line_idx].push(Span::styled(
                std::mem::take(&mut buffers[line_idx]),
                current_styles[line_idx],
            ));
        }
        current_styles[line_idx] = style;
        buffers[line_idx].push(ch);
    };

    for (idx, ch) in chars.iter().enumerate() {
        let style = styles[idx];
        if *ch == '\t' {
            let tab_width = next_tab_width(current_width, width);
            for _ in 0..tab_width {
                if current_width + 1 > width && !buffers[line_idx].is_empty() {
                    flush_buffer(&mut out, &mut buffers, &current_styles, line_idx);
                    out.push(Vec::new());
                    buffers.push(String::new());
                    current_styles.push(style);
                    line_idx += 1;
                    current_width = 0;
                }
                push_styled_char(
                    &mut out,
                    &mut buffers,
                    &mut current_styles,
                    line_idx,
                    ' ',
                    style,
                );
                current_width += 1;
            }
            continue;
        }

        let ch_width = UnicodeWidthChar::width(*ch).unwrap_or(1).max(1);
        if current_width + ch_width > width && !buffers[line_idx].is_empty() {
            flush_buffer(&mut out, &mut buffers, &current_styles, line_idx);
            out.push(Vec::new());
            buffers.push(String::new());
            current_styles.push(style);
            line_idx += 1;
            current_width = 0;
        }
        push_styled_char(
            &mut out,
            &mut buffers,
            &mut current_styles,
            line_idx,
            *ch,
            style,
        );
        current_width += ch_width;
    }

    flush_buffer(&mut out, &mut buffers, &current_styles, line_idx);
    if out.is_empty() {
        vec![vec![Span::styled(String::new(), base)]]
    } else {
        out
    }
}

fn render_numbered_code_line(
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

fn render_math_block(lines: &[String], theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let prefix = " ∑ ";
    let continuation = "   ";
    let available = width.saturating_sub(UnicodeWidthStr::width(prefix)).max(8);
    let math_style = Style::default().fg(theme.accent).bg(dim_bg_towards(
        theme.cursor_line_bg,
        theme.background,
        40,
    ));

    let mut out = vec![popup_note_line(" equation", theme.accent, theme)];
    if lines.is_empty() {
        out.push(Line::from(vec![
            Span::styled(prefix.to_string(), math_style.add_modifier(Modifier::BOLD)),
            Span::styled(String::new(), math_style),
        ]));
        return out;
    }

    for (line_idx, line) in lines.iter().enumerate() {
        let wrapped = wrap_visual_line(line.trim(), available);
        for (segment_idx, segment) in wrapped.iter().enumerate() {
            let leading = if line_idx == 0 && segment_idx == 0 {
                prefix
            } else {
                continuation
            };
            let mut spans = vec![Span::styled(
                leading.to_string(),
                math_style.add_modifier(Modifier::BOLD),
            )];
            spans.extend(styled_math_spans(
                segment,
                math_style.add_modifier(Modifier::ITALIC),
                theme,
            ));
            out.push(Line::from(spans));
        }
    }
    out
}

fn parse_markdown_heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.chars().take_while(|ch| *ch == '#').count();
    if hashes == 0 {
        return None;
    }
    let text = line[hashes..].trim();
    (!text.is_empty()).then_some((hashes, text))
}

fn strong_only_heading_text(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with("**") || !trimmed.ends_with("**") || trimmed.len() < 4 {
        return None;
    }
    let inner = trimmed[2..trimmed.len() - 2].trim();
    if inner.is_empty() || inner.contains("**") {
        return None;
    }
    Some(inner.to_string())
}

fn parse_list_marker(line: &str) -> Option<(&str, &str)> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some((&marker[..1], rest.trim_start()));
        }
    }

    let digits = line.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digits > 0 && line[digits..].starts_with(". ") {
        let marker = &line[..digits + 1];
        let rest = line[digits + 2..].trim_start();
        if !rest.is_empty() {
            return Some((marker, rest));
        }
    }
    None
}

fn parse_code_fence(line: &str) -> Option<&str> {
    line.strip_prefix("```")
        .or_else(|| line.strip_prefix("~~~"))
        .map(str::trim)
}

fn parse_math_block_start(line: &str) -> Option<&'static str> {
    match line.trim() {
        "$$" => Some("$$"),
        "\\[" => Some("\\]"),
        _ => None,
    }
}

fn extract_single_line_math_block(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with("$$") && trimmed.ends_with("$$") && trimmed.len() > 4 {
        return Some(trimmed[2..trimmed.len() - 2].trim().to_string());
    }
    if trimmed.starts_with("\\[") && trimmed.ends_with("\\]") && trimmed.len() > 4 {
        return Some(trimmed[2..trimmed.len() - 2].trim().to_string());
    }
    None
}

fn is_thematic_rule(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= 3
        && (trimmed.chars().all(|ch| ch == '-')
            || trimmed.chars().all(|ch| ch == '*')
            || trimmed.chars().all(|ch| ch == '_'))
}

fn is_markdown_table_candidate(line: &str) -> bool {
    line.starts_with('|') && line.ends_with('|') && line.matches('|').count() >= 3
}

fn split_markdown_table_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn is_markdown_table_separator(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let trimmed = cell.trim();
            !trimmed.is_empty() && trimmed.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
        })
}

fn format_table_border(widths: &[usize]) -> String {
    let mut line = String::from("+");
    for width in widths {
        line.push_str(&"-".repeat(width.saturating_add(2)));
        line.push('+');
    }
    line
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        if used.saturating_add(ch_width).saturating_add(1) > width {
            break;
        }
        out.push(ch);
        used = used.saturating_add(ch_width);
    }
    out.push('…');
    out
}

fn styled_text_spans(text: &str, base: Style, theme: &Theme) -> Vec<Span<'static>> {
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

fn styled_code_spans(
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

fn styled_json_spans(text: &str, base: Style, theme: &Theme) -> Vec<Span<'static>> {
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

fn styled_math_spans(text: &str, base: Style, theme: &Theme) -> Vec<Span<'static>> {
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

fn wrap_visual_line(text: &str, width: usize) -> Vec<String> {
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

fn next_tab_width(col: usize, width: usize) -> usize {
    let width = width.max(1);
    let to_stop = 4usize.saturating_sub(col % 4);
    to_stop.max(1).min(width)
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
