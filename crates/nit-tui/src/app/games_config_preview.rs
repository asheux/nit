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

pub(super) enum GamesConfigPreviewCommand {
    Parse {
        version: u64,
        config_text: String,
        workspace_root: PathBuf,
    },
    Shutdown,
}

pub(super) struct GamesConfigPreviewEvent {
    version: u64,
    result: Result<nit_games::config::NormalizedConfig, nit_games::config::ConfigError>,
}

pub(super) struct GamesConfigPreviewRuntime {
    cmd_tx: Sender<GamesConfigPreviewCommand>,
    events: Receiver<GamesConfigPreviewEvent>,
    handle: Option<JoinHandle<()>>,
    pending_version: Option<u64>,
}

impl GamesConfigPreviewRuntime {
    pub(super) fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<GamesConfigPreviewCommand>();
        let (event_tx, event_rx) = mpsc::channel::<GamesConfigPreviewEvent>();
        let handle = thread::Builder::new()
            .name("nit-games-config-preview".into())
            .spawn(move || {
                while let Ok(cmd) = cmd_rx.recv() {
                    match cmd {
                        GamesConfigPreviewCommand::Parse {
                            version,
                            config_text,
                            workspace_root,
                        } => {
                            let result = GamesConfig::from_toml_with_root(
                                &config_text,
                                Some(&workspace_root),
                            );
                            let _ = event_tx.send(GamesConfigPreviewEvent { version, result });
                        }
                        GamesConfigPreviewCommand::Shutdown => break,
                    }
                }
            })
            .expect("spawn games config preview");
        Self {
            cmd_tx,
            events: event_rx,
            handle: Some(handle),
            pending_version: None,
        }
    }

    pub(super) fn request_for_editor(&mut self, state: &mut AppState) {
        if state.app_kind != AppKind::Games {
            return;
        }
        let version = state.editor_buffer().version();
        if state
            .games
            .config_preview
            .as_ref()
            .is_some_and(|preview| preview.version == version)
            || self.pending_version == Some(version)
        {
            return;
        }
        let cmd = GamesConfigPreviewCommand::Parse {
            version,
            config_text: state.editor_buffer().content_as_string(),
            workspace_root: state.workspace_root.clone(),
        };
        if self.cmd_tx.send(cmd).is_ok() {
            self.pending_version = Some(version);
            state.games.config_preview_pending = true;
        } else {
            state.games.config_preview_pending = false;
        }
    }

    pub(super) fn poll(&mut self, state: &mut AppState) {
        while let Ok(event) = self.events.try_recv() {
            state.games.config_preview = Some(nit_core::GamesConfigPreview {
                version: event.version,
                result: event.result,
            });
            if self.pending_version == Some(event.version) {
                self.pending_version = None;
            }
            if state.editor_buffer().version() == event.version {
                state.games.config_preview_pending = false;
            }
        }
    }

    pub(super) fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(GamesConfigPreviewCommand::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
