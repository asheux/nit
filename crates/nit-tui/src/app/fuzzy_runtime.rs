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

pub(super) struct FuzzySearchRuntime {
    pub(super) indexer: FileIndexRunner,
    pub(super) fuzzy: FuzzyMatcherRunner,
    pub(super) content: ContentSearchRunner,
    pub(super) preview: PreviewRunner,

    pub(super) index_gen: u64,
    pub(super) file_gen: u64,
    pub(super) content_gen: u64,
    pub(super) preview_gen: u64,

    pub(super) index_ready: bool,
    pub(super) index_filters: Option<(bool, bool)>,

    pub(super) preview_model: Option<PreviewModel>,
    pub(super) last_preview_key: Option<PreviewKey>,
    pub(super) preview_scroll_delta: i32,
    pub(super) last_open: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PreviewKey {
    pub(super) mode: SearchMode,
    pub(super) path: PathBuf,
    pub(super) line_hint: usize,
    pub(super) query: String,
}

impl FuzzySearchRuntime {
    pub(super) fn new(theme: &Theme, highlight: nit_core::HighlightConfig) -> Self {
        Self {
            indexer: FileIndexRunner::spawn(),
            fuzzy: FuzzyMatcherRunner::spawn(),
            content: ContentSearchRunner::spawn(),
            preview: PreviewRunner::spawn(theme.clone(), highlight),
            index_gen: 0,
            file_gen: 0,
            content_gen: 0,
            preview_gen: 0,
            index_ready: false,
            index_filters: None,
            preview_model: None,
            last_preview_key: None,
            preview_scroll_delta: 0,
            last_open: false,
        }
    }

    pub(super) fn shutdown(&mut self) {
        self.indexer.shutdown();
        self.fuzzy.shutdown();
        self.content.shutdown();
        self.preview.shutdown();
    }

    pub(super) fn update_syntax_config(&self, highlight: nit_core::HighlightConfig) {
        self.preview.update_config(highlight);
    }

    pub(super) fn tick_open(&mut self, state: &mut AppState) {
        if state.fuzzy_search.open {
            if !self.last_open {
                self.last_open = true;
                self.preview_model = None;
                self.last_preview_key = None;
                self.preview_scroll_delta = 0;
                self.ensure_index(state);
                self.run_search_for_mode(state);
                self.request_preview_for_selection(state);
            }
            return;
        }
        if self.last_open {
            self.last_open = false;
            self.preview_model = None;
            self.last_preview_key = None;
            self.preview_scroll_delta = 0;
            self.file_gen = self.file_gen.wrapping_add(1);
            self.preview_gen = self.preview_gen.wrapping_add(1);
            self.fuzzy.send(FuzzyCommand::Query {
                generation: 0,
                query: String::new(),
            });
            state.fuzzy_search.status_msg.clear();
            state.fuzzy_search.indexing = false;
            state.fuzzy_search.searching = false;
            // Cancel any in-flight content search quickly.
            self.content_gen = self.content_gen.wrapping_add(1);
            self.content.search(
                self.content_gen,
                state.workspace_root.clone(),
                String::new(),
                state.fuzzy_search.show_hidden,
                state.fuzzy_search.show_ignored,
            );
        }
    }

    pub(super) fn ensure_index(&mut self, state: &mut AppState) {
        let filters = (
            state.fuzzy_search.show_hidden,
            state.fuzzy_search.show_ignored,
        );
        let needs = !self.index_ready || self.index_filters != Some(filters);
        if !needs {
            return;
        }
        self.index_filters = Some(filters);
        self.index_ready = false;
        self.index_gen = self.index_gen.wrapping_add(1);
        state.fuzzy_search.indexing = true;
        state.fuzzy_search.status_msg = "Indexing…".into();
        self.preview_model = None;
        self.preview_scroll_delta = 0;

        self.fuzzy.send(FuzzyCommand::ResetIndex {
            generation: self.index_gen,
            root: state.workspace_root.clone(),
        });
        self.indexer.build(
            self.index_gen,
            state.workspace_root.clone(),
            state.fuzzy_search.show_hidden,
            state.fuzzy_search.show_ignored,
        );
    }

    pub(super) fn rebuild_index(&mut self, state: &mut AppState) {
        self.index_ready = false;
        self.index_filters = None;
        self.ensure_index(state);
    }

    pub(super) fn run_search_for_mode(&mut self, state: &mut AppState) {
        match state.fuzzy_search.mode {
            SearchMode::Files => self.run_file_query(state),
            SearchMode::Content => self.run_content_query(state),
        }
    }

    pub(super) fn run_file_query(&mut self, state: &mut AppState) {
        self.file_gen = self.file_gen.wrapping_add(1);
        state.fuzzy_search.searching = false;
        state.fuzzy_search.file_results.clear();
        state.fuzzy_search.selected = 0;
        state.fuzzy_search.scroll_offset = 0;
        self.fuzzy.send(FuzzyCommand::Query {
            generation: self.file_gen,
            query: state.fuzzy_search.query.clone(),
        });
    }

    pub(super) fn run_content_query(&mut self, state: &mut AppState) {
        self.content_gen = self.content_gen.wrapping_add(1);
        state.fuzzy_search.match_results.clear();
        state.fuzzy_search.selected = 0;
        state.fuzzy_search.scroll_offset = 0;
        let query = state.fuzzy_search.query.trim().to_string();
        if query.is_empty() {
            state.fuzzy_search.searching = false;
            state.fuzzy_search.status_msg = "Type to search".into();
            self.content.search(
                self.content_gen,
                state.workspace_root.clone(),
                String::new(),
                state.fuzzy_search.show_hidden,
                state.fuzzy_search.show_ignored,
            );
            return;
        }
        state.fuzzy_search.searching = true;
        state.fuzzy_search.status_msg = "Searching…".into();
        self.content.search(
            self.content_gen,
            state.workspace_root.clone(),
            query,
            state.fuzzy_search.show_hidden,
            state.fuzzy_search.show_ignored,
        );
    }

    pub(super) fn request_preview_for_selection(&mut self, state: &AppState) {
        if !state.fuzzy_search.open {
            return;
        }
        let (path, line_hint, query) = match state.fuzzy_search.mode {
            SearchMode::Files => {
                let Some(item) = state
                    .fuzzy_search
                    .file_results
                    .get(state.fuzzy_search.selected)
                else {
                    return;
                };
                (item.abs_path.clone(), None, String::new())
            }
            SearchMode::Content => {
                let Some(item) = state
                    .fuzzy_search
                    .match_results
                    .get(state.fuzzy_search.selected)
                else {
                    return;
                };
                (
                    item.abs_path.clone(),
                    Some(item.line),
                    state.fuzzy_search.query.trim().to_string(),
                )
            }
        };
        let key = PreviewKey {
            mode: state.fuzzy_search.mode,
            path: path.clone(),
            line_hint: line_hint.unwrap_or(0),
            query: if matches!(state.fuzzy_search.mode, SearchMode::Content) {
                query.clone()
            } else {
                String::new()
            },
        };
        if self.last_preview_key.as_ref() == Some(&key) {
            return;
        }
        self.preview_scroll_delta = 0;
        self.last_preview_key = Some(key);
        self.preview_gen = self.preview_gen.wrapping_add(1);
        self.preview.request(
            self.preview_gen,
            state.fuzzy_search.mode,
            path,
            line_hint,
            query,
        );
    }
}
