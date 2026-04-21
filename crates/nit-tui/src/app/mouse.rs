#![allow(unused_imports)]
#![allow(clippy::too_many_arguments)]
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::{
    mpsc::{self, Receiver, Sender},
    Arc, Mutex, Weak,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::swarm::{
    chat_clone_base_id, normalize_role_label, GateReport, GateReportGate, SwarmArtifactFocus,
    SwarmRuntime,
};
use crate::{
    claude_runner::{ClaudeRunner, ClaudeRunnerConfig},
    codex_runner::{CodexCommand, CodexRunner, CodexRunnerConfig, CodexRuntimeMode},
    file_tree,
    file_tree_runner::{FileTreeCommand, FileTreeEvent, FileTreeRunner},
    file_watcher::FileWatcher,
    fuzzy_preview_runner::{PreviewEvent, PreviewModel, PreviewRunner},
    fuzzy_search_runner::{
        ContentEvent, ContentSearchRunner, FileIndexRunner, FuzzyCommand, FuzzyEvent,
        FuzzyMatcherRunner, IndexEvent,
    },
    games_petri_dish::GamesPetriDishRuntime,
    layout,
    petri_dish::PetriDishRuntime,
    seed_runtime::SeedRuntime,
    syntax::SyntaxRuntime,
    system_stats::SystemStats,
    theme::Theme,
    vitals::{AgentVitalsState, DiagSeverity, LabVitalsSnapshot, VitalsState},
    widgets::{
        agent_console_view, agent_ops_view, artifacts_history_popup, artifacts_popup, bottom_bar,
        editor_view, file_tree_view, fuzzy_search_popup, games_analysis_popup, games_ca_sim_popup,
        games_match_history_popup, games_replay_popup, games_run_browser_popup,
        games_strategy_popup, games_tm_sim_popup, games_visualizer_view, gate_monitor_view,
        help_overlay, protocol_picker, rule_picker, substrate_overlay, top_bar, visualizer_view,
    },
};
use arboard::Clipboard;
use crossterm::{
    cursor::{SetCursorStyle, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseEvent,
        MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ctrlc::Error as CtrlcError;
use nit_core::{
    actions::Action, apply_action, io as core_io, AgentAlert, AgentAlertSeverity, AgentBusEvent,
    AgentChannel, AgentDiagnosticEvent, AgentMessage, AgentOpsTab, AgentStatus, AppKind, AppState,
    McpConnectionState, MissionPhase, MissionRecord, Mode, PaneId, PatchProposal, PatchStatus,
    Prompt, SavedRunHistoryFilter, SearchMode, UiSelection, UiSelectionPane, YankKind,
    CONSOLE_SCROLL_BOTTOM,
};
use nit_games::config::GamesConfig;
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::Style,
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Terminal,
};

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_mouse_event_with_swarm(
    swarm: &SwarmRuntime,
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    fuzzy_runtime: &mut FuzzySearchRuntime,
    input_state: &mut InputState,
    clipboard: &mut Option<Clipboard>,
    theme: &Theme,
) -> bool {
    match mouse.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let fast = mouse
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL);
            let step = if fast {
                SCROLL_LINES_FAST
            } else {
                SCROLL_LINES
            };
            let delta = if matches!(mouse.kind, MouseEventKind::ScrollUp) {
                -(step as i32)
            } else {
                step as i32
            };
            if state.command_line.is_some() || state.prompt.is_some() {
                return true;
            }

            if state.rule_picker.open || state.protocol_picker.open {
                return true;
            }

            if state.fuzzy_search.open {
                use ratatui::layout::{Constraint, Direction, Layout, Rect};
                let area = dynamic_popup_rect(screen, fuzzy_popup_size(screen, state));
                let list_height = area
                    .height
                    .saturating_sub(6) // outer(2) + header/footer(2) + results block(2)
                    .max(1) as usize;
                let mut over_preview = false;
                if point_in_rect(mouse.column, mouse.row, area) {
                    let inner = Rect {
                        x: area.x.saturating_add(1),
                        y: area.y.saturating_add(1),
                        width: area.width.saturating_sub(2),
                        height: area.height.saturating_sub(2),
                    };
                    let body = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(1),
                            Constraint::Min(1),
                            Constraint::Length(1),
                        ])
                        .split(inner)[1];
                    let halves = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(body);
                    over_preview = point_in_rect(mouse.column, mouse.row, halves[1]);
                }
                if over_preview {
                    fuzzy_runtime.preview_scroll_delta =
                        fuzzy_runtime.preview_scroll_delta.saturating_add(delta);
                } else {
                    let len = fuzzy_results_len(state);
                    if len > 0 {
                        if delta.is_negative() {
                            state.fuzzy_search.selected = state
                                .fuzzy_search
                                .selected
                                .saturating_sub(delta.unsigned_abs() as usize);
                        } else {
                            state.fuzzy_search.selected =
                                (state.fuzzy_search.selected + delta as usize).min(len - 1);
                        }
                        adjust_fuzzy_scroll(state, list_height);
                        fuzzy_runtime.request_preview_for_selection(state);
                    }
                }
                // Modal: don't scroll underlying panes while open.
                return true;
            }

            if state.agents.artifacts_popup_open {
                let area = dynamic_popup_rect(screen, artifacts_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    // Use the cached max_scroll from the last render — rebuilding
                    // the rendered markdown just to clamp a wheel tick was making
                    // scroll feel sluggish on large artifacts. Render re-clamps.
                    //
                    // If the cache is still `usize::MAX` (popup just opened and
                    // no render has run yet), fall back to computing metrics
                    // inline ONCE so the forward clamp at max actually holds.
                    // Without this, a burst of wheel-down events before the
                    // first render could over-inflate `artifacts_popup_scroll`
                    // past the real max, and subsequent reverse scrolls would
                    // appear stuck until the inflation unwound.
                    let mut max_scroll = state.agents.artifacts_popup_last_max_scroll;
                    if max_scroll == usize::MAX {
                        let (computed, _) =
                            artifacts_popup_scroll_metrics(state, swarm, screen, theme);
                        max_scroll = computed;
                        state.agents.artifacts_popup_last_max_scroll = computed;
                    }
                    bump_scroll_clamped(
                        &mut state.agents.artifacts_popup_scroll,
                        delta,
                        max_scroll,
                    );
                }
                return true;
            }

            if state.agents.global_archive_open {
                let area =
                    dynamic_popup_rect(screen, artifacts_history_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let (max_scroll, _) = global_archive_scroll_metrics(state, screen, theme);
                    bump_scroll_clamped(&mut state.agents.global_archive_scroll, delta, max_scroll);
                }
                return true;
            }

            if state.show_help {
                let area = dynamic_popup_rect(screen, help_overlay::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max_scroll = help_popup_max_scroll(screen, theme);
                    bump_scroll_clamped(&mut state.help_scroll, delta, max_scroll);
                }
                return true;
            }

            if state.show_substrate_overlay {
                let area = substrate_overlay::preferred_size(screen, state.substrate_overlay_tab);
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max_scroll = state.substrate_overlay_last_max_scroll;
                    let max_scroll = if max_scroll == usize::MAX {
                        usize::MAX
                    } else {
                        max_scroll
                    };
                    bump_scroll_clamped(&mut state.substrate_overlay_scroll, delta, max_scroll);
                }
                return true;
            }

            if state.app_kind == AppKind::Games && state.games.analysis.open {
                let area = dynamic_popup_rect(screen, games_analysis_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max_scroll = games_analysis_popup_max_scroll(state, screen, theme);
                    bump_scroll_clamped(&mut state.games.analysis.scroll_offset, delta, max_scroll);
                }
                return true;
            }

            if state.app_kind == AppKind::Games && state.games.run_browser.open {
                let area =
                    dynamic_popup_rect(screen, games_run_browser_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max_scroll = games_run_browser_popup_max_scroll(state, screen, theme);
                    bump_scroll_clamped(
                        &mut state.games.run_browser.scroll_offset,
                        delta,
                        max_scroll,
                    );
                }
                return true;
            }

            if state.app_kind == AppKind::Games && state.games.replay.open {
                let area = dynamic_popup_rect(screen, games_replay_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max_scroll = games_replay_popup_max_scroll(state, screen, theme);
                    bump_scroll_clamped(&mut state.games.replay.scroll_offset, delta, max_scroll);
                }
                return true;
            }

            if state.app_kind == AppKind::Games && state.games.strategy_inspect.open {
                let area = dynamic_popup_rect(screen, games_strategy_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max_scroll = games_strategy_popup_max_scroll(state, screen);
                    bump_scroll_clamped(
                        &mut state.games.strategy_inspect.scroll_offset,
                        delta,
                        max_scroll,
                    );
                }
                return true;
            }
            if state.app_kind == AppKind::Games && state.games.tm_sim.open {
                let area = dynamic_popup_rect(screen, games_tm_sim_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max_scroll = games_tm_sim_popup_max_scroll(state, screen, theme);
                    bump_scroll_clamped(&mut state.games.tm_sim.scroll_offset, delta, max_scroll);
                }
                return true;
            }
            if state.app_kind == AppKind::Games && state.games.ca_sim.open {
                let area = dynamic_popup_rect(screen, games_ca_sim_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max_scroll = games_ca_sim_popup_max_scroll(state, screen, theme);
                    bump_scroll_clamped(&mut state.games.ca_sim.scroll_offset, delta, max_scroll);
                }
                return true;
            }
            if state.app_kind == AppKind::Games && state.games.match_history.open {
                let area =
                    dynamic_popup_rect(screen, games_match_history_popup::preferred_size(screen));
                if point_in_rect(mouse.column, mouse.row, area) {
                    let max = games_match_history_max_offset(state, screen);
                    if delta > 0 {
                        state.games.match_history.column_offset =
                            state.games.match_history.column_offset.saturating_sub(1);
                    } else if games_match_history_total_entries(state) > 0 {
                        state.games.match_history.column_offset =
                            (state.games.match_history.column_offset + 1).min(max);
                    }
                }
                return true;
            }

            if games_petri_visible(state) {
                return true;
            }

            let layout = layout::split(screen);
            if point_in_rect(mouse.column, mouse.row, layout.editor) {
                scroll_buffer(state.editor_buffer_mut(), delta);
                return true;
            }
            if point_in_rect(mouse.column, mouse.row, layout.notes) {
                if let Some(metrics) =
                    agent_console_view::chat_input_scroll_metrics(layout.notes, state)
                {
                    if point_in_rect(mouse.column, mouse.row, metrics.area) {
                        let mut start = metrics.window_start;
                        bump_scroll(&mut start, delta);
                        state.agents.chat_input_scroll = start.min(metrics.max_scroll);
                        return true;
                    }
                }
                if let Some(thread_area) = agent_console_view::thread_text_area(layout.notes, state)
                {
                    let lines = agent_console_view::thread_lines_for_selection_with_swarm(
                        state,
                        swarm,
                        thread_area.width.max(1) as usize,
                    );
                    let max_scroll = lines
                        .len()
                        .saturating_sub(thread_area.height.max(1) as usize);
                    let mut scroll = state.agents.console_scroll.min(max_scroll);
                    bump_scroll(&mut scroll, delta);
                    state.agents.console_scroll = scroll.min(max_scroll);
                } else {
                    bump_scroll_clamped(
                        &mut state.agents.console_scroll,
                        delta,
                        state.agents.console_max_scroll,
                    );
                }
                return true;
            }
            if point_in_rect(mouse.column, mouse.row, layout.gate) {
                // Prefer cached max_scroll populated during render. Fall back
                // to rebuilding the full genome report only when no render
                // has run yet since the panel became visible.
                let mut max_scroll = state.gate_monitor_last_max_scroll;
                if max_scroll == usize::MAX {
                    let inner = Block::default().borders(Borders::ALL).inner(layout.gate);
                    let lines =
                        gate_monitor_view::build_lines(state, None, theme, inner.width as usize);
                    max_scroll = lines.len().saturating_sub(inner.height as usize);
                    state.gate_monitor_last_max_scroll = max_scroll;
                }
                bump_scroll_clamped(&mut state.gate_monitor_scroll, delta, max_scroll);
                return true;
            }
            if point_in_rect(mouse.column, mouse.row, layout.job) {
                if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                    scroll_buffer(state.notes_buffer_mut(), delta);
                } else {
                    let text_area = job_output_text_area(layout.job);
                    let text_width = text_area.width as usize;
                    let lines = agent_ops_view::current_lines_for_width_with_swarm(
                        state,
                        Some(swarm),
                        text_width,
                    );
                    let height = text_area.height as usize;
                    let max_scroll = lines.len().saturating_sub(height);
                    let mut scroll = state.agents.ops_scroll;
                    bump_scroll(&mut scroll, delta);
                    state.agents.ops_scroll = scroll.min(max_scroll);
                }
                return true;
            }
            false
        }
        MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
            if state.fuzzy_search.open {
                handle_fuzzy_search_mouse_down(mouse, screen, state, fuzzy_runtime)
            } else {
                handle_mouse_down_with_swarm(
                    swarm,
                    mouse,
                    screen,
                    state,
                    input_state,
                    clipboard,
                    theme,
                )
            }
        }
        MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
            handle_mouse_drag_with_swarm(swarm, mouse, screen, state, input_state, clipboard, theme)
        }
        MouseEventKind::Up(crossterm::event::MouseButton::Left) => {
            maybe_open_artifacts_popup_url_on_click(
                swarm,
                mouse,
                screen,
                state,
                input_state,
                theme,
            );
            input_state.mouse_select_anchor = None;
            true
        }
        _ => false,
    }
}

