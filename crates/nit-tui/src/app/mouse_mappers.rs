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

pub(super) fn map_agent_console_mouse_with_swarm(
    swarm: &SwarmRuntime,
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    let layout = layout::split(screen);
    let text_area = agent_console_view::thread_text_area(layout.notes, state)?;
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = agent_console_view::thread_lines_for_selection_with_swarm(
        state,
        swarm,
        text_area.width as usize,
    );
    if lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let total = lines.len();
    let max_scroll = total.saturating_sub(height);
    let scroll = state.agents.console_scroll.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, lines))
}

pub(super) fn map_job_output_mouse_with_swarm(
    swarm: &SwarmRuntime,
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    clamp: bool,
) -> Option<(usize, usize, usize, Vec<String>)> {
    let layout = layout::split(screen);
    let text_area = job_output_text_area(layout.job);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let text_width = text_area.width as usize;
    let lines = agent_ops_view::current_lines_for_width_with_swarm(state, Some(swarm), text_width);
    if lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let total = lines.len();
    let max_scroll = total.saturating_sub(height);
    let scroll = state.agents.ops_scroll.min(max_scroll);
    let start = scroll;
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &lines,
        start,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_width, lines))
}

pub(super) fn map_help_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if !state.show_help {
        return None;
    }
    let area = dynamic_popup_rect(screen, help_overlay::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = help_overlay::build_lines(theme);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.help_scroll.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

pub(super) fn map_artifacts_popup_mouse_with_swarm(
    swarm: &SwarmRuntime,
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if !state.agents.artifacts_popup_open {
        return None;
    }
    let area = dynamic_popup_rect(screen, artifacts_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = artifacts_popup::build_lines(state, swarm, theme, text_area.width);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.agents.artifacts_popup_scroll.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

pub(super) fn map_artifacts_history_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if !state.agents.global_archive_open {
        return None;
    }
    let area = dynamic_popup_rect(screen, artifacts_history_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = artifacts_history_popup::build_lines(state, theme, text_area.width);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.agents.global_archive_scroll.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

pub(super) fn map_analysis_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.analysis.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_analysis_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = games_analysis_popup::build_lines(state, theme, text_area.width);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.games.analysis.scroll_offset.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

pub(super) fn map_run_browser_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.run_browser.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_run_browser_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = games_run_browser_popup::build_lines(state, theme, text_area.width);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.games.run_browser.scroll_offset.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

pub(super) fn map_replay_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.replay.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_replay_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = games_replay_popup::build_lines(state, theme, text_area.width);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.games.replay.scroll_offset.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

pub(super) fn map_strategy_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.strategy_inspect.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_strategy_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = games_strategy_popup::build_lines(state, theme, text_area.width);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = text_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.games.strategy_inspect.scroll_offset.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

pub(super) fn map_tm_sim_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(UiSelectionPane, usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.tm_sim.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_tm_sim_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let (_left_area, right_area) = games_tm_sim_popup::layout_for_tm_sim(text_area);
    let right_inner = right_area.map(|area| Block::default().borders(Borders::ALL).inner(area));
    let pane = if let Some(right_inner) = right_inner {
        if point_in_rect(mouse.column, mouse.row, right_inner) {
            UiSelectionPane::GamesTmSimPopupRight
        } else {
            UiSelectionPane::GamesTmSimPopupLeft
        }
    } else {
        UiSelectionPane::GamesTmSimPopupLeft
    };
    let (line_idx, col, lines) =
        map_tm_sim_popup_mouse_for_pane(mouse, screen, state, theme, clamp, pane)?;
    Some((pane, line_idx, col, lines))
}

pub(super) fn map_tm_sim_popup_mouse_for_pane(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
    pane: UiSelectionPane,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.tm_sim.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_tm_sim_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let (left_area, right_area) = games_tm_sim_popup::layout_for_tm_sim(text_area);
    let right_inner = right_area.map(|area| Block::default().borders(Borders::ALL).inner(area));
    let (target_area, lines) = match pane {
        UiSelectionPane::GamesTmSimPopupRight => {
            let right_inner = right_inner?;
            let right_width = right_inner.width.max(1) as usize;
            let (_left_lines, right_lines) = games_tm_sim_popup::build_columns(
                state,
                theme,
                left_area.width.max(1) as usize,
                right_width,
            );
            (right_inner, right_lines)
        }
        _ => {
            let right_width = right_inner
                .map(|area| area.width.max(1) as usize)
                .unwrap_or(0);
            let (left_lines, _right_lines) = games_tm_sim_popup::build_columns(
                state,
                theme,
                left_area.width.max(1) as usize,
                right_width,
            );
            (left_area, left_lines)
        }
    };
    if !point_in_rect(mouse.column, mouse.row, target_area) && !clamp {
        return None;
    }
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = target_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.games.tm_sim.scroll_offset.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        target_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

pub(super) fn map_ca_sim_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(UiSelectionPane, usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.ca_sim.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_ca_sim_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let (_left_area, right_area) = games_ca_sim_popup::layout_for_ca_sim(text_area);
    let right_inner = right_area.map(|area| Block::default().borders(Borders::ALL).inner(area));
    let pane = if let Some(right_inner) = right_inner {
        if point_in_rect(mouse.column, mouse.row, right_inner) {
            UiSelectionPane::GamesCaSimPopupRight
        } else {
            UiSelectionPane::GamesCaSimPopupLeft
        }
    } else {
        UiSelectionPane::GamesCaSimPopupLeft
    };
    let (line_idx, col, lines) =
        map_ca_sim_popup_mouse_for_pane(mouse, screen, state, theme, clamp, pane)?;
    Some((pane, line_idx, col, lines))
}

pub(super) fn map_ca_sim_popup_mouse_for_pane(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
    pane: UiSelectionPane,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.ca_sim.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_ca_sim_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let (left_area, right_area) = games_ca_sim_popup::layout_for_ca_sim(text_area);
    let right_inner = right_area.map(|area| Block::default().borders(Borders::ALL).inner(area));
    let (target_area, lines) = match pane {
        UiSelectionPane::GamesCaSimPopupRight => {
            let right_inner = right_inner?;
            let right_width = right_inner.width.max(1) as usize;
            let (_left_lines, right_lines) = games_ca_sim_popup::build_columns(
                state,
                theme,
                left_area.width.max(1) as usize,
                right_width,
            );
            (right_inner, right_lines)
        }
        _ => {
            let right_width = right_inner
                .map(|area| area.width.max(1) as usize)
                .unwrap_or(0);
            let (left_lines, _right_lines) = games_ca_sim_popup::build_columns(
                state,
                theme,
                left_area.width.max(1) as usize,
                right_width,
            );
            (left_area, left_lines)
        }
    };
    if !point_in_rect(mouse.column, mouse.row, target_area) && !clamp {
        return None;
    }
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let height = target_area.height as usize;
    let max_scroll = text_lines.len().saturating_sub(height);
    let scroll = state.games.ca_sim.scroll_offset.min(max_scroll);
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        target_area,
        &text_lines,
        scroll,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

pub(super) fn map_match_history_popup_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games || !state.games.match_history.open {
        return None;
    }
    let area = dynamic_popup_rect(screen, games_match_history_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = games_match_history_popup::build_lines(state, theme, text_area);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &text_lines,
        0,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

pub(super) fn map_games_petri_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if !games_petri_visible(state) {
        return None;
    }
    let area = crate::games_petri_dish::petri_rect(screen);
    let text_area = Block::default().borders(Borders::ALL).inner(area);
    if !point_in_rect(mouse.column, mouse.row, text_area) && !clamp {
        return None;
    }
    let lines = state.games.petri_lines.clone();
    if lines.is_empty() {
        return None;
    }
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        text_area,
        &lines,
        0,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, lines))
}

pub(super) fn map_visualizer_main_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games {
        return None;
    }
    let layout = layout::split(screen);
    let inner = Block::default()
        .borders(Borders::ALL)
        .inner(layout.visualizer);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let config_result = current_games_config_result(state);
    let layout_info = games_visualizer_view::layout_for_config(
        inner,
        state,
        config_result.and_then(|result| result.as_ref().ok()),
    );
    let area = layout_info.main;
    if !point_in_rect(mouse.column, mouse.row, area) && !clamp {
        return None;
    }
    let lines = games_visualizer_view::build_main_lines(
        state,
        theme,
        config_result,
        state.games.config_preview_pending,
        layout_info.show_payoff_side,
        area.width as usize,
    );
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        area,
        &text_lines,
        0,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

pub(super) fn map_visualizer_side_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    if state.app_kind != AppKind::Games {
        return None;
    }
    let layout = layout::split(screen);
    let inner = Block::default()
        .borders(Borders::ALL)
        .inner(layout.visualizer);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let config_result = current_games_config_result(state);
    let layout_info = games_visualizer_view::layout_for_config(
        inner,
        state,
        config_result.and_then(|result| result.as_ref().ok()),
    );
    let side_area = layout_info.side?;
    let side_inner = Block::default().borders(Borders::ALL).inner(side_area);
    if side_inner.width == 0 || side_inner.height == 0 {
        return None;
    }
    if !point_in_rect(mouse.column, mouse.row, side_inner) && !clamp {
        return None;
    }
    let lines = games_visualizer_view::build_side_lines(state, theme, side_inner.width as usize);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        side_inner,
        &text_lines,
        0,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

pub(super) fn map_gate_monitor_mouse(
    mouse: MouseEvent,
    screen: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
    clamp: bool,
) -> Option<(usize, usize, Vec<String>)> {
    let layout = layout::split(screen);
    let inner = Block::default().borders(Borders::ALL).inner(layout.gate);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    if !point_in_rect(mouse.column, mouse.row, inner) && !clamp {
        return None;
    }
    let lines = gate_monitor_view::build_lines(state, theme, inner.width as usize);
    let text_lines = lines_to_strings(&lines);
    if text_lines.is_empty() {
        return None;
    }
    let (line_idx, col) = map_mouse_to_line_col(
        mouse,
        inner,
        &text_lines,
        0,
        state.settings.editor.tab_width as usize,
        clamp,
    )?;
    Some((line_idx, col, text_lines))
}

pub(super) fn map_mouse_to_line_col(
    mouse: MouseEvent,
    area: ratatui::layout::Rect,
    lines: &[String],
    scroll: usize,
    tab_width: usize,
    clamp: bool,
) -> Option<(usize, usize)> {
    if lines.is_empty() || area.height == 0 || area.width == 0 {
        return None;
    }
    let max_row = area.height.saturating_sub(1);
    let row = if clamp {
        if mouse.row < area.y {
            0
        } else {
            mouse.row.saturating_sub(area.y).min(max_row) as usize
        }
    } else if mouse.row < area.y || mouse.row > area.y.saturating_add(max_row) {
        return None;
    } else {
        mouse.row.saturating_sub(area.y) as usize
    };
    let line_idx = scroll
        .saturating_add(row)
        .min(lines.len().saturating_sub(1));
    let line = &lines[line_idx];
    let display_col = if mouse.column <= area.x {
        0
    } else {
        (mouse.column - area.x) as usize
    };
    let col = char_idx_for_display_col(line, display_col, tab_width);
    Some((line_idx, col))
}

pub(super) fn set_buffer_cursor_from_mouse(
    state: &mut AppState,
    pane: PaneId,
    mouse: MouseEvent,
    area: ratatui::layout::Rect,
    tab_width: usize,
    clamp: bool,
) {
    state.focus = pane;
    let buffer = match pane {
        PaneId::Editor => state.editor_buffer_mut(),
        PaneId::Notes => state.notes_buffer_mut(),
        PaneId::JobOutput if state.agents.dock_tab == AgentOpsTab::Scratchpad => {
            state.notes_buffer_mut()
        }
        _ => return,
    };
    let Some((line, col)) = mouse_to_buffer_pos(mouse, area, buffer, tab_width, clamp) else {
        return;
    };
    buffer.cursor.line = line;
    buffer.cursor.col = col;
    buffer.ensure_visible();
}

pub(super) fn mouse_to_buffer_pos(
    mouse: MouseEvent,
    area: ratatui::layout::Rect,
    buffer: &nit_core::Buffer,
    tab_width: usize,
    clamp: bool,
) -> Option<(usize, usize)> {
    let inner_x = area.x.saturating_add(1);
    let inner_y = area.y.saturating_add(1);
    let inner_width = area.width.saturating_sub(2) as usize;
    let inner_height = area.height.saturating_sub(2) as usize;
    if inner_width == 0 || inner_height == 0 {
        return None;
    }

    let total_lines = buffer.lines_len().max(1);
    let line_num_width = total_lines.to_string().len().max(3);
    let gutter_width = line_num_width + 4;
    let content_x = inner_x.saturating_add(gutter_width as u16);

    let row = if clamp {
        if mouse.row < inner_y {
            0
        } else {
            let max_row = inner_height.saturating_sub(1) as u16;
            mouse.row.saturating_sub(inner_y).min(max_row) as usize
        }
    } else if mouse.row < inner_y || mouse.row >= inner_y.saturating_add(inner_height as u16) {
        return None;
    } else {
        mouse.row.saturating_sub(inner_y) as usize
    };

    let line_idx = buffer
        .viewport
        .offset_line
        .saturating_add(row)
        .min(total_lines.saturating_sub(1));

    let mut line = buffer.line_as_string(line_idx);
    if line.ends_with('\n') {
        line.pop();
    }

    let display_offset = display_col_for_char_idx(&line, buffer.viewport.offset_col, tab_width);
    let display_col = if mouse.column <= content_x {
        0
    } else {
        (mouse.column - content_x) as usize
    };
    let target_display = display_offset.saturating_add(display_col);
    let col = char_idx_for_display_col(&line, target_display, tab_width);
    Some((line_idx, col))
}

pub(super) fn display_col_for_char_idx(line: &str, char_idx: usize, tab_width: usize) -> usize {
    let mut col = 0;
    for (count, ch) in line.chars().enumerate() {
        if count >= char_idx {
            break;
        }
        if ch == '\t' {
            let tab = tab_width.max(1);
            let advance = tab - (col % tab);
            col += advance;
        } else {
            let w = unicode_width::UnicodeWidthChar::width(ch)
                .unwrap_or(1)
                .max(1);
            col += w;
        }
    }
    col
}

pub(super) fn char_idx_for_display_col(line: &str, target_col: usize, tab_width: usize) -> usize {
    let mut col = 0;
    let mut idx = 0;
    for ch in line.chars() {
        let w = if ch == '\t' {
            let tab = tab_width.max(1);
            tab - (col % tab)
        } else {
            unicode_width::UnicodeWidthChar::width(ch)
                .unwrap_or(1)
                .max(1)
        };
        if col + w > target_col {
            break;
        }
        col += w;
        idx += 1;
    }
    idx
}
