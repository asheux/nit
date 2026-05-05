use std::path::Path;

use nit_core::AppState;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};

use super::event_loop::popup_rect_for;
use crate::multipane::grid;
use crate::multipane::roster_view;
use crate::swarm::SwarmRuntime;
use crate::theme::Theme;
use crate::widgets::agent_console_view::{self, ChatCursor};

const MIN_PANE_WIDTH: u16 = 20;
const MIN_PANE_HEIGHT: u16 = 10;
const BOTTOM_HINT: &str = "MULTIPANE  ·  Tab cycle  ·  Ctrl+Q quit  ·  F1 help";

pub(super) fn pane_at(state: &AppState, pane_idx: usize) -> Option<&nit_core::PaneSession> {
    state.multipane.as_ref()?.panes.get(pane_idx)
}

pub(super) fn pane_at_mut(
    state: &mut AppState,
    pane_idx: usize,
) -> Option<&mut nit_core::PaneSession> {
    state.multipane.as_mut()?.panes.get_mut(pane_idx)
}

const DIR_SEARCH_DROPDOWN_MIN_ROWS: u16 = 3;
const DIR_SEARCH_DROPDOWN_MAX_ROWS: u16 = 16;
const DIR_SEARCH_INPUT_ROWS: u16 = 1;

pub(in crate::multipane) fn render_grid(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &mut AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
) -> Option<ChatCursor> {
    let (panes_len, focused_idx, cols, rows) = {
        let mp = state.multipane.as_ref()?;
        (mp.panes.len(), mp.focused, mp.grid_cols, mp.grid_rows)
    };
    if panes_len == 0 {
        return None;
    }

    // Reserve 1 row each for the top status strip and bottom indicator
    // when the terminal is tall enough; otherwise the panes get the
    // full area and chrome is suppressed.
    let chrome = area.height >= 4;
    let grid_area = if chrome {
        Rect::new(area.x, area.y + 1, area.width, area.height - 2)
    } else {
        area
    };
    if chrome {
        let top = Rect::new(area.x, area.y, area.width, 1);
        let bottom = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
        paint_top_strip(frame, top, state, theme);
        paint_bar(
            frame,
            bottom,
            BOTTOM_HINT.to_string(),
            bottom_strip_style(theme),
        );
    }

    let too_small = grid_too_small(grid_area, cols, rows);
    if too_small {
        let msg = format!(
            "Terminal too small for {panes_len} panes — resize or relaunch with --panes <smaller>"
        );
        paint_bar(frame, grid_area, msg, top_strip_style(theme));
        return None;
    }

    let mut cursor: Option<ChatCursor> = None;
    for idx in 0..panes_len {
        let focused = idx == focused_idx;
        if let Some(c) = render_one_pane(
            frame, grid_area, state, swarm, theme, idx, cols, rows, focused,
        ) {
            if focused {
                cursor = Some(c);
            }
        }
    }

    let help_open = matches!(state.multipane.as_ref(), Some(mp) if mp.help_open);
    if help_open {
        render_help_overlay(frame, popup_rect_for(area, (60, 18)), theme);
        // Hide the cursor while the overlay covers the input.
        return None;
    }
    cursor
}

pub(super) fn paint_bar(frame: &mut ratatui::Frame, rect: Rect, text: String, style: Style) {
    if rect.height == 0 || rect.width == 0 {
        return;
    }
    // `.style(style)` paints the entire rect with the bar's bg, so
    // cells beyond the text length still inherit the strip background.
    // Without it, `Span::styled` only colours the text cells and the
    // bar appears truncated on terminals wider than the label.
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text, style))).style(style),
        rect,
    );
}

fn top_strip_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.title_focused)
        .bg(theme.background)
        .add_modifier(Modifier::BOLD)
}

fn bottom_strip_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.border)
        .bg(theme.background)
        .add_modifier(Modifier::DIM)
}

fn grid_too_small(area: Rect, cols: usize, rows: usize) -> bool {
    if cols == 0 || rows == 0 {
        return false;
    }
    let pane_w = area.width / cols as u16;
    let pane_h = area.height / rows as u16;
    pane_w < MIN_PANE_WIDTH || pane_h < MIN_PANE_HEIGHT
}