pub(super) const SCROLL_LINES: usize = 3;

pub(super) const SCROLL_LINES_FAST: usize = 10;

pub(super) fn handle_fuzzy_search_mouse_down(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    runtime: &mut FuzzySearchRuntime,
) -> bool {
    use ratatui::layout::{Constraint, Direction, Layout, Rect};

    let area = dynamic_popup_rect(screen, fuzzy_popup_size(screen, state));
    // Modal: ignore clicks outside the popup (and prevent underlying panes from receiving them).
    if !point_in_rect(mouse.column, mouse.row, area) {
        return true;
    }

    let list_height = area
        .height
        .saturating_sub(6) // outer(2) + header/footer(2) + results block(2)
        .max(1) as usize;

    let inner = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner)[1];
    let halves = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(body);

    // RESULTS block has its own border.
    let results_inner = Rect {
        x: halves[0].x.saturating_add(1),
        y: halves[0].y.saturating_add(1),
        width: halves[0].width.saturating_sub(2),
        height: halves[0].height.saturating_sub(2),
    };
    if point_in_rect(mouse.column, mouse.row, results_inner) {
        let idx_in_view = mouse.row.saturating_sub(results_inner.y) as usize;
        let target = state.fuzzy_search.scroll_offset.saturating_add(idx_in_view);
        let len = fuzzy_results_len(state);
        if len > 0 && target < len {
            state.fuzzy_search.selected = target;
            adjust_fuzzy_scroll(state, list_height);
            runtime.request_preview_for_selection(state);
        }
    }

    true
}

