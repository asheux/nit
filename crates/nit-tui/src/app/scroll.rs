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

pub(super) fn bump_scroll(value: &mut usize, delta: i32) {
    if delta.is_negative() {
        *value = value.saturating_sub(delta.unsigned_abs() as usize);
    } else if delta > 0 {
        *value = value.saturating_add(delta as usize);
    }
}

pub(super) fn bump_scroll_clamped(value: &mut usize, delta: i32, max_scroll: usize) {
    let mut scroll = (*value).min(max_scroll);
    bump_scroll(&mut scroll, delta);
    *value = scroll.min(max_scroll);
}

pub(super) fn popup_max_scroll(line_count: usize, text_area: ratatui::layout::Rect) -> usize {
    if text_area.height == 0 {
        return 0;
    }
    line_count.saturating_sub(text_area.height as usize)
}

pub(super) fn max_scroll_for_height(line_count: usize, height: usize) -> usize {
    if height == 0 {
        return 0;
    }
    line_count.saturating_sub(height)
}

pub(super) fn popup_page_step(text_area: ratatui::layout::Rect) -> usize {
    text_area.height.max(1) as usize
}

pub(super) fn popup_text_metrics(area: ratatui::layout::Rect, line_count: usize) -> (usize, usize) {
    let text_area = popup_text_area(area);
    (
        popup_max_scroll(line_count, text_area),
        popup_page_step(text_area),
    )
}

