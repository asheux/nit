use crate::{
    actions::Action, buffer::Buffer, io, mode::Mode, pane::PaneId, prompt::Prompt,
    viewport::Viewport,
};
use std::collections::VecDeque;
use std::path::PathBuf;

const DEFAULT_LOG_CAPACITY: usize = 512;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LogBuffer {
    lines: VecDeque<String>,
    capacity: usize,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, line: impl Into<String>) {
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(line.into());
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    pub fn iter(&self) -> std::collections::vec_deque::Iter<'_, String> {
        self.lines.iter()
    }
}

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct JobState {
    pub paused: bool,
    pub progress: f32,
}

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct VisualizerState {
    pub seed: u64,
    pub variant: u8,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Metrics {
    pub last_render_ms: u128,
    pub frame_count: u64,
    pub last_action: Option<Action>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AppState {
    pub workspace_root: PathBuf,
    pub buffers: Vec<Buffer>,
    pub active_editor_buffer_id: usize,
    pub notes_buffer_id: usize,
    pub mode: Mode,
    pub focus: PaneId,
    pub logs: LogBuffer,
    pub job: JobState,
    pub visualizer: VisualizerState,
    pub metrics: Metrics,
    pub prompt: Option<Prompt>,
    pub show_help: bool,
    pub status: Option<String>,
    #[serde(skip)]
    pub yank: Option<String>,
}

pub struct ActionOutcome {
    pub should_exit: bool,
    pub state_changed: bool,
}

impl AppState {
    pub fn new(workspace_root: PathBuf, editor: Buffer, notes: Buffer) -> Self {
        Self {
            workspace_root,
            buffers: vec![editor, notes],
            active_editor_buffer_id: 0,
            notes_buffer_id: 1,
            mode: Mode::Normal,
            focus: PaneId::Editor,
            logs: LogBuffer::new(DEFAULT_LOG_CAPACITY),
            job: JobState {
                paused: false,
                progress: 0.0,
            },
            visualizer: VisualizerState {
                seed: 1,
                variant: 0,
            },
            metrics: Metrics {
                last_render_ms: 0,
                frame_count: 0,
                last_action: None,
            },
            prompt: None,
            show_help: false,
            status: None,
            yank: None,
        }
    }

    pub fn buffer_mut(&mut self, id: usize) -> Option<&mut Buffer> {
        self.buffers.get_mut(id)
    }

    pub fn buffer(&self, id: usize) -> Option<&Buffer> {
        self.buffers.get(id)
    }

    pub fn editor_buffer(&self) -> &Buffer {
        &self.buffers[self.active_editor_buffer_id]
    }

    pub fn editor_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.active_editor_buffer_id]
    }

    pub fn notes_buffer(&self) -> &Buffer {
        &self.buffers[self.notes_buffer_id]
    }

    pub fn notes_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.notes_buffer_id]
    }

    pub fn focused_buffer_mut(&mut self) -> Option<&mut Buffer> {
        match self.focus {
            PaneId::Editor => Some(self.editor_buffer_mut()),
            PaneId::Notes => Some(self.notes_buffer_mut()),
            _ => None,
        }
    }

    pub fn set_viewport(&mut self, pane: PaneId, viewport: Viewport) {
        match pane {
            PaneId::Editor => {
                let buf = self.editor_buffer_mut();
                buf.viewport = viewport;
            }
            PaneId::Notes => {
                let buf = self.notes_buffer_mut();
                buf.viewport = viewport;
            }
            _ => {}
        }
    }

    pub fn line_col(&self) -> (usize, usize) {
        let buf = self.editor_buffer();
        (buf.cursor.line + 1, buf.cursor.col + 1)
    }

    pub fn receive_log(&mut self, line: impl Into<String>) {
        self.logs.push(line);
    }

    pub fn tick_job(&mut self, delta: f32) {
        if self.job.paused {
            return;
        }
        self.job.progress += delta;
        if self.job.progress >= 1.0 {
            self.job.progress = 0.0;
        }
    }
}

fn focus_order_index(focus: PaneId) -> usize {
    PaneId::ALL.iter().position(|p| *p == focus).unwrap_or(0)
}