pub(super) fn maybe_open_artifacts_popup_url_on_click(
    swarm: &SwarmRuntime,
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    input_state: &InputState,
    theme: &Theme,
) {
    if !state.agents.artifacts_popup_open {
        return;
    }
    let Some(anchor) = input_state.mouse_select_anchor else {
        return;
    };
    if !matches!(
        anchor.target,
        MouseSelectTarget::Ui(UiSelectionPane::ArtifactsPopup)
    ) {
        return;
    }

    let Some(selection) = state.ui_selection else {
        return;
    };
    if selection.pane != UiSelectionPane::ArtifactsPopup {
        return;
    }
    if selection.start_line != selection.end_line || selection.start_col != selection.end_col {
        // Drag selection: never open.
        return;
    }
    if selection.start_line != anchor.line || selection.start_col != anchor.col {
        return;
    }

    let Some((line_idx, col, lines)) =
        map_artifacts_popup_mouse_with_swarm(swarm, mouse, screen, state, theme, false)
    else {
        return;
    };
    if line_idx != anchor.line || col != anchor.col {
        return;
    }

    let Some(url) = http_url_at_line_col(&lines, line_idx, col) else {
        return;
    };
    match open_url_in_browser(&url) {
        Ok(()) => {
            state.status = Some(format!("Opened {url}"));
        }
        Err(err) => {
            state.status = Some(format!("Open URL failed: {err}"));
        }
    }
}

pub(super) fn open_url_in_browser(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("empty url".into());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("url must start with http:// or https://".into());
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = url;
        Err("unsupported platform".into())
    }
}

pub(super) fn http_url_at_line_col(
    lines: &[String],
    line_idx: usize,
    col: usize,
) -> Option<String> {
    let line = lines.get(line_idx)?.as_str();
    let (start, end) = token_bounds_at_col(line, col)?;
    let token = slice_by_char(line, start, end);
    if let Some(url) = normalize_http_url(&token) {
        return Some(url);
    }

    // Best-effort: if a long URL was wrapped mid-token, stitch contiguous chunks from adjacent
    // lines (where the token touches the line boundary) and then re-scan.
    let mut blob = token;
    let mut start_line = line_idx;
    let mut end_line = line_idx;
    let mut token_start = start;
    let mut token_end = end;

    for _ in 0..8 {
        if start_line == 0 {
            break;
        }
        let current = lines.get(start_line)?.as_str();
        let first_nonspace = current.chars().take_while(|ch| ch.is_whitespace()).count();
        if token_start > first_nonspace {
            break;
        }
        let prev = lines.get(start_line.saturating_sub(1))?.as_str();
        let (prev_token, prev_start, prev_end) = last_token(prev)?;
        let prev_trim_len = prev
            .trim_end_matches(|ch: char| ch.is_whitespace())
            .chars()
            .count();
        if prev_end != prev_trim_len {
            break;
        }
        if !looks_like_url_token(prev_token.as_str()) {
            break;
        }
        blob = format!("{prev_token}{blob}");
        start_line = start_line.saturating_sub(1);
        token_start = prev_start;
    }

    for _ in 0..8 {
        let current = lines.get(end_line)?.as_str();
        let trim_len = current
            .trim_end_matches(|ch: char| ch.is_whitespace())
            .chars()
            .count();
        if token_end < trim_len {
            break;
        }
        let next_line = end_line.saturating_add(1);
        if next_line >= lines.len() {
            break;
        }
        let next = lines.get(next_line)?.as_str();
        let (next_token, _next_start, next_end) = first_token(next)?;
        if !looks_like_url_token(next_token.as_str()) {
            break;
        }
        blob = format!("{blob}{next_token}");
        end_line = next_line;
        token_end = next_end;
    }

    normalize_http_url(&blob)
}

