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

pub(super) fn map_focus_hotkey(key: &KeyEvent) -> Option<PaneId> {
    match key {
        KeyEvent {
            code: KeyCode::Char('1'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => Some(PaneId::Editor),
        KeyEvent {
            code: KeyCode::Char('2'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => Some(PaneId::JobOutput),
        KeyEvent {
            code: KeyCode::Char('3'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => Some(PaneId::Notes),
        _ => None,
    }
}

pub(super) fn is_global_run_key(key: &KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Enter | KeyCode::Char('\n') | KeyCode::Char('\r'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    )
}

pub(super) fn is_global_quit_key(key: &KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('q'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    )
}

pub(super) fn is_command_prompt_open_key(key: &KeyEvent) -> bool {
    match key {
        KeyEvent {
            code: KeyCode::Char(':'),
            ..
        } => true,
        KeyEvent {
            code: KeyCode::Char(';'),
            modifiers,
            ..
        } => modifiers.contains(KeyModifiers::SHIFT),
        _ => false,
    }
}

pub(super) fn is_help_toggle_key(key: &KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::F(1),
            ..
        }
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('?'),
            modifiers,
            ..
        } if modifiers.is_empty() || *modifiers == KeyModifiers::SHIFT
    )
}

pub(super) fn is_petri_show_key(key: &KeyEvent, state: &AppState) -> bool {
    match state.app_kind {
        AppKind::Gol => {
            if !state.visualizer.petri_hidden || !state.visualizer.running {
                return false;
            }
        }
        AppKind::Games => {
            if !state.games.petri_hidden || !games_petri_active(state) {
                return false;
            }
        }
    }
    match key {
        KeyEvent {
            code: KeyCode::Char('^') | KeyCode::Char('6'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL) => true,
        KeyEvent {
            code: KeyCode::Char('\u{1e}'),
            modifiers,
            ..
        } if modifiers.is_empty() || modifiers.contains(KeyModifiers::CONTROL) => true,
        _ => false,
    }
}

pub(super) fn games_petri_active(state: &AppState) -> bool {
    state.games.running
        || matches!(
            state.games.status,
            nit_core::GamesStatus::Paused | nit_core::GamesStatus::Done
        )
        || !state.games.petri_lines.is_empty()
}

pub(super) fn is_games_history_open_key(key: &KeyEvent, state: &AppState) -> bool {
    if state.app_kind != AppKind::Games {
        return false;
    }
    if !state.games.running && state.games.last_run.is_none() {
        return false;
    }
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('*') | KeyCode::Char('8'),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    )
}

pub(super) fn games_petri_visible(state: &AppState) -> bool {
    state.app_kind == AppKind::Games && games_petri_active(state) && !state.games.petri_hidden
}

pub(super) fn current_games_config_result(
    state: &AppState,
) -> Option<&Result<nit_games::config::NormalizedConfig, nit_games::config::ConfigError>> {
    let version = state.editor_buffer().version();
    state
        .games
        .config_preview
        .as_ref()
        .and_then(|preview| (preview.version == version).then_some(&preview.result))
}

pub(super) fn games_modal_popup_open(state: &AppState) -> bool {
    if state.app_kind != AppKind::Games {
        return false;
    }
    state.games.analysis.open
        || state.games.run_browser.open
        || state.games.replay.open
        || state.games.strategy_inspect.open
        || state.games.tm_sim.open
        || state.games.ca_sim.open
        || state.games.match_history.open
}

pub(super) fn is_games_petri_control_key(key: &KeyEvent) -> bool {
    matches!(
        key.code,
        KeyCode::Char(' ')
            | KeyCode::Null
            | KeyCode::Char('\u{0}')
            | KeyCode::Enter
            | KeyCode::Char('\n')
            | KeyCode::Char('\r')
            | KeyCode::Char('+')
            | KeyCode::Char('=')
            | KeyCode::Char('-')
            | KeyCode::Char('_')
            | KeyCode::Tab
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Char('c')
            | KeyCode::Char('C')
            | KeyCode::Char('h')
            | KeyCode::Char('H')
            | KeyCode::Char('r')
            | KeyCode::Char('R')
            | KeyCode::Char('x')
            | KeyCode::Char('X')
            | KeyCode::Char('y')
            | KeyCode::Char('Y')
            | KeyCode::Char('n')
            | KeyCode::Char('N')
    )
}

pub(super) fn is_job_pause_key(key: &KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Char(' '),
            modifiers,
            ..
        } if modifiers.contains(KeyModifiers::CONTROL)
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Null,
            ..
        }
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('\u{0}'),
            modifiers,
            ..
        } if modifiers.is_empty()
    ) || matches!(
        key,
        KeyEvent {
            code: KeyCode::F(6),
            ..
        }
    )
}

pub(super) fn ctrl_nav_dir(key: &KeyEvent) -> Option<FocusDir> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if !ctrl || key.modifiers.contains(KeyModifiers::SHIFT) {
        return None;
    }
    match key.code {
        KeyCode::Char('h') if ctrl => Some(FocusDir::Left),
        KeyCode::Char('j') if ctrl => Some(FocusDir::Down),
        KeyCode::Char('k') if ctrl => Some(FocusDir::Up),
        KeyCode::Char('l') if ctrl => Some(FocusDir::Right),
        KeyCode::Backspace if ctrl => Some(FocusDir::Left),
        KeyCode::Enter if ctrl => Some(FocusDir::Down),
        KeyCode::Char('\u{8}') => Some(FocusDir::Left),
        KeyCode::Char('\n') => Some(FocusDir::Down),
        KeyCode::Char('\u{0b}') => Some(FocusDir::Up),
        KeyCode::Char('\u{0c}') => Some(FocusDir::Right),
        _ => None,
    }
}
