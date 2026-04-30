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

pub(super) fn lines_to_strings(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

pub(super) fn update_ui_selection_text(
    state: &mut AppState,
    pane: UiSelectionPane,
    lines: &[String],
    clipboard: &mut Option<Clipboard>,
    input_state: &mut InputState,
) {
    let Some(selection) = state.ui_selection else {
        return;
    };
    if selection.pane != pane {
        return;
    }
    let signature = UiSelectionSignature {
        pane,
        start_line: selection.start_line,
        start_col: selection.start_col,
        end_line: selection.end_line,
        end_col: selection.end_col,
    };
    if input_state.last_ui_selection == Some(signature) {
        return;
    }
    input_state.last_ui_selection = Some(signature);
    let text = if matches!(pane, UiSelectionPane::AgentConsole) {
        selection_text_agent_console(lines, selection)
    } else {
        selection_text(lines, selection)
    };
    if text.is_empty() {
        return;
    }
    state.yank_kind = if text.contains('\n') {
        YankKind::Line
    } else {
        YankKind::Char
    };
    state.yank = Some(text.clone());
    if let Some(cb) = clipboard.as_mut() {
        let _ = cb.set_text(text);
    }
}

pub(crate) fn selection_text(lines: &[String], selection: UiSelection) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let (start_line, start_col, end_line, end_col) =
        if (selection.start_line, selection.start_col) <= (selection.end_line, selection.end_col) {
            (
                selection.start_line,
                selection.start_col,
                selection.end_line,
                selection.end_col,
            )
        } else {
            (
                selection.end_line,
                selection.end_col,
                selection.start_line,
                selection.start_col,
            )
        };
    let mut out = String::new();
    let last_line = lines.len().saturating_sub(1);
    let end_line = end_line.min(last_line);
    for (line_idx, line) in lines
        .iter()
        .enumerate()
        .take(end_line.saturating_add(1))
        .skip(start_line)
    {
        let line_len = line.chars().count();
        let sel_start = if line_idx == start_line { start_col } else { 0 };
        let sel_end = if line_idx == end_line {
            end_col
        } else {
            line_len
        };
        let sel_start = sel_start.min(line_len);
        let sel_end = sel_end.min(line_len);
        out.push_str(&slice_by_char(line, sel_start, sel_end));
        if line_idx < end_line {
            out.push('\n');
        }
    }
    out
}

pub(super) fn selection_text_agent_console(lines: &[String], selection: UiSelection) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let (start_line, start_col, end_line, end_col) =
        if (selection.start_line, selection.start_col) <= (selection.end_line, selection.end_col) {
            (
                selection.start_line,
                selection.start_col,
                selection.end_line,
                selection.end_col,
            )
        } else {
            (
                selection.end_line,
                selection.end_col,
                selection.start_line,
                selection.start_col,
            )
        };
    let last_line = lines.len().saturating_sub(1);
    let end_line = end_line.min(last_line);
    let mut out_lines = Vec::new();
    for line_idx in start_line..=end_line {
        let line = &lines[line_idx];
        let line_len = line.chars().count();
        let mut sel_start = if line_idx == start_line { start_col } else { 0 };
        let mut sel_end = if line_idx == end_line {
            end_col
        } else {
            line_len
        };
        sel_start = sel_start.min(line_len);
        sel_end = sel_end.min(line_len);
        let slice = if let Some((payload_start, payload_end)) =
            user_prompt_payload_bounds_in_block(lines, line_idx)
        {
            let sel_start = sel_start.max(payload_start);
            let sel_end = sel_end.min(payload_end);
            slice_by_char(line, sel_start, sel_end)
                .trim_end_matches(' ')
                .to_string()
        } else {
            slice_by_char(line, sel_start, sel_end)
        };
        out_lines.push(slice);
    }
    out_lines.join("\n")
}

pub(super) const USER_PROMPT_INDENT: usize = 2;