pub(super) fn token_bounds_at_col(line: &str, col: usize) -> Option<(usize, usize)> {
    let chars = line.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return None;
    }
    let mut pos = col.min(chars.len());
    if pos == chars.len() && pos > 0 {
        pos = pos.saturating_sub(1);
    }
    while pos > 0 && chars[pos].is_whitespace() {
        pos = pos.saturating_sub(1);
    }
    if chars[pos].is_whitespace() {
        return None;
    }
    let mut start = pos;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start = start.saturating_sub(1);
    }
    let mut end = pos.saturating_add(1);
    while end < chars.len() && !chars[end].is_whitespace() {
        end = end.saturating_add(1);
    }
    Some((start, end))
}

pub(super) fn first_token(line: &str) -> Option<(String, usize, usize)> {
    let len = line.chars().count();
    if len == 0 {
        return None;
    }
    let start = line.chars().take_while(|ch| ch.is_whitespace()).count();
    if start >= len {
        return None;
    }
    let mut end = start;
    for (idx, ch) in line.chars().enumerate().skip(start) {
        if ch.is_whitespace() {
            break;
        }
        end = idx.saturating_add(1);
    }
    let token = slice_by_char(line, start, end);
    Some((token, start, end))
}

pub(super) fn last_token(line: &str) -> Option<(String, usize, usize)> {
    let trimmed = line.trim_end_matches(|ch: char| ch.is_whitespace());
    let len = trimmed.chars().count();
    if len == 0 {
        return None;
    }
    let end = len;
    let mut start = end.saturating_sub(1);
    let chars = trimmed.chars().collect::<Vec<_>>();
    while start > 0 && !chars[start - 1].is_whitespace() {
        start = start.saturating_sub(1);
    }
    let token = slice_by_char(trimmed, start, end);
    Some((token, start, end))
}

pub(super) fn looks_like_url_token(token: &str) -> bool {
    let token = token.trim_matches(|ch: char| matches!(ch, '`' | '<' | '>' | '"' | '\''));
    if token.is_empty() {
        return false;
    }
    token.chars().all(|ch| {
        ch.is_ascii_alphanumeric()
            || matches!(
                ch,
                ':' | '/'
                    | '?'
                    | '#'
                    | '['
                    | ']'
                    | '@'
                    | '!'
                    | '$'
                    | '&'
                    | '\''
                    | '('
                    | ')'
                    | '*'
                    | '+'
                    | ','
                    | ';'
                    | '='
                    | '.'
                    | '_'
                    | '-'
                    | '~'
                    | '%'
            )
    })
}

pub(super) fn normalize_http_url(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let https = text.find("https://");
    let http = text.find("http://");
    let start = match (https, http) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }?;
    let mut url = &text[start..];
    url = url.trim_matches(|ch: char| matches!(ch, '`' | '<' | '>' | '"' | '\''));
    url = url.trim_end_matches(['.', ',', ';', ':', ')', ']', '}']);
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url.to_string())
    } else {
        None
    }
}