pub fn apply_action(state: &mut AppState, action: Action) -> ActionOutcome {
    state.metrics.last_action = Some(action.clone());
    let mut should_exit = false;
    let changed = true;

    match action {
        Action::Quit => {
            if state.editor_buffer().is_dirty() {
                state.prompt = Some(Prompt::ConfirmQuit);
            } else {
                should_exit = true;
            }
        }
        Action::ConfirmQuitYes => {
            should_exit = true;
        }
        Action::ConfirmQuitNo => {
            state.prompt = None;
        }
        Action::Save | Action::SaveAndNormal => {
            let buf = state.editor_buffer_mut();
            if buf.path().is_none() {
                state.status = Some("No path to save".into());
            } else if let Err(e) = io::save_buffer(buf) {
                state.status = Some(format!("Save failed: {e}"));
            } else {
                buf.mark_clean();
                state.status = Some("Saved".into());
            }
            if matches!(action, Action::SaveAndNormal) {
                state.mode = Mode::Normal;
                if let Some(buf) = state.focused_buffer_mut() {
                    buf.exit_insert_mode();
                    buf.clear_selection();
                }
            }
        }
        Action::FocusNextPane => {
            let idx = focus_order_index(state.focus);
            let next = (idx + 1) % PaneId::ALL.len();
            state.focus = PaneId::ALL[next];
        }
        Action::FocusPrevPane => {
            let idx = focus_order_index(state.focus);
            let prev = if idx == 0 {
                PaneId::ALL.len() - 1
            } else {
                idx - 1
            };
            state.focus = PaneId::ALL[prev];
        }
        Action::FocusPane(p) => {
            state.focus = p;
        }
        Action::SwitchMode(m) => {
            state.mode = m;
            if let Some(buf) = state.focused_buffer_mut() {
                if m == Mode::Normal {
                    buf.exit_insert_mode();
                    buf.clear_selection();
                } else if m == Mode::Visual {
                    buf.set_selection_anchor();
                } else {
                    buf.clear_selection();
                }
            }
        }
        Action::ToggleMode => {
            state.mode = state.mode.toggle();
            let mode = state.mode;
            if let Some(buf) = state.focused_buffer_mut() {
                if mode == Mode::Normal {
                    buf.exit_insert_mode();
                }
                buf.clear_selection();
            }
        }
        Action::InsertChar(c) => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.insert_char(c);
                buf.ensure_visible();
            }
        }
        Action::InsertNewline => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.insert_newline();
                buf.ensure_visible();
            }
        }
        Action::InsertTab => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.insert_tab();
                buf.ensure_visible();
            }
        }
        Action::EnterVisual => {
            state.mode = Mode::Visual;
            if let Some(buf) = state.focused_buffer_mut() {
                buf.set_selection_anchor();
            }
        }
        Action::ExitVisual => {
            state.mode = Mode::Normal;
            if let Some(buf) = state.focused_buffer_mut() {
                buf.clear_selection();
            }
        }
        Action::YankSelection => {
            let yank = if let Some(buf) = state.focused_buffer_mut() {
                let yank = buf.yank_selection();
                buf.clear_selection();
                yank
            } else {
                None
            };
            state.yank = yank;
            state.mode = Mode::Normal;
        }
        Action::DeleteSelection => {
            if let Some(buf) = state.focused_buffer_mut() {
                if buf.delete_selection() {
                    buf.ensure_visible();
                }
            }
            state.mode = Mode::Normal;
        }
        Action::Paste => {
            let yank = state.yank.clone();
            let is_normal = state.mode == Mode::Normal;
            if let (Some(yank), Some(buf)) = (yank, state.focused_buffer_mut()) {
                if is_normal {
                    buf.append();
                }
                buf.insert_str(&yank);
                buf.ensure_visible();
            }
        }
        Action::Append => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.append();
                buf.ensure_visible();
                state.mode = Mode::Insert;
            }
        }
        Action::Backspace => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.backspace();
                buf.ensure_visible();
            }
        }
        Action::Delete => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.delete_forward();
                buf.ensure_visible();
            }
        }
        Action::DeleteLine => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.delete_line();
                buf.ensure_visible();
            }
        }
        Action::MoveUp => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_up();
                buf.ensure_visible();
            }
        }
        Action::MoveDown => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_down();
                buf.ensure_visible();
            }
        }
        Action::MoveLeft => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_left();
                buf.ensure_visible();
            }
        }
        Action::MoveRight => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_right();
                buf.ensure_visible();
            }
        }
        Action::PageUp => {
            if let Some(buf) = state.focused_buffer_mut() {
                let height = buf.viewport.height.max(1);
                buf.page_up(height);
                buf.ensure_visible();
            }
        }
        Action::PageDown => {
            if let Some(buf) = state.focused_buffer_mut() {
                let height = buf.viewport.height.max(1);
                buf.page_down(height);
                buf.ensure_visible();
            }
        }
        Action::Home => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_home();
                buf.ensure_visible();
            }
        }
        Action::End => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_end();
                buf.ensure_visible();
            }
        }
        Action::MoveWordEnd => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_word_end();
                buf.ensure_visible();
            }
        }
        Action::MoveWordBack => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_word_back();
                buf.ensure_visible();
            }
        }
        Action::GoToTop => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.go_to_top();
                buf.ensure_visible();
            }
        }
        Action::GoToBottom => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.go_to_bottom();
                buf.ensure_visible();
            }
        }
        Action::OpenLineBelow => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.move_end();
                buf.insert_newline();
                buf.ensure_visible();
                state.mode = Mode::Insert;
            }
        }
        Action::Undo => {
            if let Some(buf) = state.focused_buffer_mut() {
                if buf.undo() {
                    buf.ensure_visible();
                }
            }
        }
        Action::Redo => {
            if let Some(buf) = state.focused_buffer_mut() {
                if buf.redo() {
                    buf.ensure_visible();
                }
            }
        }
        Action::ScrollUp => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.viewport.offset_line = buf.viewport.offset_line.saturating_sub(1);
            }
        }
        Action::ScrollDown => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.viewport.offset_line = buf.viewport.offset_line.saturating_add(1);
            }
        }
        Action::ClearLogs => {
            state.logs.clear();
        }
        Action::ToggleJobPause => {
            state.job.paused = !state.job.paused;
        }
        Action::VisualizerReseed => {
            state.visualizer.seed = state.visualizer.seed.wrapping_add(1);
        }
        Action::VisualizerApply => {
            state.visualizer.variant = state.visualizer.variant.wrapping_add(1);
        }
        Action::ShowHelp => state.show_help = true,
        Action::HideHelp => state.show_help = false,
    }

    ActionOutcome {
        should_exit,
        state_changed: changed,
    }
}