pub(super) fn global_archive_scroll_metrics(
    state: &AppState,
    screen: ratatui::layout::Rect,
    _theme: &Theme,
) -> (usize, usize) {
    // Cheap line count — avoids reallocating styled entry rows on every
    // wheel tick. Stays in sync with `artifacts_history_popup::build_lines`.
    let area = dynamic_popup_rect(screen, artifacts_history_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    (
        popup_max_scroll(artifacts_history_popup::line_count(state), text_area),
        popup_page_step(text_area),
    )
}

pub(super) fn help_popup_max_scroll(screen: ratatui::layout::Rect, _theme: &Theme) -> usize {
    // Help content is static — use the memoized line count instead of
    // rebuilding ~600 styled help lines on every scroll tick.
    let area = dynamic_popup_rect(screen, help_overlay::preferred_size(screen));
    let text_area = popup_text_area(area);
    popup_max_scroll(help_overlay::line_count(), text_area)
}

pub(super) fn help_popup_scroll_metrics(
    screen: ratatui::layout::Rect,
    _theme: &Theme,
) -> (usize, usize) {
    let area = dynamic_popup_rect(screen, help_overlay::preferred_size(screen));
    popup_text_metrics(area, help_overlay::line_count())
}

pub(super) fn games_analysis_popup_max_scroll(
    state: &AppState,
    screen: ratatui::layout::Rect,
    _theme: &Theme,
) -> usize {
    // Cheap line count — avoids rebuilding sparklines, strategy tables, and
    // styled spans on every wheel tick. Must stay in sync with `build_lines`.
    let area = dynamic_popup_rect(screen, games_analysis_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    popup_max_scroll(games_analysis_popup::line_count(state), text_area)
}

pub(super) fn games_analysis_popup_scroll_metrics(
    state: &AppState,
    screen: ratatui::layout::Rect,
    _theme: &Theme,
) -> (usize, usize) {
    let area = dynamic_popup_rect(screen, games_analysis_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    (
        popup_max_scroll(games_analysis_popup::line_count(state), text_area),
        popup_page_step(text_area),
    )
}

pub(super) fn games_run_browser_popup_max_scroll(
    state: &AppState,
    screen: ratatui::layout::Rect,
    _theme: &Theme,
) -> usize {
    // Cheap line count — avoids rebuilding styled line vectors on every wheel
    // tick. Must stay in sync with `games_run_browser_popup::build_lines`.
    let area = dynamic_popup_rect(screen, games_run_browser_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    popup_max_scroll(games_run_browser_popup::line_count(state), text_area)
}

pub(super) fn games_replay_popup_max_scroll(
    state: &AppState,
    screen: ratatui::layout::Rect,
    _theme: &Theme,
) -> usize {
    // Use the cheap `line_count` helper instead of `build_lines` — the scroll
    // hot path does not need styled/wrapped lines just to know how many there
    // are.
    let area = dynamic_popup_rect(screen, games_replay_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    popup_max_scroll(games_replay_popup::line_count(state), text_area)
}

pub(super) fn games_strategy_popup_max_scroll(
    state: &AppState,
    screen: ratatui::layout::Rect,
) -> usize {
    let area = dynamic_popup_rect(screen, games_strategy_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    games_strategy_popup::max_scroll(state, text_area)
}

pub(super) fn games_tm_sim_popup_max_scroll(
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> usize {
    // Prefer the cached value from the last render. Only fall back to
    // rebuilding `build_columns` (runs the TM simulation and formats grid +
    // rule tables) when the cache is still the `usize::MAX` sentinel — i.e.
    // a scroll event arrived before the first render after the popup opened.
    let cached = state.games.tm_sim.last_max_scroll;
    if cached != usize::MAX {
        return cached;
    }
    let area = dynamic_popup_rect(screen, games_tm_sim_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let (left_area, right_area) = games_tm_sim_popup::layout_for_tm_sim(text_area);
    let computed = if let Some(right_area) = right_area {
        let right_inner = Block::default().borders(Borders::ALL).inner(right_area);
        let (left_lines, right_lines) = games_tm_sim_popup::build_columns(
            state,
            theme,
            left_area.width.max(1) as usize,
            right_inner.width.max(1) as usize,
        );
        let content_height = left_area.height.min(right_inner.height) as usize;
        max_scroll_for_height(left_lines.len().max(right_lines.len()), content_height)
    } else {
        let (lines, _) =
            games_tm_sim_popup::build_columns(state, theme, text_area.width.max(1) as usize, 0);
        popup_max_scroll(lines.len(), text_area)
    };
    state.games.tm_sim.last_max_scroll = computed;
    computed
}

pub(super) fn games_ca_sim_popup_max_scroll(
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) -> usize {
    // Prefer the cached value from the last render. Fall back to rebuilding
    // `build_columns` (runs CA simulation + formats grid/rules) only when
    // no render has populated the cache yet.
    let cached = state.games.ca_sim.last_max_scroll;
    if cached != usize::MAX {
        return cached;
    }
    let area = dynamic_popup_rect(screen, games_ca_sim_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let (left_area, right_area) = games_ca_sim_popup::layout_for_ca_sim(text_area);
    let computed = if let Some(right_area) = right_area {
        let right_inner = Block::default().borders(Borders::ALL).inner(right_area);
        let (left_lines, right_lines) = games_ca_sim_popup::build_columns(
            state,
            theme,
            left_area.width.max(1) as usize,
            right_inner.width.max(1) as usize,
        );
        let content_height = left_area.height.min(right_inner.height) as usize;
        max_scroll_for_height(left_lines.len().max(right_lines.len()), content_height)
    } else {
        let (lines, _) =
            games_ca_sim_popup::build_columns(state, theme, text_area.width.max(1) as usize, 0);
        popup_max_scroll(lines.len(), text_area)
    };
    state.games.ca_sim.last_max_scroll = computed;
    computed
}

pub(super) fn games_match_history_max_offset(
    state: &AppState,
    screen: ratatui::layout::Rect,
) -> usize {
    let area = dynamic_popup_rect(screen, games_match_history_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    games_match_history_popup::max_column_offset(
        games_match_history_total_entries(state),
        text_area.width,
    )
}

pub(super) fn games_match_history_max_rounds(state: &AppState) -> usize {
    if state.games.match_history.max_rounds_seen > 0 {
        state.games.match_history.max_rounds_seen
    } else {
        games_match_history_popup::max_round_limit(state.games.match_history.entries.as_slice())
    }
}

pub(super) fn games_match_history_total_entries(state: &AppState) -> usize {
    if state.games.match_history.total_entries > 0 {
        state.games.match_history.total_entries
    } else {
        state.games.match_history.entries.len()
    }
}

pub(super) fn games_match_history_default_rounds(state: &AppState) -> usize {
    games_match_history_popup::default_round_limit(games_match_history_max_rounds(state))
}

pub(super) fn clamp_modal_scroll_offsets(
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) {
    if state.show_help {
        let max_scroll = help_popup_max_scroll(screen, theme);
        state.help_scroll = state.help_scroll.min(max_scroll);
    }
    if state.agents.global_archive_open {
        let (max_scroll, _) = global_archive_scroll_metrics(state, screen, theme);
        state.agents.global_archive_scroll = state.agents.global_archive_scroll.min(max_scroll);
        let max = state.agents.global_archive_filtered.len().saturating_sub(1);
        state.agents.global_archive_selected = state.agents.global_archive_selected.min(max);
    }
    if state.app_kind != AppKind::Games {
        return;
    }
    if state.games.analysis.open {
        let max_scroll = games_analysis_popup_max_scroll(state, screen, theme);
        state.games.analysis.scroll_offset = state.games.analysis.scroll_offset.min(max_scroll);
    }
    if state.games.run_browser.open {
        let max_scroll = games_run_browser_popup_max_scroll(state, screen, theme);
        state.games.run_browser.scroll_offset =
            state.games.run_browser.scroll_offset.min(max_scroll);
    }
    if state.games.replay.open {
        let max_scroll = games_replay_popup_max_scroll(state, screen, theme);
        state.games.replay.scroll_offset = state.games.replay.scroll_offset.min(max_scroll);
    }
    if state.games.strategy_inspect.open {
        let max_scroll = games_strategy_popup_max_scroll(state, screen);
        state.games.strategy_inspect.scroll_offset =
            state.games.strategy_inspect.scroll_offset.min(max_scroll);
    }
    if state.games.tm_sim.open {
        let max_scroll = games_tm_sim_popup_max_scroll(state, screen, theme);
        state.games.tm_sim.scroll_offset = state.games.tm_sim.scroll_offset.min(max_scroll);
    }
    if state.games.ca_sim.open {
        let max_scroll = games_ca_sim_popup_max_scroll(state, screen, theme);
        state.games.ca_sim.scroll_offset = state.games.ca_sim.scroll_offset.min(max_scroll);
    }
    if state.games.match_history.open {
        let max_offset = games_match_history_max_offset(state, screen);
        state.games.match_history.column_offset =
            state.games.match_history.column_offset.min(max_offset);
        let max_rounds = games_match_history_max_rounds(state);
        let default_rounds = games_match_history_default_rounds(state);
        if let Some(limit) = state.games.match_history.round_limit {
            let clamped = limit.min(max_rounds);
            state.games.match_history.round_limit = if clamped == default_rounds {
                None
            } else {
                Some(clamped)
            };
        }
    }
}

pub(super) fn scroll_buffer(buf: &mut nit_core::Buffer, delta: i32) {
    let height = buf.viewport.height.max(1);
    let max_offset = buf.lines_len().saturating_sub(height);
    let offset = buf.viewport.offset_line as i32 + delta;
    let clamped = offset.clamp(0, max_offset as i32);
    buf.viewport.offset_line = clamped as usize;
}

pub(super) fn adjust_fuzzy_scroll(state: &mut AppState, list_height: usize) {
    let len = fuzzy_results_len(state);
    if len == 0 || list_height == 0 {
        state.fuzzy_search.selected = 0;
        state.fuzzy_search.scroll_offset = 0;
        return;
    }
    state.fuzzy_search.selected = state.fuzzy_search.selected.min(len - 1);
    let selected = state.fuzzy_search.selected;
    let mut scroll = state.fuzzy_search.scroll_offset.min(len.saturating_sub(1));
    if selected < scroll {
        scroll = selected;
    } else if selected >= scroll + list_height {
        scroll = selected.saturating_sub(list_height - 1);
    }
    let max_scroll = len.saturating_sub(list_height);
    state.fuzzy_search.scroll_offset = scroll.min(max_scroll);
}

pub(super) fn adjust_file_tree_scroll(state: &mut AppState, editor_area: ratatui::layout::Rect) {
    let inner_height = editor_area.height.saturating_sub(2) as usize;
    if inner_height == 0 {
        return;
    }
    let total = state.file_tree.rows.len();
    if total == 0 {
        state.file_tree.scroll_offset = 0;
        state.file_tree.selected = 0;
        return;
    }
    state.file_tree.selected = state.file_tree.selected.min(total - 1);
    let selected = state.file_tree.selected;
    if selected < state.file_tree.scroll_offset {
        state.file_tree.scroll_offset = selected;
    } else if selected >= state.file_tree.scroll_offset + inner_height {
        state.file_tree.scroll_offset = selected.saturating_sub(inner_height - 1);
    }
    let max_scroll = total.saturating_sub(inner_height);
    state.file_tree.scroll_offset = state.file_tree.scroll_offset.min(max_scroll);
}

pub(super) fn adjust_run_browser_scroll(state: &mut AppState, screen: ratatui::layout::Rect) {
    let area = dynamic_popup_rect(screen, games_run_browser_popup::preferred_size(screen));
    let inner_height = area.height.saturating_sub(2) as usize;
    if inner_height == 0 {
        return;
    }
    let selected = state.games.run_browser.selected;
    if selected < state.games.run_browser.scroll_offset {
        state.games.run_browser.scroll_offset = selected;
    } else if selected >= state.games.run_browser.scroll_offset + inner_height {
        state.games.run_browser.scroll_offset = selected.saturating_sub(inner_height - 1);
    }
}

pub(super) fn adjust_replay_scroll(state: &mut AppState, screen: ratatui::layout::Rect) {
    let area = dynamic_popup_rect(screen, games_replay_popup::preferred_size(screen));
    let inner_height = area.height.saturating_sub(2) as usize;
    if inner_height == 0 {
        return;
    }
    let selected = state.games.replay.selected_index;
    if selected < state.games.replay.scroll_offset {
        state.games.replay.scroll_offset = selected;
    } else if selected >= state.games.replay.scroll_offset + inner_height {
        state.games.replay.scroll_offset = selected.saturating_sub(inner_height - 1);
    }
}

pub(super) fn adjust_strategy_scroll(state: &mut AppState, screen: ratatui::layout::Rect) {
    let area = dynamic_popup_rect(screen, games_strategy_popup::preferred_size(screen));
    let inner_height = area.height.saturating_sub(2) as usize;
    if inner_height == 0 {
        return;
    }
    let selected = state.games.strategy_inspect.selected_index;
    if selected < state.games.strategy_inspect.scroll_offset {
        state.games.strategy_inspect.scroll_offset = selected;
    } else if selected >= state.games.strategy_inspect.scroll_offset + inner_height {
        state.games.strategy_inspect.scroll_offset = selected.saturating_sub(inner_height - 1);
    }
}

pub(super) fn adjust_global_archive_scroll(
    state: &mut AppState,
    screen: ratatui::layout::Rect,
    theme: &Theme,
) {
    let area = dynamic_popup_rect(screen, artifacts_history_popup::preferred_size(screen));
    let text_area = popup_text_area(area);
    let inner_height = text_area.height.max(1) as usize;
    let total = artifacts_history_popup::build_lines(state, theme, text_area.width).len();
    let max_scroll = total.saturating_sub(inner_height);
    // HEADER_LINES = 4 (search bar, status, blank, column headers)
    let selected_line = 4usize.saturating_add(state.agents.global_archive_selected);
    if selected_line < state.agents.global_archive_scroll {
        state.agents.global_archive_scroll = selected_line;
    } else if selected_line
        >= state
            .agents
            .global_archive_scroll
            .saturating_add(inner_height)
    {
        state.agents.global_archive_scroll =
            selected_line.saturating_sub(inner_height.saturating_sub(1));
    }
    state.agents.global_archive_scroll = state.agents.global_archive_scroll.min(max_scroll);
}