pub(super) fn handle_mouse_down_with_swarm(
    swarm: &SwarmRuntime,
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    input_state: &mut InputState,
    clipboard: &mut Option<Clipboard>,
    theme: &Theme,
) -> bool {
    if state.command_line.is_some() || state.prompt.is_some() {
        return true;
    }
    if state.rule_picker.open || state.protocol_picker.open {
        return true;
    }
    if state.agents.artifacts_popup_open {
        // Check if the click is inside the popup's chat input box first.
        let popup_area = dynamic_popup_rect(screen, artifacts_popup::preferred_size(screen));
        if let Some(cursor_char_idx) = artifacts_popup::map_chat_input_point_to_cursor(
            state,
            swarm,
            popup_area,
            mouse.column,
            mouse.row,
            false,
        ) {
            reset_ui_selection(state, input_state);
            let total_chars = state.agents.artifacts_popup_chat_input.chars().count();
            let new_cursor = cursor_char_idx.min(total_chars);
            if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                if state.agents.artifacts_popup_chat_selection_anchor.is_none() {
                    state.agents.artifacts_popup_chat_selection_anchor =
                        Some(state.agents.artifacts_popup_chat_cursor.min(total_chars));
                }
            } else {
                state.agents.artifacts_popup_chat_selection_anchor = Some(new_cursor);
            }
            state.agents.artifacts_popup_chat_cursor = new_cursor;
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::PopupChatInput,
                line: 0,
                col: 0,
            });
            copy_popup_chat_input_selection(state, clipboard);
        } else if let Some((line_idx, col, lines)) =
            map_artifacts_popup_mouse_with_swarm(swarm, mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::ArtifactsPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::ArtifactsPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::ArtifactsPopup,
                &lines,
                clipboard,
                input_state,
            );
        } else if !point_in_rect(mouse.column, mouse.row, popup_area) {
            reset_ui_selection(state, input_state);
            state.agents.artifacts_popup_open = false;
            state.agents.artifacts_popup_scroll = 0;
            state.agents.global_archive_opened_entry = None;
        }
        return true;
    }
    if state.agents.global_archive_open {
        if let Some((line_idx, col, lines)) =
            map_artifacts_history_popup_mouse(mouse, screen, state, theme, false)
        {
            if let Some(entry_idx) = artifacts_history_popup::entry_index_for_line(state, line_idx)
            {
                // Click on already-selected entry opens it.
                if state.agents.global_archive_selected == entry_idx {
                    load_selected_global_archive_entry(state);
                    return true;
                }
                state.agents.global_archive_selected = entry_idx;
            }
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::ArtifactsHistoryPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::ArtifactsHistoryPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::ArtifactsHistoryPopup,
                &lines,
                clipboard,
                input_state,
            );
        }
        // Swallow clicks outside — don't close the RAG popup.
        return true;
    }
    if state.show_help {
        if let Some((line_idx, col, lines)) =
            map_help_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::HelpPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::HelpPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::HelpPopup,
                &lines,
                clipboard,
                input_state,
            );
        }
        return true;
    }
    if state.app_kind == AppKind::Games && state.games.run_browser.open {
        if let Some((line_idx, col, lines)) =
            map_run_browser_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::GamesRunBrowserPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::GamesRunBrowserPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::GamesRunBrowserPopup,
                &lines,
                clipboard,
                input_state,
            );
        }
        return true;
    }
    if state.app_kind == AppKind::Games && state.games.replay.open {
        if let Some((line_idx, col, lines)) =
            map_replay_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::GamesReplayPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::GamesReplayPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::GamesReplayPopup,
                &lines,
                clipboard,
                input_state,
            );
        }
        return true;
    }
    if state.app_kind == AppKind::Games && state.games.strategy_inspect.open {
        if let Some((line_idx, col, lines)) =
            map_strategy_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::GamesStrategyPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::GamesStrategyPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::GamesStrategyPopup,
                &lines,
                clipboard,
                input_state,
            );
        }
        return true;
    }
    if state.app_kind == AppKind::Games && state.games.tm_sim.open {
        if let Some((pane, line_idx, col, lines)) =
            map_tm_sim_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(pane),
                line: line_idx,
                col,
            });
            update_ui_selection_text(state, pane, &lines, clipboard, input_state);
        }
        return true;
    }
    if state.app_kind == AppKind::Games && state.games.ca_sim.open {
        if let Some((pane, line_idx, col, lines)) =
            map_ca_sim_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(pane),
                line: line_idx,
                col,
            });
            update_ui_selection_text(state, pane, &lines, clipboard, input_state);
        }
        return true;
    }
    if state.app_kind == AppKind::Games && state.games.match_history.open {
        if let Some((line_idx, col, lines)) =
            map_match_history_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::GamesMatchHistoryPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::GamesMatchHistoryPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::GamesMatchHistoryPopup,
                &lines,
                clipboard,
                input_state,
            );
        }
        return true;
    }
    if state.app_kind == AppKind::Games && state.games.analysis.open {
        if let Some((line_idx, col, lines)) =
            map_analysis_popup_mouse(mouse, screen, state, theme, false)
        {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::GamesAnalysisPopup,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::GamesAnalysisPopup),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::GamesAnalysisPopup,
                &lines,
                clipboard,
                input_state,
            );
        }
        return true;
    }
    if games_petri_visible(state) {
        if let Some((line_idx, col, lines)) = map_games_petri_mouse(mouse, screen, state, false) {
            reset_ui_selection(state, input_state);
            state.ui_selection = Some(UiSelection {
                pane: UiSelectionPane::GamesPetriDish,
                start_line: line_idx,
                start_col: col,
                end_line: line_idx,
                end_col: col,
            });
            input_state.mouse_select_anchor = Some(MouseSelectAnchor {
                target: MouseSelectTarget::Ui(UiSelectionPane::GamesPetriDish),
                line: line_idx,
                col,
            });
            update_ui_selection_text(
                state,
                UiSelectionPane::GamesPetriDish,
                &lines,
                clipboard,
                input_state,
            );
        }
        return true;
    }
    reset_ui_selection(state, input_state);
    let layout = layout::split(screen);
    if point_in_rect(mouse.column, mouse.row, layout.editor) {
        set_buffer_cursor_from_mouse(
            state,
            PaneId::Editor,
            mouse,
            layout.editor,
            state.settings.editor.tab_width as usize,
            false,
        );
        if state.mode == Mode::Visual {
            state.mode = Mode::Normal;
        }
        state.editor_buffer_mut().clear_selection();
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::Buffer(PaneId::Editor),
            line: state.editor_buffer().cursor.line,
            col: state.editor_buffer().cursor.col,
        });
        return true;
    }
    if let Some(cursor_char_idx) = agent_console_view::map_chat_input_point_to_cursor(
        layout.notes,
        state,
        mouse.column,
        mouse.row,
        false,
    ) {
        reset_ui_selection(state, input_state);
        state.focus = PaneId::Notes;
        state.mode = Mode::Normal;
        let total_chars = state.agents.chat_input.chars().count();
        let new_cursor = cursor_char_idx.min(total_chars);
        if mouse.modifiers.contains(KeyModifiers::SHIFT) {
            if state.agents.chat_input_selection_anchor.is_none() {
                state.agents.chat_input_selection_anchor =
                    Some(state.agents.chat_input_cursor.min(total_chars));
            }
        } else {
            state.agents.chat_input_selection_anchor = Some(new_cursor);
        }
        state.agents.chat_input_cursor = new_cursor;
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::ChatInput,
            line: 0,
            col: 0,
        });
        copy_chat_input_selection(state, clipboard);
        return true;
    }
    if let Some((line_idx, col, lines)) =
        map_agent_console_mouse_with_swarm(swarm, mouse, screen, state, false)
    {
        state.focus = PaneId::Notes;
        state.mode = Mode::Normal;
        state.agents.chat_input_selection_anchor = None;
        if let Some(text_area) = agent_console_view::thread_text_area(layout.notes, state) {
            if maybe_open_artifact_popup_from_console_line(
                state,
                swarm,
                text_area.width as usize,
                line_idx,
            ) {
                input_state.mouse_select_anchor = None;
                return true;
            }
        }
        state.ui_selection = Some(UiSelection {
            pane: UiSelectionPane::AgentConsole,
            start_line: line_idx,
            start_col: col,
            end_line: line_idx,
            end_col: col,
        });
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::Ui(UiSelectionPane::AgentConsole),
            line: line_idx,
            col,
        });
        update_ui_selection_text(
            state,
            UiSelectionPane::AgentConsole,
            &lines,
            clipboard,
            input_state,
        );
        return true;
    }
    if point_in_rect(mouse.column, mouse.row, layout.notes) {
        reset_ui_selection(state, input_state);
        state.focus = PaneId::Notes;
        state.mode = Mode::Normal;
        state.agents.chat_input_selection_anchor = None;
        input_state.mouse_select_anchor = None;
        return true;
    }
    let agent_ops_tabs_area = agent_ops_tab_bar_area(layout.job);
    if point_in_rect(mouse.column, mouse.row, agent_ops_tabs_area) {
        reset_ui_selection(state, input_state);
        state.focus = PaneId::JobOutput;
        let rel_col = mouse.column.saturating_sub(agent_ops_tabs_area.x) as usize;
        if let Some(tab) = agent_ops_view::tab_at_column(rel_col) {
            if state.agents.dock_tab != tab {
                state.agents.dock_tab = tab;
                state.agents.roster_tree_selected = None;
                state.agents.ops_scroll = 0;
            }
            state.mode = if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                Mode::Insert
            } else {
                Mode::Normal
            };
            state.agents.note_event();
        }
        input_state.mouse_select_anchor = None;
        return true;
    }
    let scratchpad_area = agent_ops_scratchpad_editor_area(layout.job);
    if point_in_rect(mouse.column, mouse.row, scratchpad_area)
        && state.agents.dock_tab == AgentOpsTab::Scratchpad
    {
        set_buffer_cursor_from_mouse(
            state,
            PaneId::JobOutput,
            mouse,
            scratchpad_area,
            state.settings.editor.tab_width as usize,
            false,
        );
        if state.mode == Mode::Visual {
            state.mode = Mode::Normal;
        }
        state.notes_buffer_mut().clear_selection();
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::Buffer(PaneId::JobOutput),
            line: state.notes_buffer().cursor.line,
            col: state.notes_buffer().cursor.col,
        });
        return true;
    }
    if point_in_rect(mouse.column, mouse.row, layout.job)
        && state.agents.dock_tab == AgentOpsTab::Scratchpad
    {
        state.focus = PaneId::JobOutput;
        state.mode = Mode::Insert;
        input_state.mouse_select_anchor = None;
        return true;
    }
    {
        let layout = layout::split(screen);
        if mouse.row == layout.visualizer.y {
            let col_in_rect = mouse.column.saturating_sub(layout.visualizer.x);
            if let Some(action) = visualizer_view::title_button_hit(col_in_rect) {
                state.focus = PaneId::Visualizer;
                apply_action(state, action);
                return true;
            }
        }
        // Gate monitor title buttons (STATS / FILESCORES).
        if mouse.row == layout.gate.y {
            let col_in_rect = mouse.column.saturating_sub(layout.gate.x);
            // Compute the title prefix length to find button positions.
            let prefix_len = if let Some(report) = state
                .editor_buffer()
                .path()
                .and_then(|p| state.genome_reports.get(p))
            {
                format!(
                    " CODE STRUCTURAL QUALITY [{}x{}] ",
                    report.grid_size, report.grid_size
                )
                .len() as u16
            } else {
                " CODE STRUCTURAL QUALITY ".len() as u16
            };
            if let Some(action) = gate_monitor_view::title_button_hit(col_in_rect, prefix_len) {
                state.focus = PaneId::GateMonitor;
                apply_action(state, action);
                return true;
            }
        }
    }
    if let Some((line_idx, col, lines)) =
        map_visualizer_side_mouse(mouse, screen, state, theme, false)
    {
        state.focus = PaneId::Visualizer;
        state.ui_selection = Some(UiSelection {
            pane: UiSelectionPane::VisualizerSide,
            start_line: line_idx,
            start_col: col,
            end_line: line_idx,
            end_col: col,
        });
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::Ui(UiSelectionPane::VisualizerSide),
            line: line_idx,
            col,
        });
        update_ui_selection_text(
            state,
            UiSelectionPane::VisualizerSide,
            &lines,
            clipboard,
            input_state,
        );
        return true;
    }
    if let Some((line_idx, col, lines)) =
        map_visualizer_main_mouse(mouse, screen, state, theme, false)
    {
        state.focus = PaneId::Visualizer;
        state.ui_selection = Some(UiSelection {
            pane: UiSelectionPane::VisualizerMain,
            start_line: line_idx,
            start_col: col,
            end_line: line_idx,
            end_col: col,
        });
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::Ui(UiSelectionPane::VisualizerMain),
            line: line_idx,
            col,
        });
        update_ui_selection_text(
            state,
            UiSelectionPane::VisualizerMain,
            &lines,
            clipboard,
            input_state,
        );
        return true;
    }
    if let Some((line_idx, col, lines)) = map_gate_monitor_mouse(mouse, screen, state, theme, false)
    {
        state.focus = PaneId::GateMonitor;
        state.ui_selection = Some(UiSelection {
            pane: UiSelectionPane::GateMonitor,
            start_line: line_idx,
            start_col: col,
            end_line: line_idx,
            end_col: col,
        });
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::Ui(UiSelectionPane::GateMonitor),
            line: line_idx,
            col,
        });
        update_ui_selection_text(
            state,
            UiSelectionPane::GateMonitor,
            &lines,
            clipboard,
            input_state,
        );
        return true;
    }
    if let Some((line_idx, col, text_width, lines)) =
        map_job_output_mouse_with_swarm(swarm, mouse, screen, state, false)
    {
        state.focus = PaneId::JobOutput;
        apply_agent_ops_click_selection(state, line_idx, col, text_width, &lines);
        if state.agents.dock_tab == AgentOpsTab::Scratchpad {
            state.mode = Mode::Insert;
        }
        state.ui_selection = Some(UiSelection {
            pane: UiSelectionPane::JobOutput,
            start_line: line_idx,
            start_col: col,
            end_line: line_idx,
            end_col: col,
        });
        input_state.mouse_select_anchor = Some(MouseSelectAnchor {
            target: MouseSelectTarget::Ui(UiSelectionPane::JobOutput),
            line: line_idx,
            col,
        });
        update_ui_selection_text(
            state,
            UiSelectionPane::JobOutput,
            &lines,
            clipboard,
            input_state,
        );
        return true;
    }
    false
}