pub(super) fn is_user_prompt_row(line: &str) -> bool {
    // User prompts are padded out to the full thread width so the background fills the row.
    // That makes them easy to detect for clipboard trimming: they start with the fixed indent and
    // end with spaces.
    line.starts_with("  ") && line.ends_with(' ')
}

pub(super) fn user_prompt_payload_bounds_in_block(
    lines: &[String],
    idx: usize,
) -> Option<(usize, usize)> {
    let line = lines.get(idx)?;
    if !is_user_prompt_row(line) {
        return None;
    }

    // Find the contiguous block of padded user rows that this line belongs to.
    let mut start = idx;
    while start > 0 && is_user_prompt_row(&lines[start - 1]) {
        start = start.saturating_sub(1);
    }
    let mut end = idx;
    while end + 1 < lines.len() && is_user_prompt_row(&lines[end + 1]) {
        end = end.saturating_add(1);
    }

    // Only treat the block as a user prompt if it contains the "You" label line.
    let has_label = (start..=end).any(|line_idx| lines[line_idx].trim() == "You");
    if !has_label {
        return None;
    }

    let len = line.chars().count();
    let start_col = USER_PROMPT_INDENT.min(len);
    Some((start_col, len))
}

pub(super) fn reset_ui_selection(state: &mut AppState, input_state: &mut InputState) {
    input_state.mouse_select_anchor = None;
    state.ui_selection = None;
    input_state.last_ui_selection = None;
}

pub(super) fn adjust_agent_console_drag_col(
    lines: &[String],
    anchor_line: usize,
    line_idx: usize,
    col: usize,
) -> usize {
    let Some(line) = lines.get(line_idx) else {
        return col;
    };
    let Some(payload_start) = user_bubble_payload_start_col(line) else {
        return col;
    };
    if col > payload_start {
        return col;
    }
    if line_idx > anchor_line {
        line.chars().count()
    } else if line_idx < anchor_line {
        0
    } else {
        col
    }
}

pub(super) fn user_bubble_payload_start_col(line: &str) -> Option<usize> {
    is_user_prompt_row(line).then_some(USER_PROMPT_INDENT)
}

pub(super) fn mouse_drag_allowed(state: &AppState, anchor: MouseSelectAnchor) -> bool {
    if state.command_line.is_some() || state.prompt.is_some() {
        return false;
    }
    if state.rule_picker.open || state.protocol_picker.open {
        return false;
    }
    if state.agents.artifacts_popup_open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::ArtifactsPopup)
                | MouseSelectTarget::PopupChatInput
        );
    }
    if state.agents.global_archive_open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::ArtifactsHistoryPopup)
        );
    }
    if state.show_help {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::HelpPopup)
        );
    }
    if state.app_kind == AppKind::Games && state.games.analysis.open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesAnalysisPopup)
        );
    }
    if state.app_kind == AppKind::Games && state.games.run_browser.open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesRunBrowserPopup)
        );
    }
    if state.app_kind == AppKind::Games && state.games.replay.open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesReplayPopup)
        );
    }
    if state.app_kind == AppKind::Games && state.games.strategy_inspect.open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesStrategyPopup)
        );
    }
    if state.app_kind == AppKind::Games && state.games.tm_sim.open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesTmSimPopupLeft)
                | MouseSelectTarget::Ui(UiSelectionPane::GamesTmSimPopupRight)
        );
    }
    if state.app_kind == AppKind::Games && state.games.ca_sim.open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesCaSimPopupLeft)
                | MouseSelectTarget::Ui(UiSelectionPane::GamesCaSimPopupRight)
        );
    }
    if state.app_kind == AppKind::Games && state.games.match_history.open {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesMatchHistoryPopup)
        );
    }
    if games_petri_visible(state) {
        return matches!(
            anchor.target,
            MouseSelectTarget::Ui(UiSelectionPane::GamesPetriDish)
        );
    }
    true
}
