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

#[derive(Default)]
pub(super) struct TerminalState {
    active: bool,
    raw_mode: bool,
    alternate_screen: bool,
    keyboard_flags_pushed: bool,
    mouse_capture: bool,
    bracketed_paste: bool,
    cursor_hidden: bool,
}

impl TerminalState {
    pub(super) fn restore(&mut self) {
        if !self.active {
            return;
        }
        let mut stdout = io::stdout();
        if self.keyboard_flags_pushed {
            let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        }
        if self.mouse_capture {
            let _ = execute!(stdout, DisableMouseCapture);
        }
        if self.bracketed_paste {
            let _ = execute!(stdout, DisableBracketedPaste);
        }
        let _ = execute!(stdout, SetCursorStyle::DefaultUserShape);
        if self.cursor_hidden {
            let _ = execute!(stdout, Show);
        }
        if self.raw_mode {
            let _ = disable_raw_mode();
        }
        if self.alternate_screen {
            let _ = execute!(stdout, LeaveAlternateScreen);
        }
        self.active = false;
    }
}

pub(super) struct TerminalGuard {
    state: Arc<Mutex<TerminalState>>,
}

impl TerminalGuard {
    pub(super) fn activate() -> io::Result<(Self, Stdout)> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(err) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(err);
        }
        let state = TerminalState {
            active: true,
            raw_mode: true,
            alternate_screen: true,
            ..TerminalState::default()
        };
        Ok((
            Self {
                state: Arc::new(Mutex::new(state)),
            },
            stdout,
        ))
    }

    pub(super) fn weak_state(&self) -> Weak<Mutex<TerminalState>> {
        Arc::downgrade(&self.state)
    }

    pub(super) fn enable_mouse_capture(&self, stdout: &mut Stdout) -> io::Result<()> {
        execute!(stdout, EnableMouseCapture)?;
        if let Ok(mut state) = self.state.lock() {
            state.mouse_capture = true;
        }
        Ok(())
    }

    pub(super) fn push_keyboard_flags(&self, stdout: &mut Stdout, flags: KeyboardEnhancementFlags) {
        if execute!(stdout, PushKeyboardEnhancementFlags(flags)).is_ok() {
            if let Ok(mut state) = self.state.lock() {
                state.keyboard_flags_pushed = true;
            }
        }
    }

    pub(super) fn enable_bracketed_paste(&self, stdout: &mut Stdout) -> io::Result<()> {
        execute!(stdout, EnableBracketedPaste)?;
        if let Ok(mut state) = self.state.lock() {
            state.bracketed_paste = true;
        }
        Ok(())
    }

    pub(super) fn mark_cursor_hidden(&self, hidden: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.cursor_hidden = hidden;
        }
    }

    pub(super) fn restore(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.restore();
        }
    }

    pub(super) fn install_sigint_handler(&self) -> Result<(), CtrlcError> {
        let weak = Arc::downgrade(&self.state);
        ctrlc::set_handler(move || {
            if let Some(state) = weak.upgrade() {
                if let Ok(mut state) = state.lock() {
                    state.restore();
                }
            }
            std::process::exit(130);
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

pub(super) fn install_terminal_panic_hook(state: Weak<Mutex<TerminalState>>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(state) = state.upgrade() {
            if let Ok(mut state) = state.lock() {
                state.restore();
            }
        }
        previous(info);
    }));
}

// Shells out to `git show HEAD:<relpath>` so the diff overlay can compare the
// on-disk buffer against the committed version. Returns `None` whenever the
// file isn't tracked, git is missing, or the resolution fails for any reason —
// callers just fall back to "no diff".
pub(super) fn git_head_content(path: &Path) -> Option<String> {
    use std::process::Command;
    let dir = path.parent()?;
    let root_out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !root_out.status.success() {
        return None;
    }
    let root = PathBuf::from(String::from_utf8(root_out.stdout).ok()?.trim());
    let rel = path
        .canonicalize()
        .ok()?
        .strip_prefix(&root)
        .ok()?
        .to_path_buf();
    let output = Command::new("git")
        .args(["show", &format!("HEAD:{}", rel.display())])
        .current_dir(&root)
        .output()
        .ok()?;
    if output.status.success() {
        String::from_utf8(output.stdout).ok()
    } else {
        None
    }
}