pub(super) fn apply_agent_ops_click_selection(
    state: &mut AppState,
    line_idx: usize,
    col: usize,
    text_width: usize,
    lines: &[String],
) {
    let offset = match state.agents.dock_tab {
        AgentOpsTab::Roster => agent_ops_view::roster_body_offset(state),
        _ => 2,
    };
    if state.agents.dock_tab == AgentOpsTab::Roster
        && line_idx == agent_ops_view::roster_swarm_template_line_idx(state)
    {
        if let Some(template) = agent_ops_view::roster_swarm_template_hit(col) {
            state.agents.swarm_default_template = template.to_string();
            state.agents.roster_tree_selected = None;
        }
        return;
    }
    if state.agents.dock_tab == AgentOpsTab::Roster
        && line_idx == agent_ops_view::roster_swarm_mission_line_idx(state)
    {
        if let Some(mission) = agent_ops_view::roster_swarm_mission_hit(col) {
            state.agents.swarm_default_mission = mission.to_string();
            state.agents.roster_tree_selected = None;
        }
        return;
    }
    if line_idx < offset {
        return;
    }
    let data_line = line_idx.saturating_sub(offset);
    match state.agents.dock_tab {
        AgentOpsTab::Roster => {
            let Some(meta) = agent_ops_view::roster_meta_for_body_line(state, data_line) else {
                return;
            };
            if let agent_ops_view::RosterBodyNode::Backend { backend } = meta.node {
                select_roster_backend(state, backend);
                let _ = toggle_roster_backend_expanded(state, backend);
                return;
            }
            let Some(agent_idx) = meta.agent_idx else {
                return;
            };
            // Clicking the roster priority checkbox should NOT change selection or expand/collapse.
            if matches!(meta.node, agent_ops_view::RosterBodyNode::Agent) {
                if let Some(agent) = state.agents.agents.get(agent_idx) {
                    let checkbox_hit = agent.supports_swarm_priority()
                        && !agent.id.contains("#swarm-")
                        && (1..5).contains(&col);
                    if checkbox_hit {
                        if state.agents.swarm_priority_agent_ids.remove(&agent.id) {
                            // removed
                        } else {
                            state
                                .agents
                                .swarm_priority_agent_ids
                                .insert(agent.id.clone());
                        }
                        return;
                    }
                }
            }

            let was_selected = agent_idx == state.agents.roster_selected;
            sync_roster_selected_agent(state, agent_idx);
            if let Some(agent) = state.agents.agents.get(agent_idx) {
                match meta.node {
                    agent_ops_view::RosterBodyNode::Agent => {
                        let model_hit = agent_ops_view::roster_role_cell_hit(col, text_width);
                        if model_hit && !was_selected {
                            state
                                .agents
                                .roster_tree_collapsed_agent_ids
                                .remove(&agent.id);
                        } else if was_selected
                            && model_hit
                            && !state
                                .agents
                                .roster_tree_collapsed_agent_ids
                                .remove(&agent.id)
                        {
                            state
                                .agents
                                .roster_tree_collapsed_agent_ids
                                .insert(agent.id.clone());
                        }
                    }
                    agent_ops_view::RosterBodyNode::Branch { branch } => {
                        let leaf_idx = match branch {
                            nit_core::RosterTreeBranch::Size => {
                                let efforts = state
                                    .agents
                                    .codex_supported_reasoning_efforts
                                    .get(&agent.id)
                                    .map(|v| v.as_slice())
                                    .unwrap_or(&[]);
                                let current = state
                                    .agents
                                    .codex_selected_reasoning_effort
                                    .get(&agent.id)
                                    .or_else(|| {
                                        state.agents.codex_default_reasoning_effort.get(&agent.id)
                                    })
                                    .map(|s| s.as_str());
                                current
                                    .and_then(|effort| efforts.iter().position(|e| e == effort))
                                    .unwrap_or(0)
                                    .min(efforts.len().saturating_sub(1))
                            }
                            nit_core::RosterTreeBranch::Role => {
                                let current = state
                                    .agents
                                    .swarm_role_by_agent_id
                                    .get(&agent.id)
                                    .map(|s| s.trim())
                                    .filter(|s| !s.is_empty());
                                current
                                    .and_then(|role| {
                                        let current = normalize_swarm_role_hint_for_roster(role);
                                        SWARM_ROLE_OPTIONS.iter().position(|candidate| {
                                            current
                                                == normalize_swarm_role_hint_for_roster(candidate)
                                        })
                                    })
                                    .unwrap_or(0)
                                    .min(SWARM_ROLE_OPTIONS.len().saturating_sub(1))
                            }
                        };
                        state.agents.roster_tree_selected =
                            Some(nit_core::RosterTreeSelection { branch, leaf_idx });
                    }
                    agent_ops_view::RosterBodyNode::Leaf { branch, leaf_idx } => {
                        state.agents.roster_tree_selected =
                            Some(nit_core::RosterTreeSelection { branch, leaf_idx });
                        let _ = select_roster_tree_leaf(state);
                    }
                    agent_ops_view::RosterBodyNode::Backend { .. } => (),
                }
            }
        }
        AgentOpsTab::Missions => {
            let Some(mission_idx) = agent_ops_view::mission_index_for_body_line(state, data_line)
            else {
                return;
            };
            state.agents.mission_selected = mission_idx;
            if let Some(mission) = state.agents.missions.get(mission_idx) {
                state.agents.selected_mission = Some(mission.id.clone());
            }
        }
        AgentOpsTab::Alerts => {
            let Some(alert_idx) =
                agent_ops_view::alert_index_for_body_line(state, text_width, data_line)
            else {
                return;
            };
            state.agents.alert_selected = alert_idx;
        }
        AgentOpsTab::Evidence => {
            if let Some(card_idx) = agent_ops_view::artifacts_card_index_for_line(lines, line_idx) {
                state.agents.artifacts_selected = card_idx;
                state.agents.artifacts_popup_open = true;
                state.agents.artifacts_popup_scroll = 0;
            }
        }
        AgentOpsTab::Patch
        | AgentOpsTab::Diagnostics
        | AgentOpsTab::Dag
        | AgentOpsTab::Scratchpad => {}
        AgentOpsTab::Mcp => {}
    }
}

