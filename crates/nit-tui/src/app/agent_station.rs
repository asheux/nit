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
pub(super) fn handle_agent_station_key_with_clipboard(
    key: KeyEvent,
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    swarm: &mut SwarmRuntime,
    shadow: &mut crate::shadow::ShadowRuntime,
    clipboard: &mut Option<Clipboard>,
) -> bool {
    if let Some(target) = map_focus_hotkey(&key) {
        state.focus = target;
        if target == PaneId::JobOutput && state.agents.dock_tab == AgentOpsTab::Scratchpad {
            state.mode = Mode::Insert;
        } else if target != PaneId::Editor {
            state.mode = Mode::Normal;
        }
        return true;
    }
    if state.command_line.is_some()
        || state.prompt.is_some()
        || state.show_help
        || state.rule_picker.open
        || state.protocol_picker.open
        || state.fuzzy_search.open
        || games_modal_popup_open(state)
    {
        return false;
    }

    match state.focus {
        PaneId::JobOutput => handle_agent_ops_key(key, state, vitals, codex, claude, swarm),
        PaneId::Notes => {
            handle_agent_console_key(key, state, vitals, codex, claude, swarm, shadow, clipboard)
        }
        _ => false,
    }
}

pub(super) fn handle_agent_ops_key(
    key: KeyEvent,
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    _claude: Option<&ClaudeRunner>,
    swarm: &SwarmRuntime,
) -> bool {
    if state.agents.dock_tab == AgentOpsTab::Scratchpad {
        match key {
            KeyEvent {
                code: KeyCode::Tab,
                modifiers: KeyModifiers::SHIFT,
                ..
            } => {
                state.agents.dock_tab = state.agents.dock_tab.prev();
                state.agents.roster_tree_selected = None;
                state.agents.ops_scroll = 0;
                state.mode = if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                    Mode::Insert
                } else {
                    Mode::Normal
                };
                state.agents.note_event();
                vitals.record_agent_event(Instant::now());
                return true;
            }
            KeyEvent {
                code: KeyCode::Tab, ..
            } => {
                state.agents.dock_tab = state.agents.dock_tab.next();
                state.agents.roster_tree_selected = None;
                state.agents.ops_scroll = 0;
                state.mode = if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                    Mode::Insert
                } else {
                    Mode::Normal
                };
                state.agents.note_event();
                vitals.record_agent_event(Instant::now());
                return true;
            }
            KeyEvent {
                code: KeyCode::Left,
                ..
            } if state.mode != Mode::Insert => {
                state.agents.dock_tab = state.agents.dock_tab.prev();
                state.agents.roster_tree_selected = None;
                state.agents.ops_scroll = 0;
                state.mode = if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                    Mode::Insert
                } else {
                    Mode::Normal
                };
                state.agents.note_event();
                vitals.record_agent_event(Instant::now());
                return true;
            }
            KeyEvent {
                code: KeyCode::Right,
                ..
            } if state.mode != Mode::Insert => {
                state.agents.dock_tab = state.agents.dock_tab.next();
                state.agents.roster_tree_selected = None;
                state.agents.ops_scroll = 0;
                state.mode = if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                    Mode::Insert
                } else {
                    Mode::Normal
                };
                state.agents.note_event();
                vitals.record_agent_event(Instant::now());
                return true;
            }
            KeyEvent {
                code: KeyCode::Char(_),
                modifiers,
                ..
            } if state.mode != Mode::Insert
                && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
            {
                // In Scratchpad, treat first printable key as intent to type.
                state.mode = Mode::Insert;
                return false;
            }
            KeyEvent {
                code: KeyCode::Enter | KeyCode::Backspace | KeyCode::Delete,
                modifiers,
                ..
            } if state.mode != Mode::Insert && modifiers.is_empty() => {
                state.mode = Mode::Insert;
                return false;
            }
            KeyEvent {
                code: KeyCode::Esc, ..
            } if state.mode == Mode::Insert => {
                state.mode = Mode::Normal;
                return true;
            }
            _ => return false,
        }
    }

    let mut changed = false;
    match key {
        KeyEvent {
            code: KeyCode::Char('l'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if let Some(agent_ops_view::RosterSelectableRow::Backend { backend }) =
                agent_ops_view::roster_selected_row(state)
            {
                select_roster_backend(state, backend);
                changed = if !state
                    .agents
                    .roster_expanded_backend_kinds
                    .contains(&backend)
                {
                    toggle_roster_backend_expanded(state, backend)
                } else {
                    false
                };
            } else {
                changed = enter_roster_tree_cursor(state);
            }
        }
        KeyEvent {
            code: KeyCode::Char('h'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if let Some(agent_ops_view::RosterSelectableRow::Backend { backend }) =
                agent_ops_view::roster_selected_row(state)
            {
                select_roster_backend(state, backend);
                changed = if state
                    .agents
                    .roster_expanded_backend_kinds
                    .contains(&backend)
                {
                    toggle_roster_backend_expanded(state, backend)
                } else {
                    false
                };
            } else {
                changed = exit_roster_tree_cursor(state);
            }
        }
        KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            changed = reset_roster_context(state, swarm);
        }
        KeyEvent {
            code: KeyCode::Char('1'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if !state
                .agents
                .swarm_default_template
                .eq_ignore_ascii_case("lab")
            {
                state.agents.swarm_default_template = "lab".into();
                state.agents.roster_tree_selected = None;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('2'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if !state
                .agents
                .swarm_default_template
                .eq_ignore_ascii_case("parallel")
            {
                state.agents.swarm_default_template = "parallel".into();
                state.agents.roster_tree_selected = None;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('3'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if !state
                .agents
                .swarm_default_template
                .eq_ignore_ascii_case("bulk")
            {
                state.agents.swarm_default_template = "bulk".into();
                state.agents.roster_tree_selected = None;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('4'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if !state
                .agents
                .swarm_default_mission
                .eq_ignore_ascii_case("auto")
            {
                state.agents.swarm_default_mission = "auto".into();
                state.agents.roster_tree_selected = None;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('5'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if !state
                .agents
                .swarm_default_mission
                .eq_ignore_ascii_case("general")
            {
                state.agents.swarm_default_mission = "general".into();
                state.agents.roster_tree_selected = None;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('6'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if !state
                .agents
                .swarm_default_mission
                .eq_ignore_ascii_case("research")
            {
                state.agents.swarm_default_mission = "research".into();
                state.agents.roster_tree_selected = None;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('7'),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if !state
                .agents
                .swarm_default_mission
                .eq_ignore_ascii_case("computational-research")
            {
                state.agents.swarm_default_mission = "computational-research".into();
                state.agents.roster_tree_selected = None;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::SHIFT,
            ..
        } => {
            state.agents.dock_tab = state.agents.dock_tab.prev();
            state.agents.roster_tree_selected = None;
            state.agents.ops_scroll = 0;
            if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                state.mode = Mode::Insert;
            }
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Tab, ..
        } => {
            state.agents.dock_tab = state.agents.dock_tab.next();
            state.agents.roster_tree_selected = None;
            state.agents.ops_scroll = 0;
            if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                state.mode = Mode::Insert;
            }
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Left,
            ..
        } => {
            state.agents.dock_tab = state.agents.dock_tab.prev();
            state.agents.roster_tree_selected = None;
            state.agents.ops_scroll = 0;
            if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                state.mode = Mode::Insert;
            }
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Right,
            ..
        } => {
            state.agents.dock_tab = state.agents.dock_tab.next();
            state.agents.roster_tree_selected = None;
            state.agents.ops_scroll = 0;
            if state.agents.dock_tab == AgentOpsTab::Scratchpad {
                state.mode = Mode::Insert;
            }
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Up, ..
        }
        | KeyEvent {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            changed = move_agent_ops_selection(state, swarm, -1);
        }
        KeyEvent {
            code: KeyCode::Down,
            ..
        }
        | KeyEvent {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            changed = move_agent_ops_selection(state, swarm, 1);
        }
        KeyEvent {
            code: KeyCode::Char(' '),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster
            && state.agents.roster_tree_selected.is_none() =>
        {
            changed = toggle_roster_priority(state);
        }
        KeyEvent {
            code: KeyCode::Char(' '),
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster
            && state.agents.roster_tree_selected.is_some() =>
        {
            changed = select_roster_tree_leaf(state);
        }
        KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster
            && state.agents.roster_tree_selected.is_some() =>
        {
            changed = select_roster_tree_leaf(state);
        }
        KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Roster => {
            if let Some(agent_ops_view::RosterSelectableRow::Backend { backend }) =
                agent_ops_view::roster_selected_row(state)
            {
                select_roster_backend(state, backend);
                changed = toggle_roster_backend_expanded(state, backend);
            } else {
                state.focus = PaneId::Notes;
                state.mode = Mode::Normal;
                state.agents.console_scroll = CONSOLE_SCROLL_BOTTOM;
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            ..
        } if state.agents.dock_tab == AgentOpsTab::Evidence => {
            // Check if the selected card is a PROMPT — toggle collapse.
            let selected_card_kind = {
                let text_width = state.agents.ops_viewport_width.max(32);
                let widths = agent_ops_view::artifact_list_widths(text_width);
                let preview_chars = widths
                    .get(3)
                    .copied()
                    .unwrap_or(120)
                    .saturating_sub(1)
                    .max(10);
                let cards =
                    agent_ops_view::artifact_cards_for_context(state, Some(swarm), preview_chars);
                let sel = state
                    .agents
                    .artifacts_selected
                    .min(cards.len().saturating_sub(1));
                cards.get(sel).map(|c| c.kind.to_string())
            };
            if selected_card_kind.as_deref() == Some("PROMPT") {
                let idx = state.agents.artifacts_selected;
                if !state.agents.artifacts_collapsed_prompts.remove(&idx) {
                    state.agents.artifacts_collapsed_prompts.insert(idx);
                }
            } else {
                state.agents.artifacts_popup_open = true;
                state.agents.artifacts_popup_scroll = 0;
            }
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Char('r') | KeyCode::Char('R'),
            modifiers,
            ..
        } if modifiers.is_empty() && state.agents.dock_tab == AgentOpsTab::Evidence => {
            state.agents.global_archive_open = true;
            state.agents.global_archive_query.clear();
            state.agents.global_archive_query_cursor = 0;
            state.agents.global_archive_selected = 0;
            state.agents.global_archive_scroll = 0;
            state.agents.global_archive_filter = SavedRunHistoryFilter::All;
            state.agents.global_archive_index = agent_ops_view::build_global_archive_index(state);
            state.agents.global_archive_filtered = agent_ops_view::filter_global_archive(
                &state.agents.global_archive_index,
                "",
                SavedRunHistoryFilter::All,
            );
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Enter,
            ..
        } => {
            state.focus = PaneId::Notes;
            state.mode = Mode::Normal;
            state.agents.console_scroll = CONSOLE_SCROLL_BOTTOM;
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Char('n'),
            modifiers,
            ..
        } if modifiers.is_empty() => {
            spawn_mock_mission(state);
            changed = true;
        }
        KeyEvent {
            code: KeyCode::Char('r'),
            modifiers,
            ..
        } if modifiers.is_empty() && state.agents.dock_tab == AgentOpsTab::Mcp => {
            if let Some(codex) = codex {
                state.status =
                    Some("MCP reconnect: cancelling in-flight turns (context preserved)".into());
                codex.send(CodexCommand::McpReconnect);
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('s'),
            modifiers,
            ..
        } if modifiers.is_empty() && state.agents.dock_tab == AgentOpsTab::Mcp => {
            if let Some(codex) = codex {
                codex.send(CodexCommand::McpStart);
                changed = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('x'),
            modifiers,
            ..
        } if modifiers.is_empty() && state.agents.dock_tab == AgentOpsTab::Mcp => {
            if let Some(codex) = codex {
                reset_codex_mcp_sessions(state, "MCP stop clears Codex thread context");
                codex.send(CodexCommand::McpStop);
                changed = true;
            }
        }
        _ => {}
    }
    if changed {
        state.agents.note_event();
        vitals.record_agent_event(Instant::now());
    }
    changed
}

pub(super) fn reset_codex_mcp_sessions(state: &mut AppState, status: &str) {
    state.agents.codex_thread_ids.clear();
    state.agents.codex_mission_thread_ids.clear();
    state.agents.codex_used_tokens.clear();
    state.agents.codex_mission_used_tokens.clear();
    state.agents.codex_context_remaining_pct.clear();
    state.agents.codex_mission_context_remaining_pct.clear();
    state.agents.codex_estimated_tokens_used_by_mission.clear();
    state.status = Some(status.to_string());
}

pub(super) fn sync_roster_selected_agent(state: &mut AppState, agent_idx: usize) {
    state.agents.roster_selected = agent_idx;
    state.agents.roster_selected_backend = None;
    state.agents.roster_tree_selected = None;
    if let Some(agent) = state.agents.agents.get(agent_idx) {
        state.agents.selected_agent = Some(agent.id.clone());
        if let Some(mission_id) = agent.current_mission.as_deref() {
            state.agents.selected_mission = Some(mission_id.to_string());
            if let Some(idx) = state
                .agents
                .missions
                .iter()
                .position(|mission| mission.id == mission_id)
            {
                state.agents.mission_selected = idx;
            }
        }
    }
}

pub(super) fn select_roster_backend(state: &mut AppState, backend: nit_core::AgentLaneKind) {
    state.agents.roster_selected_backend = Some(backend);
    state.agents.roster_tree_selected = None;
}

pub(super) fn toggle_roster_backend_expanded(
    state: &mut AppState,
    backend: nit_core::AgentLaneKind,
) -> bool {
    let keep_backend_selected = state.agents.roster_selected_backend == Some(backend);
    if state.agents.roster_expanded_backend_kinds.remove(&backend) {
        if keep_backend_selected {
            state.agents.roster_tree_selected = None;
            return true;
        }
        let visible = agent_ops_view::roster_agent_display_order(state);
        if !visible.contains(&state.agents.roster_selected) {
            state.agents.roster_tree_selected = None;
            if let Some(&first_visible) = visible.first() {
                sync_roster_selected_agent(state, first_visible);
            }
        }
        return true;
    }

    if !state.agents.roster_expanded_backend_kinds.insert(backend) {
        return false;
    }
    if keep_backend_selected {
        state.agents.roster_tree_selected = None;
        return true;
    }
    if let Some(first_agent_idx) =
        agent_ops_view::roster_first_agent_idx_for_backend(state, backend)
    {
        sync_roster_selected_agent(state, first_agent_idx);
    }
    true
}

pub(super) fn roster_selected_agent_is_visible(state: &AppState) -> bool {
    agent_ops_view::roster_agent_display_order(state).contains(&state.agents.roster_selected)
}

pub(super) fn move_roster_primary_selection(state: &mut AppState, delta: i32) -> bool {
    let order = agent_ops_view::roster_selection_rows(state);
    if order.is_empty() {
        return false;
    }

    let current = agent_ops_view::roster_selected_row(state).unwrap_or(order[0]);
    let cur_pos = order.iter().position(|row| *row == current).unwrap_or(0);
    let next_pos = (cur_pos as i32 + delta).clamp(0, order.len().saturating_sub(1) as i32) as usize;
    if next_pos == cur_pos {
        return false;
    }

    match order[next_pos] {
        agent_ops_view::RosterSelectableRow::Backend { backend } => {
            select_roster_backend(state, backend);
        }
        agent_ops_view::RosterSelectableRow::Agent { agent_idx } => {
            sync_roster_selected_agent(state, agent_idx);
        }
    }
    true
}

pub(super) fn move_agent_ops_selection(
    state: &mut AppState,
    swarm: &SwarmRuntime,
    delta: i32,
) -> bool {
    match state.agents.dock_tab {
        AgentOpsTab::Roster => {
            if state.agents.agents.is_empty() {
                return false;
            }
            if let Some(sel) = state.agents.roster_tree_selected {
                let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
                    state.agents.roster_tree_selected = None;
                    return true;
                };
                let show_roles = state
                    .agents
                    .swarm_default_template
                    .eq_ignore_ascii_case("bulk")
                    || state
                        .agents
                        .swarm_default_template
                        .eq_ignore_ascii_case("parallel");
                let efforts = state
                    .agents
                    .codex_supported_reasoning_efforts
                    .get(&agent.id)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let size_len = efforts.len();
                let has_roles = show_roles && agent.is_codex();
                let roles_len = if has_roles { 8usize } else { 0usize };

                match sel.branch {
                    nit_core::RosterTreeBranch::Size => {
                        if size_len == 0 {
                            state.agents.roster_tree_selected = None;
                            return true;
                        }
                        let max = size_len.saturating_sub(1);
                        if delta.is_negative() {
                            if sel.leaf_idx > 0 {
                                state.agents.roster_tree_selected =
                                    Some(nit_core::RosterTreeSelection {
                                        branch: nit_core::RosterTreeBranch::Size,
                                        leaf_idx: sel.leaf_idx.saturating_sub(1),
                                    });
                                return true;
                            }
                            state.agents.roster_tree_selected = None;
                            return true;
                        }
                        if delta > 0 {
                            if sel.leaf_idx < max {
                                state.agents.roster_tree_selected =
                                    Some(nit_core::RosterTreeSelection {
                                        branch: nit_core::RosterTreeBranch::Size,
                                        leaf_idx: (sel.leaf_idx + 1).min(max),
                                    });
                                return true;
                            }

                            if roles_len > 0 {
                                state.agents.roster_tree_selected =
                                    Some(nit_core::RosterTreeSelection {
                                        branch: nit_core::RosterTreeBranch::Role,
                                        leaf_idx: 0,
                                    });
                                return true;
                            }
                        }
                    }
                    nit_core::RosterTreeBranch::Role => {
                        if roles_len == 0 {
                            state.agents.roster_tree_selected = None;
                            return true;
                        }
                        let max = roles_len.saturating_sub(1);
                        if delta.is_negative() {
                            if sel.leaf_idx > 0 {
                                state.agents.roster_tree_selected =
                                    Some(nit_core::RosterTreeSelection {
                                        branch: nit_core::RosterTreeBranch::Role,
                                        leaf_idx: sel.leaf_idx.saturating_sub(1),
                                    });
                                return true;
                            }
                            if size_len > 0 {
                                state.agents.roster_tree_selected =
                                    Some(nit_core::RosterTreeSelection {
                                        branch: nit_core::RosterTreeBranch::Size,
                                        leaf_idx: size_len.saturating_sub(1),
                                    });
                                return true;
                            }
                            state.agents.roster_tree_selected = None;
                            return true;
                        }
                        if delta > 0 && sel.leaf_idx < max {
                            state.agents.roster_tree_selected =
                                Some(nit_core::RosterTreeSelection {
                                    branch: nit_core::RosterTreeBranch::Role,
                                    leaf_idx: (sel.leaf_idx + 1).min(max),
                                });
                            return true;
                        }
                    }
                }

                // Walk out of the tree when we hit the end and press Down.
                if delta > 0 {
                    state.agents.roster_tree_selected = None;
                    return move_roster_primary_selection(state, 1);
                }
                return false;
            }

            move_roster_primary_selection(state, delta)
        }
        AgentOpsTab::Missions => {
            if state.agents.missions.is_empty() {
                return false;
            }
            let max = state.agents.missions.len().saturating_sub(1) as i32;
            let next = (state.agents.mission_selected as i32 + delta).clamp(0, max) as usize;
            if next == state.agents.mission_selected {
                return false;
            }
            state.agents.mission_selected = next;
            if let Some(mission) = state.agents.missions.get(next) {
                state.agents.selected_mission = Some(mission.id.clone());
            }
            true
        }
        AgentOpsTab::Evidence => {
            let text_width = state.agents.ops_viewport_width.max(32).max(1);
            let lines =
                agent_ops_view::current_lines_for_width_with_swarm(state, Some(swarm), text_width);
            let count = agent_ops_view::artifacts_card_count(&lines);
            if count == 0 {
                return false;
            }
            let max = count.saturating_sub(1) as i32;
            let next = (state.agents.artifacts_selected as i32 + delta).clamp(0, max) as usize;
            if next == state.agents.artifacts_selected {
                return false;
            }
            state.agents.artifacts_selected = next;

            if let Some(line_idx) = agent_ops_view::artifacts_card_line_for_index(&lines, next) {
                let height = state.agents.ops_viewport_height.max(1);
                if line_idx < state.agents.ops_scroll {
                    state.agents.ops_scroll = line_idx;
                } else if line_idx >= state.agents.ops_scroll.saturating_add(height) {
                    state.agents.ops_scroll = line_idx.saturating_sub(height.saturating_sub(1));
                }
            }
            true
        }
        AgentOpsTab::Alerts => {
            if state.agents.alerts.is_empty() {
                return false;
            }
            let max = state.agents.alerts.len().saturating_sub(1) as i32;
            let next = (state.agents.alert_selected as i32 + delta).clamp(0, max) as usize;
            if next == state.agents.alert_selected {
                return false;
            }
            state.agents.alert_selected = next;
            true
        }
        AgentOpsTab::Patch
        | AgentOpsTab::Diagnostics
        | AgentOpsTab::Dag
        | AgentOpsTab::Scratchpad => {
            let text_width = state.agents.ops_viewport_width.max(1);
            let lines =
                agent_ops_view::current_lines_for_width_with_swarm(state, Some(swarm), text_width);
            let height = state.agents.ops_viewport_height.max(1);
            let max_scroll = lines.len().saturating_sub(height);
            bump_scroll_clamped(&mut state.agents.ops_scroll, delta, max_scroll);
            true
        }
        AgentOpsTab::Mcp => false,
    }
}

pub(super) const SWARM_ROLE_OPTIONS: [&str; 8] = [
    "all",
    "propose",
    "research",
    "computational-research",
    "judge",
    "integrate",
    "review",
    "test",
];

pub(super) fn normalize_swarm_role_hint_for_roster(raw: &str) -> String {
    let role = raw.trim();
    if role.eq_ignore_ascii_case("all") {
        return "all".into();
    }
    normalize_role_label(role).unwrap_or_else(|| role.to_ascii_lowercase())
}

pub(super) fn enter_roster_tree_cursor(state: &mut AppState) -> bool {
    if state.agents.roster_tree_selected.is_some() {
        return false;
    }
    if !roster_selected_agent_is_visible(state) {
        return false;
    }
    let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
        state.agents.roster_tree_selected = None;
        return false;
    };

    let show_roles = state
        .agents
        .swarm_default_template
        .eq_ignore_ascii_case("bulk")
        || state
            .agents
            .swarm_default_template
            .eq_ignore_ascii_case("parallel");

    let efforts = state
        .agents
        .codex_supported_reasoning_efforts
        .get(&agent.id)
        .or_else(|| state.agents.claude_supported_efforts.get(&agent.id))
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let has_size = !efforts.is_empty();
    let has_roles = show_roles && (agent.is_codex() || agent.is_claude());
    if !has_size && !has_roles {
        state.agents.roster_tree_selected = None;
        return false;
    }
    state
        .agents
        .roster_tree_collapsed_agent_ids
        .remove(&agent.id);
    if has_size {
        let current = state
            .agents
            .codex_selected_reasoning_effort
            .get(&agent.id)
            .or_else(|| state.agents.codex_default_reasoning_effort.get(&agent.id))
            .or_else(|| state.agents.claude_selected_effort.get(&agent.id))
            .or_else(|| state.agents.claude_default_effort.get(&agent.id))
            .map(|s| s.as_str());
        let idx = current
            .and_then(|effort| efforts.iter().position(|e| e == effort))
            .unwrap_or(0)
            .min(efforts.len().saturating_sub(1));
        state.agents.roster_tree_selected = Some(nit_core::RosterTreeSelection {
            branch: nit_core::RosterTreeBranch::Size,
            leaf_idx: idx,
        });
        return true;
    }

    if has_roles {
        let current = state
            .agents
            .swarm_role_by_agent_id
            .get(&agent.id)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        let idx = current
            .and_then(|role| {
                let current = normalize_swarm_role_hint_for_roster(role);
                SWARM_ROLE_OPTIONS.iter().position(|candidate| {
                    current == normalize_swarm_role_hint_for_roster(candidate)
                })
            })
            .unwrap_or(0)
            .min(SWARM_ROLE_OPTIONS.len().saturating_sub(1));
        state.agents.roster_tree_selected = Some(nit_core::RosterTreeSelection {
            branch: nit_core::RosterTreeBranch::Role,
            leaf_idx: idx,
        });
        return true;
    }

    state.agents.roster_tree_selected = None;
    false
}

pub(super) fn exit_roster_tree_cursor(state: &mut AppState) -> bool {
    if state.agents.roster_tree_selected.is_some() {
        state.agents.roster_tree_selected = None;
        return true;
    }
    if !roster_selected_agent_is_visible(state) {
        return false;
    }
    let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
        return false;
    };
    if state
        .agents
        .roster_tree_collapsed_agent_ids
        .insert(agent.id.clone())
    {
        return true;
    }
    false
}

pub(super) fn select_roster_tree_leaf(state: &mut AppState) -> bool {
    let Some(sel) = state.agents.roster_tree_selected else {
        return false;
    };
    let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
        return false;
    };

    match sel.branch {
        nit_core::RosterTreeBranch::Size => {
            let efforts = state
                .agents
                .codex_supported_reasoning_efforts
                .get(&agent.id)
                .or_else(|| state.agents.claude_supported_efforts.get(&agent.id));
            let Some(efforts) = efforts else {
                return false;
            };
            let Some(effort) = efforts.get(sel.leaf_idx) else {
                return false;
            };

            let effort = effort.trim();
            if effort.is_empty() {
                return false;
            }

            let is_claude = agent.is_claude();
            if is_claude {
                let current = state
                    .agents
                    .claude_selected_effort
                    .get(&agent.id)
                    .map(|s| s.as_str());
                if current == Some(effort) {
                    return false;
                }
                state
                    .agents
                    .claude_selected_effort
                    .insert(agent.id.clone(), effort.to_string());
            } else {
                let current = state
                    .agents
                    .codex_selected_reasoning_effort
                    .get(&agent.id)
                    .map(|s| s.as_str());
                if current == Some(effort) {
                    return false;
                }
                state
                    .agents
                    .codex_selected_reasoning_effort
                    .insert(agent.id.clone(), effort.to_string());
            }
            true
        }
        nit_core::RosterTreeBranch::Role => {
            let Some(role) = SWARM_ROLE_OPTIONS.get(sel.leaf_idx).copied() else {
                return false;
            };

            if role.eq_ignore_ascii_case("all") {
                let current = state
                    .agents
                    .swarm_role_by_agent_id
                    .get(&agent.id)
                    .map(|s| s.as_str());
                if current.is_some_and(|cur| cur.trim().eq_ignore_ascii_case("all")) {
                    return false;
                }
                state
                    .agents
                    .swarm_role_by_agent_id
                    .insert(agent.id.clone(), "all".to_string());
                return true;
            }

            let current = state
                .agents
                .swarm_role_by_agent_id
                .get(&agent.id)
                .map(|s| s.as_str());
            if current.is_some_and(|cur| {
                normalize_swarm_role_hint_for_roster(cur)
                    == normalize_swarm_role_hint_for_roster(role)
            }) {
                return false;
            }
            state
                .agents
                .swarm_role_by_agent_id
                .insert(agent.id.clone(), role.to_string());
            true
        }
    }
}

pub(super) fn toggle_roster_priority(state: &mut AppState) -> bool {
    if !roster_selected_agent_is_visible(state) {
        return false;
    }
    let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
        return false;
    };
    if !agent.supports_swarm_priority() {
        return false;
    }
    if agent.id.contains("#swarm-") {
        return false;
    }
    if state.agents.swarm_priority_agent_ids.remove(&agent.id) {
        return true;
    }
    state
        .agents
        .swarm_priority_agent_ids
        .insert(agent.id.clone())
}

pub(super) fn reset_roster_context(state: &mut AppState, swarm: &SwarmRuntime) -> bool {
    let Some(agent) = state.agents.agents.get(state.agents.roster_selected) else {
        return false;
    };
    let agent_id = agent.id.clone();
    let agent_label = agent.role.trim().to_string();
    let is_codex = agent.is_codex();
    let mission_ctx = state
        .agents
        .selected_context_mission()
        .map(ToString::to_string);

    state.agents.roster_tree_selected = None;
    // Clear any in-flight liveness tracking for this agent context.
    state.agents.active_turns.remove(&agent_id);
    if is_codex {
        // Reset back to "full context" for display purposes.
        state
            .agents
            .codex_context_remaining_pct
            .insert(agent_id.clone(), 100);
    } else {
        state.agents.codex_context_remaining_pct.remove(&agent_id);
    }

    let before = state.agents.messages.len();
    if let Some(mission_id) = mission_ctx.as_deref() {
        if let Err(err) = write_agent_run_provenance(state, swarm, mission_id) {
            state.agents.diag_events.push(AgentDiagnosticEvent {
                severity: AgentAlertSeverity::Warn,
                source: "artifacts".into(),
                message: format!(
                    "failed to persist mission artifacts before reset for {mission_id}: {err}"
                ),
                at: timestamp_label(state),
            });
        } else {
            let run_dir = state
                .workspace_root
                .join(".nit")
                .join("agents")
                .join("runs")
                .join(mission_id);
            if let Err(err) = archive_saved_run_snapshot(&run_dir) {
                state.agents.diag_events.push(AgentDiagnosticEvent {
                    severity: AgentAlertSeverity::Warn,
                    source: "artifacts".into(),
                    message: format!("failed to archive saved mission run for {mission_id}: {err}"),
                    at: timestamp_label(state),
                });
            }
            state
                .agents
                .pending_provenance_mission_ids
                .retain(|id| id != mission_id);
        }
        // In mission context, the Codex session is shared by mission id. Resetting context should
        // clear the mission transcript and forget the session id so the next prompt starts fresh.
        state.agents.codex_mission_thread_ids.remove(mission_id);
        state
            .agents
            .codex_mission_context_remaining_pct
            .remove(mission_id);
        state.agents.codex_mission_used_tokens.remove(mission_id);
        state
            .agents
            .codex_estimated_tokens_used_by_mission
            .remove(mission_id);
        state
            .agents
            .messages
            .retain(|msg| msg.mission_id.as_deref() != Some(mission_id));
    } else {
        if let Err(err) = write_ad_hoc_run_provenance(state, &agent_id) {
            state.agents.diag_events.push(AgentDiagnosticEvent {
                severity: AgentAlertSeverity::Warn,
                source: "artifacts".into(),
                message: format!(
                    "failed to persist ad-hoc artifacts before reset for {agent_id}: {err}"
                ),
                at: timestamp_label(state),
            });
        } else {
            let run_dir = state
                .workspace_root
                .join(".nit")
                .join("agents")
                .join("ad-hoc")
                .join(sanitize_for_filename(&agent_id));
            if let Err(err) = archive_saved_run_snapshot(&run_dir) {
                state.agents.diag_events.push(AgentDiagnosticEvent {
                    severity: AgentAlertSeverity::Warn,
                    source: "artifacts".into(),
                    message: format!("failed to archive saved ad-hoc run for {agent_id}: {err}"),
                    at: timestamp_label(state),
                });
            }
            state
                .agents
                .pending_provenance_agent_ids
                .retain(|id| id != &agent_id);
        }
        // In non-mission chat, the thread isn't partitioned by agent; reset the whole local thread.
        state.agents.codex_thread_ids.clear();
        state.agents.codex_used_tokens.clear();
        state.agents.messages.retain(|msg| msg.mission_id.is_some());
    }

    // If there are queued Codex turns for the context we're resetting, drop them (they would run
    // against a now-forgotten thread id). Keep each agent's `queue_len` consistent with removals.
    let mut removed_by_agent: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    if let Some(mission_id) = mission_ctx.as_deref() {
        state.agents.queued_codex_turns.retain(|turn| {
            if turn.mission_id.as_deref() == Some(mission_id) {
                *removed_by_agent.entry(turn.agent_id.clone()).or_insert(0) += 1;
                false
            } else {
                true
            }
        });
    } else {
        state.agents.queued_codex_turns.retain(|turn| {
            if turn.mission_id.is_none() {
                *removed_by_agent.entry(turn.agent_id.clone()).or_insert(0) += 1;
                false
            } else {
                true
            }
        });
    }
    if !removed_by_agent.is_empty() {
        for agent in state.agents.agents.iter_mut() {
            let Some(removed) = removed_by_agent.get(&agent.id).copied() else {
                continue;
            };
            agent.queue_len = agent.queue_len.saturating_sub(removed);
            if agent.queue_len == 0 && matches!(agent.status, AgentStatus::Waiting) {
                agent.status = AgentStatus::Idle;
            }
        }
    }
    let removed = before.saturating_sub(state.agents.messages.len());
    state.agents.console_scroll = CONSOLE_SCROLL_BOTTOM;
    state.agents.artifacts_selected = 0;
    state.agents.artifacts_selected_saved_run_path = None;
    state.agents.artifacts_history_selected = 0;
    state.agents.artifacts_history_popup_scroll = 0;
    state.agents.artifacts_history_popup_open = false;
    state.agents.artifacts_history_pending_action = None;
    state.agents.global_archive_open = false;
    state.agents.global_archive_index.clear();
    state.agents.global_archive_filtered.clear();

    state.agents.diag_events.push(AgentDiagnosticEvent {
        severity: AgentAlertSeverity::Info,
        source: "ops".into(),
        message: format!(
            "{}context reset{} (cleared {removed} msgs)",
            mission_ctx
                .as_deref()
                .map(|id| format!("mission {id} "))
                .unwrap_or_default(),
            if agent_label.is_empty() {
                format!(" for {agent_id}")
            } else {
                format!(" for {agent_id} ({agent_label})")
            }
        ),
        at: timestamp_label(state),
    });
    state.status = Some(format!(
        "{}Context reset: {}",
        mission_ctx
            .as_deref()
            .map(|id| format!("{id} "))
            .unwrap_or_default(),
        if agent_label.is_empty() {
            agent_id
        } else {
            agent_label.to_string()
        }
    ));
    true
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_agent_console_key(
    key: KeyEvent,
    state: &mut AppState,
    vitals: &mut VitalsState,
    codex: Option<&CodexRunner>,
    claude: Option<&ClaudeRunner>,
    swarm: &mut SwarmRuntime,
    shadow: &mut crate::shadow::ShadowRuntime,
    clipboard: &mut Option<Clipboard>,
) -> bool {
    let mut changed = false;
    let mut handled = false;
    let mut follow_chat_cursor = false;

    // Try reusable text-editing handler first.
    let edit = handle_chat_input_editing_key(&key, state, clipboard);
    if edit.handled {
        handled = true;
        changed = edit.changed;
        follow_chat_cursor = edit.follow_cursor;
    }

    // Keys specific to the Agent Console context (not handled by the shared editor).
    if !handled {
        match key {
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                handled = true;
                changed =
                    submit_chat_input_and_dispatch(state, vitals, codex, claude, swarm, shadow);
                follow_chat_cursor = changed;
            }
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                if matches!(
                    state.ui_selection,
                    Some(nit_core::UiSelection {
                        pane: UiSelectionPane::AgentConsole,
                        ..
                    })
                ) {
                    handled = true;
                    state.ui_selection = None;
                }
                if state.agents.chat_input_selection_anchor.is_some() {
                    handled = true;
                    state.agents.chat_input_selection_anchor = None;
                }
            }
            KeyEvent {
                modifiers,
                code: KeyCode::Up,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                handled = true;
                bump_scroll_clamped(
                    &mut state.agents.console_scroll,
                    -1,
                    state.agents.console_max_scroll,
                );
                changed = true;
            }
            KeyEvent {
                code: KeyCode::Down,
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                handled = true;
                bump_scroll_clamped(
                    &mut state.agents.console_scroll,
                    1,
                    state.agents.console_max_scroll,
                );
                changed = true;
            }
            KeyEvent {
                code: KeyCode::Up,
                modifiers,
                ..
            } => {
                handled = true;
                let selecting = modifiers.contains(KeyModifiers::SHIFT);
                let cursor = state.agents.chat_input_cursor;
                let moved = chat_cursor_move_vertical(&state.agents.chat_input, cursor, -1);
                if moved != cursor {
                    if selecting {
                        if state.agents.chat_input_selection_anchor.is_none() {
                            state.agents.chat_input_selection_anchor = Some(cursor);
                        }
                    } else {
                        state.agents.chat_input_selection_anchor = None;
                    }
                    state.agents.chat_input_cursor = moved;
                    changed = true;
                    follow_chat_cursor = true;
                    if selecting {
                        copy_chat_input_selection(state, clipboard);
                    }
                } else if !selecting && chat_history_prev(state) {
                    state.agents.chat_input_selection_anchor = None;
                    changed = true;
                    follow_chat_cursor = true;
                }
            }
            KeyEvent {
                code: KeyCode::Down,
                modifiers,
                ..
            } => {
                handled = true;
                let selecting = modifiers.contains(KeyModifiers::SHIFT);
                let cursor = state.agents.chat_input_cursor;
                let moved = chat_cursor_move_vertical(&state.agents.chat_input, cursor, 1);
                if moved != cursor {
                    if selecting {
                        if state.agents.chat_input_selection_anchor.is_none() {
                            state.agents.chat_input_selection_anchor = Some(cursor);
                        }
                    } else {
                        state.agents.chat_input_selection_anchor = None;
                    }
                    state.agents.chat_input_cursor = moved;
                    changed = true;
                    follow_chat_cursor = true;
                    if selecting {
                        copy_chat_input_selection(state, clipboard);
                    }
                } else if !selecting && chat_history_next(state) {
                    state.agents.chat_input_selection_anchor = None;
                    changed = true;
                    follow_chat_cursor = true;
                }
            }
            _ => {}
        }
    }

    if changed {
        if follow_chat_cursor {
            state.agents.chat_input_scroll = usize::MAX;
        }
        state.agents.note_event();
        vitals.record_agent_event(Instant::now());
    }
    handled
}