fn paint_top_strip(frame: &mut ratatui::Frame, rect: Rect, state: &AppState, theme: &Theme) {
    let Some(mp) = state.multipane.as_ref() else {
        return;
    };
    let mut label = format!("MULTIPANE  pane {}/{}", mp.focused + 1, mp.panes.len());
    if let Some(status) = state.status.as_deref() {
        if !status.is_empty() {
            label.push_str("  STATUS:");
            label.push_str(status);
        }
    }
    paint_bar(frame, rect, label, bottom_strip_style(theme));
}

fn render_help_overlay(frame: &mut ratatui::Frame, rect: Rect, theme: &Theme) {
    let lines = [
        "Multipane keymap",
        "",
        "  Tab            cycle pane focus forward",
        "  Shift+Tab      cycle pane focus backward",
        "  Ctrl+R         revert focused pane to roster",
        "  Ctrl+/  / F2   toggle dir-search overlay",
        "  Enter          submit prompt / commit roster pick",
        "  Up / Down      walk per-pane chat history",
        "  PgUp / PgDn    scroll pane chat thread",
        "  Ctrl+C         abort focused pane (empty input)",
        "  Esc Esc        abort focused pane",
        "  Ctrl+Q         quit multipane",
        "  F1 / ?         toggle this overlay",
        "",
        "  In chat: @swarm @shadow @all @new @queue @q",
        "           /abort  /abort all  /abort <agent-id>",
    ];
    let body: Vec<Line<'static>> = lines
        .iter()
        .map(|l| {
            Line::from(Span::styled(
                (*l).to_string(),
                Style::default().fg(theme.foreground),
            ))
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_focused))
        .style(Style::default().bg(theme.background))
        .title(Span::styled(
            " HELP — multipane (F1 / ? / Esc to close) ",
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    frame.render_widget(ratatui::widgets::Clear, rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(body), inner);
}

#[allow(clippy::too_many_arguments)]
fn render_one_pane(
    frame: &mut ratatui::Frame,
    area: Rect,
    state: &mut AppState,
    swarm: &SwarmRuntime,
    theme: &Theme,
    idx: usize,
    cols: usize,
    rows: usize,
    focused: bool,
) -> Option<ChatCursor> {
    let rect = grid::pane_rect(area, cols, rows, idx);
    if rect.width < 2 || rect.height < 2 {
        return None;
    }
    let inner = paint_pane_chrome(frame, rect, state, idx, focused, theme);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    sync_dir_search_viewport(state, idx, inner.height);
    let pane = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(idx))
        .cloned()?;

    if pane.selected_agent_id.is_none() && pane.agent_id.is_empty() {
        let backend_filter = state
            .multipane
            .as_ref()
            .and_then(|mp| mp.backend_filter.clone());
        clamp_roster_scroll(state, idx, inner.height as usize);
        let updated_pane = state
            .multipane
            .as_ref()
            .and_then(|mp| mp.panes.get(idx))
            .cloned()
            .unwrap_or(pane);
        let body_rect = render_pane_dir_search_overlay(frame, inner, &updated_pane, theme);
        roster_view::render(
            frame,
            body_rect,
            state,
            &updated_pane,
            backend_filter.as_deref(),
            focused,
            theme,
        );
        return None;
    }

    let body_rect = render_pane_dir_search_overlay(frame, inner, &pane, theme);
    // Alias `state.agents.selected_*` to this pane's agent / mission
    // for the duration of the render so `breather_rows_for_user_prompt`
    // and `inline_breather_rows` (which read `selected_context_*`)
    // only show this pane's lanes. Restored before the next pane
    // renders to avoid bleed.
    let saved_agent = state.agents.selected_agent.clone();
    let saved_mission = state.agents.selected_mission.clone();
    let saved_mission_selected = state.agents.mission_selected;
    let pane_agent_id = if !pane.agent_id.is_empty() {
        Some(pane.agent_id.clone())
    } else {
        pane.selected_agent_id.clone()
    };
    state.agents.selected_agent = pane_agent_id;
    // For default-chat (no real swarm overlay), fall back to the
    // pane's synthetic chat id so `breather_rows_for_user_prompt`
    // sees a non-None `mission_ctx` and partitions other panes' agents
    // OUT of `primary_ids`. Mirrors the alias source in
    // `dispatch::with_pane_aliased`.
    state.agents.selected_mission = pane
        .mission_id
        .clone()
        .or_else(|| (!pane.chat_mission_id.is_empty()).then(|| pane.chat_mission_id.clone()));
    // Mirror `with_pane_aliased`: disable the global mission fallback
    // during this pane's render.
    state.agents.mission_selected = usize::MAX;
    let cursor = agent_console_view::render_pane(
        frame,
        body_rect,
        state,
        Some(swarm),
        theme,
        &pane,
        focused,
    );
    state.agents.selected_agent = saved_agent;
    state.agents.selected_mission = saved_mission;
    state.agents.mission_selected = saved_mission_selected;
    cursor
}

/// Stash the current visible-row count on `DirSearchState.last_visible`
/// (so key handlers can clamp the viewport without the layout rect)
/// and clamp `view_offset` to keep the highlight in view.
fn sync_dir_search_viewport(state: &mut AppState, idx: usize, inner_height: u16) {
    let Some(pane) = pane_at_mut(state, idx) else {
        return;
    };
    let Some(ds) = pane.dir_search.as_mut() else {
        return;
    };
    let visible = compute_dropdown_rows(inner_height, ds.results.len());
    ds.last_visible = visible;
    clamp_viewport(ds, visible as usize);
}

/// Walk the highlight back into the visible window after a results
/// refresh, a Down/Up key, or a viewport-height change. Idempotent.
pub(super) fn clamp_viewport(ds: &mut nit_core::DirSearchState, visible: usize) {
    let len = ds.results.len();
    if len == 0 {
        ds.view_offset = 0;
        ds.selected = 0;
        return;
    }
    if ds.selected >= len {
        ds.selected = len - 1;
    }
    let visible = visible.max(1);
    let max_offset = len.saturating_sub(visible);
    if ds.selected < ds.view_offset {
        ds.view_offset = ds.selected;
    }
    if ds.selected >= ds.view_offset + visible {
        ds.view_offset = ds.selected + 1 - visible;
    }
    if ds.view_offset > max_offset {
        ds.view_offset = max_offset;
    }
}

pub(super) fn compute_dropdown_rows(inner_height: u16, results_len: usize) -> u16 {
    let budget = inner_height.saturating_sub(DIR_SEARCH_INPUT_ROWS) / 2;
    let clamped = budget.clamp(DIR_SEARCH_DROPDOWN_MIN_ROWS, DIR_SEARCH_DROPDOWN_MAX_ROWS);
    clamped.min(results_len.max(1) as u16)
}

fn compute_dir_search_layout(
    inner: Rect,
    pane: &nit_core::PaneSession,
) -> Option<(u16, Rect, Rect)> {
    let ds = pane.dir_search.as_ref()?;
    if inner.height < DIR_SEARCH_INPUT_ROWS + DIR_SEARCH_DROPDOWN_MIN_ROWS {
        return None;
    }
    let visible_rows = compute_dropdown_rows(inner.height, ds.results.len());
    let bar_rect = Rect::new(inner.x, inner.y, inner.width, DIR_SEARCH_INPUT_ROWS);
    let drop_rect = Rect::new(
        inner.x,
        inner.y + DIR_SEARCH_INPUT_ROWS,
        inner.width,
        visible_rows,
    );
    Some((visible_rows, bar_rect, drop_rect))
}

/// Single source of truth for "where this pane's content paints with
/// the dir-search overlay open". Both the renderer and click hit-tests
/// must use this — otherwise roster clicks misroute when the dropdown
/// is open.
pub(super) fn dir_search_body_rect(inner: Rect, pane: &nit_core::PaneSession) -> Rect {
    let Some((visible_rows, _, _)) = compute_dir_search_layout(inner, pane) else {
        return inner;
    };
    let total = DIR_SEARCH_INPUT_ROWS + visible_rows;
    Rect::new(
        inner.x,
        inner.y + total,
        inner.width,
        inner.height.saturating_sub(total),
    )
}

fn render_pane_dir_search_overlay(
    frame: &mut ratatui::Frame,
    inner: Rect,
    pane: &nit_core::PaneSession,
    theme: &Theme,
) -> Rect {
    let Some((visible_rows, bar_rect, drop_rect)) = compute_dir_search_layout(inner, pane) else {
        return inner;
    };
    let ds = pane.dir_search.as_ref().unwrap();
    let bar_text = format!(
        " search: {}{} ",
        ds.query,
        if ds.show_hidden { "  [hidden]" } else { "" }
    );
    let bar_style = Style::default()
        .fg(theme.title_focused)
        .bg(theme.background)
        .add_modifier(Modifier::BOLD);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(bar_text, bar_style))),
        bar_rect,
    );
    let visible_usize = visible_rows as usize;
    let end = ds
        .view_offset
        .saturating_add(visible_usize)
        .min(ds.results.len());
    let slice_start = ds.view_offset.min(end);
    let lines: Vec<Line<'static>> = ds.results[slice_start..end]
        .iter()
        .enumerate()
        .map(|(rel_idx, path)| {
            let abs_idx = slice_start + rel_idx;
            let label = breadcrumb_label(&ds.base, path);
            let depth = path
                .strip_prefix(&ds.base)
                .ok()
                .map(|rel| rel.components().count().saturating_sub(1))
                .unwrap_or(0);
            let indent = "  ".repeat(depth);
            let style = if abs_idx == ds.selected {
                Style::default()
                    .fg(theme.background)
                    .bg(theme.border_focused)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground)
            };
            Line::from(Span::styled(format!(" {indent}{label} "), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), drop_rect);
    dir_search_body_rect(inner, pane)
}