pub(super) fn handle_mouse_drag_with_swarm(
    swarm: &SwarmRuntime,
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &mut AppState,
    input_state: &mut InputState,
    clipboard: &mut Option<Clipboard>,
    theme: &Theme,
) -> bool {
    let Some(anchor) = input_state.mouse_select_anchor else {
        return false;
    };
    if !mouse_drag_allowed(state, anchor) {
        input_state.mouse_select_anchor = None;
        return true;
    }
    match anchor.target {
        MouseSelectTarget::Buffer(pane) => {
            let layout = layout::split(screen);
            let (pane_rect, tab_width) = match pane {
                PaneId::Editor => (layout.editor, state.settings.editor.tab_width as usize),
                PaneId::JobOutput if state.agents.dock_tab == AgentOpsTab::Scratchpad => (
                    agent_ops_scratchpad_editor_area(layout.job),
                    state.settings.editor.tab_width as usize,
                ),
                _ => return false,
            };
            state.focus = pane;
            let buffer = match pane {
                PaneId::Editor => state.editor_buffer_mut(),
                PaneId::JobOutput if state.agents.dock_tab == AgentOpsTab::Scratchpad => {
                    state.notes_buffer_mut()
                }
                _ => return false,
            };
            let Some((line, col)) = mouse_to_buffer_pos(mouse, pane_rect, buffer, tab_width, true)
            else {
                return false;
            };
            if buffer.selection_range().is_none() {
                buffer.cursor.line = anchor.line;
                buffer.cursor.col = anchor.col;
                buffer.set_selection_anchor();
            }
            buffer.cursor.line = line;
            buffer.cursor.col = col;
            buffer.ensure_visible();
            state.mode = Mode::Visual;
            handle_selection_autocopy(state, clipboard, input_state);
            true
        }
        MouseSelectTarget::ChatInput => {
            let layout = layout::split(screen);
            state.focus = PaneId::Notes;
            state.mode = Mode::Normal;
            let Some(cursor_char_idx) = agent_console_view::map_chat_input_point_to_cursor(
                layout.notes,
                state,
                mouse.column,
                mouse.row,
                true,
            ) else {
                return false;
            };
            let total_chars = state.agents.chat_input.chars().count();
            if state.agents.chat_input_selection_anchor.is_none() {
                state.agents.chat_input_selection_anchor =
                    Some(state.agents.chat_input_cursor.min(total_chars));
            }
            state.agents.chat_input_cursor = cursor_char_idx.min(total_chars);
            state.agents.chat_input_scroll = usize::MAX;
            copy_chat_input_selection(state, clipboard);
            true
        }
        MouseSelectTarget::PopupChatInput => {
            let popup_area = dynamic_popup_rect(screen, artifacts_popup::preferred_size(screen));
            let Some(cursor_char_idx) = artifacts_popup::map_chat_input_point_to_cursor(
                state,
                swarm,
                popup_area,
                mouse.column,
                mouse.row,
                true,
            ) else {
                return false;
            };
            let total_chars = state.agents.artifacts_popup_chat_input.chars().count();
            if state.agents.artifacts_popup_chat_selection_anchor.is_none() {
                state.agents.artifacts_popup_chat_selection_anchor =
                    Some(state.agents.artifacts_popup_chat_cursor.min(total_chars));
            }
            state.agents.artifacts_popup_chat_cursor = cursor_char_idx.min(total_chars);
            state.agents.artifacts_popup_chat_scroll = usize::MAX;
            copy_popup_chat_input_selection(state, clipboard);
            true
        }
        MouseSelectTarget::Ui(pane) => {
            let result = match pane {
                UiSelectionPane::JobOutput => {
                    { map_job_output_mouse_with_swarm(swarm, mouse, screen, state, true) }
                        .map(|(line_idx, col, _text_width, lines)| (line_idx, col, lines))
                }
                UiSelectionPane::AgentConsole => {
                    map_agent_console_mouse_with_swarm(swarm, mouse, screen, state, true)
                }
                UiSelectionPane::GamesPetriDish => {
                    map_games_petri_mouse(mouse, screen, state, true)
                }
                UiSelectionPane::VisualizerMain => {
                    map_visualizer_main_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::VisualizerSide => {
                    map_visualizer_side_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::GateMonitor => {
                    map_gate_monitor_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::HelpPopup => {
                    map_help_popup_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::ArtifactsHistoryPopup => {
                    map_artifacts_history_popup_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::ArtifactsPopup => {
                    map_artifacts_popup_mouse_with_swarm(swarm, mouse, screen, state, theme, true)
                }
                UiSelectionPane::GamesAnalysisPopup => {
                    map_analysis_popup_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::GamesRunBrowserPopup => {
                    map_run_browser_popup_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::GamesReplayPopup => {
                    map_replay_popup_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::GamesStrategyPopup => {
                    map_strategy_popup_mouse(mouse, screen, state, theme, true)
                }
                UiSelectionPane::GamesTmSimPopupLeft | UiSelectionPane::GamesTmSimPopupRight => {
                    map_tm_sim_popup_mouse_for_pane(mouse, screen, state, theme, true, pane)
                }
                UiSelectionPane::GamesCaSimPopupLeft | UiSelectionPane::GamesCaSimPopupRight => {
                    map_ca_sim_popup_mouse_for_pane(mouse, screen, state, theme, true, pane)
                }
                UiSelectionPane::GamesMatchHistoryPopup => {
                    map_match_history_popup_mouse(mouse, screen, state, theme, true)
                }
            };
            let Some((line_idx, col, lines)) = result else {
                return false;
            };
            let adjusted_col = if matches!(pane, UiSelectionPane::AgentConsole) {
                adjust_agent_console_drag_col(&lines, anchor.line, line_idx, col)
            } else {
                col
            };
            state.ui_selection = Some(UiSelection {
                pane,
                start_line: anchor.line,
                start_col: anchor.col,
                end_line: line_idx,
                end_col: adjusted_col,
            });
            update_ui_selection_text(state, pane, &lines, clipboard, input_state);
            true
        }
    }
}
