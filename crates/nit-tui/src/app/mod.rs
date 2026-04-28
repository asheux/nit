#![allow(unused_imports)]

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

mod artifacts_popup_handler;
mod chat_input;
mod dispatch;

mod agent_station;
mod chat_cursor;
mod chords;
mod draw;
mod file_tree_fuzzy;
mod fuzzy_runtime;
mod games_config_preview;
mod genome_retry;
mod input_state;
mod key_dispatch;
mod key_predicates;
mod layout_rects;
mod mouse;
mod mouse_mappers;
pub(crate) mod popup_keys;
mod provenance;
mod runner;
mod scroll;
mod terminal;
mod ui_selection;
mod vitals_log;

pub use runner::run;

pub(crate) use artifacts_popup_handler::*;
pub(crate) use chat_input::*;
pub(crate) use dispatch::*;

pub(crate) use agent_station::*;
pub(crate) use chat_cursor::*;
pub(crate) use chords::*;
pub(crate) use draw::*;
pub(crate) use file_tree_fuzzy::*;
pub(crate) use fuzzy_runtime::*;
pub(crate) use games_config_preview::*;
pub(crate) use genome_retry::*;
pub(crate) use input_state::*;
pub(crate) use key_dispatch::*;
pub(crate) use key_predicates::*;
pub(crate) use layout_rects::*;
pub(crate) use mouse::*;
pub(crate) use mouse_mappers::*;
pub(crate) use popup_keys::*;
pub(crate) use provenance::*;
pub(crate) use runner::*;
pub(crate) use scroll::*;
pub(crate) use terminal::*;
pub(crate) use ui_selection::*;
pub(crate) use vitals_log::*;

#[cfg(test)]
mod tests;