/// Render a path as `parent/child/.../leaf/` relative to `base`. Uses
/// `Path::components()` joined with literal `/` so cross-platform
/// output stays stable; falls back to the absolute path display when
/// the entry isn't actually under base (e.g. symlink hop).
fn breadcrumb_label(base: &Path, path: &Path) -> String {
    let Ok(rel) = path.strip_prefix(base) else {
        return path.display().to_string();
    };
    let joined: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if joined.is_empty() {
        return path
            .file_name()
            .map(|n| format!("{}/", n.to_string_lossy()))
            .unwrap_or_else(|| path.display().to_string());
    }
    format!("{}/", joined.join("/"))
}

pub(super) fn clamp_roster_scroll(state: &mut AppState, pane_idx: usize, height: usize) {
    let backend_filter = state
        .multipane
        .as_ref()
        .and_then(|mp| mp.backend_filter.clone());
    let Some(pane_clone) = pane_at(state, pane_idx).cloned() else {
        return;
    };
    let rows = roster_view::compute_rows(state, &pane_clone, backend_filter.as_deref());
    let max_scroll = rows.len().saturating_sub(height);
    let stops = roster_view::selectable_count(&rows);
    let Some(pane) = pane_at_mut(state, pane_idx) else {
        return;
    };
    pane.roster_scroll = pane.roster_scroll.min(max_scroll);
    pane.roster_cursor = if stops == 0 {
        0
    } else {
        pane.roster_cursor.min(stops - 1)
    };
}

fn paint_pane_chrome(
    frame: &mut ratatui::Frame,
    rect: Rect,
    state: &AppState,
    idx: usize,
    focused: bool,
    theme: &Theme,
) -> Rect {
    let mp = match state.multipane.as_ref() {
        Some(mp) => mp,
        None => return rect,
    };
    let Some(pane) = mp.panes.get(idx) else {
        return rect;
    };
    let cwd_text = pane_path_label(state, &pane.cwd);
    let mode_label = if pane.selected_agent_id.is_none() && pane.agent_id.is_empty() {
        "roster"
    } else {
        "chat"
    };
    let title_prefix = format!(" pane {idx} · {mode_label} · ");
    let title_path = format!("{cwd_text} ");
    let border_color = if focused {
        theme.border_focused
    } else {
        theme.border
    };
    let title_color = if focused {
        theme.title_focused
    } else {
        theme.title
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(if focused {
            BorderType::Thick
        } else {
            BorderType::Plain
        })
        .border_style(Style::default().fg(border_color))
        .title(Line::from(vec![
            Span::styled(
                title_prefix,
                Style::default()
                    .fg(title_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                title_path,
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::DIM),
            ),
        ]))
        .style(Style::default().bg(theme.background));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    paint_hint_line(
        frame,
        inner,
        pane_in_roster_mode(state, idx),
        pane_dir_search_active(state, idx),
        theme,
    );
    inner_rect_after_hint(inner)
}

fn pane_dir_search_active(state: &AppState, idx: usize) -> bool {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(idx))
        .map(|p| p.dir_search.is_some())
        .unwrap_or(false)
}

fn pane_path_label(state: &AppState, cwd: &Path) -> String {
    let workspace_root = state.workspace_root.as_path();
    if let Ok(rel) = cwd.strip_prefix(workspace_root) {
        let rel_str = rel.to_string_lossy();
        let project = workspace_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        return match (project, rel_str.is_empty()) {
            (Some(name), true) => name,
            (Some(name), false) => format!("{name}/{rel_str}"),
            (None, false) => rel_str.into_owned(),
            (None, true) => cwd.display().to_string(),
        };
    }
    let Some(home) = std::env::var_os("HOME") else {
        return cwd.display().to_string();
    };
    let home_path = std::path::PathBuf::from(home);
    let Ok(rel) = cwd.strip_prefix(&home_path) else {
        return cwd.display().to_string();
    };
    let rel_str = rel.to_string_lossy();
    if rel_str.is_empty() {
        "~".into()
    } else {
        format!("~/{rel_str}")
    }
}

fn pane_in_roster_mode(state: &AppState, idx: usize) -> bool {
    state
        .multipane
        .as_ref()
        .and_then(|mp| mp.panes.get(idx))
        .map(|p| p.selected_agent_id.is_none() && p.agent_id.is_empty())
        .unwrap_or(false)
}

fn paint_hint_line(
    frame: &mut ratatui::Frame,
    inner: Rect,
    in_roster: bool,
    in_dir_search: bool,
    theme: &Theme,
) {
    if inner.height == 0 {
        return;
    }
    let hint_text = if in_dir_search {
        " ↑/↓ Ctrl+J/K · ←/→ Ctrl+H/L expand · Enter cd · Alt+F hidden · Esc close "
    } else if in_roster {
        " ↑/↓ j/k · h/l fold · Space check · Enter commit · Tab pane "
    } else {
        " /abort · Ctrl+C · Esc Esc · Ctrl+R roster · PgUp/PgDn scroll "
    };
    let hint = Line::from(Span::styled(
        hint_text,
        Style::default()
            .fg(theme.border)
            .add_modifier(Modifier::ITALIC | Modifier::DIM),
    ));
    let hint_rect = Rect::new(inner.x, inner.y, inner.width, 1);
    frame.render_widget(Paragraph::new(hint), hint_rect);
}

fn inner_rect_after_hint(inner: Rect) -> Rect {
    if inner.height <= 1 {
        return Rect::new(inner.x, inner.y, inner.width, 0);
    }
    Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1)
}

pub(super) fn pane_inner_after_chrome(rect: Rect) -> Rect {
    if rect.width < 2 || rect.height < 2 {
        return Rect::new(rect.x, rect.y, 0, 0);
    }
    let inner = Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2);
    inner_rect_after_hint(inner)
}

/// Single source of truth for "where this pane's content paints" —
/// chrome stripped + dir-search overlay stripped. Roster panes paint
/// rows directly into this rect; chat panes split it further into
/// thread + input via [`pane_thread_area_for_pane`].
pub(in crate::multipane) fn pane_body_rect(
    state: &AppState,
    area: Rect,
    pane_idx: usize,
    pane: &nit_core::PaneSession,
) -> Option<Rect> {
    let mp = state.multipane.as_ref()?;
    let pane_rect = grid::pane_rect(area, mp.grid_cols, mp.grid_rows, pane_idx);
    let inner = pane_inner_after_chrome(pane_rect);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let body = dir_search_body_rect(inner, pane);
    (body.width > 0 && body.height > 0).then_some(body)
}
