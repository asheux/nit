use crate::{
    actions::Action,
    buffer::Buffer,
    config::{GolSeedSource, Settings},
    io,
    mode::Mode,
    pane::PaneId,
    prompt::Prompt,
    seed::{SeedEncoderId, SeedParams, SeedPreviewMode, SeedStats, SeedViewMode},
    viewport::Viewport,
};
use nit_gol::{AttractorEvent, AutoStopPolicy};
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

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum VisualizerMode {
    SimOnly,
    Search,
}

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum GolRenderMode {
    Solid,
    HalfBlock,
    Braille,
}

impl GolRenderMode {
    pub fn next(self, braille_enabled: bool) -> Self {
        match self {
            GolRenderMode::Solid => GolRenderMode::HalfBlock,
            GolRenderMode::HalfBlock => {
                if braille_enabled {
                    GolRenderMode::Braille
                } else {
                    GolRenderMode::Solid
                }
            }
            GolRenderMode::Braille => GolRenderMode::Solid,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GolRenderMode::Solid => "SOLID",
            GolRenderMode::HalfBlock => "HALF",
            GolRenderMode::Braille => "BRAILLE",
        }
    }

    pub fn effective(self, braille_enabled: bool) -> Self {
        match self {
            GolRenderMode::Braille if !braille_enabled => GolRenderMode::HalfBlock,
            _ => self,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct VisualizerRuleEntry {
    pub rule: String,
    pub score: f32,
    pub period: Option<u32>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct VisualizerState {
    pub seed: u64,
    pub variant: u8,
    pub mode: VisualizerMode,
    pub seed_encoder: SeedEncoderId,
    pub seed_view: SeedViewMode,
    pub seed_plate_mode: SeedPreviewMode,
    pub seed_params: SeedParams,
    pub seed_stats: SeedStats,
    pub seed_hash: u64,
    pub input_hash: u64,
    pub seed_search_active: bool,
    pub seed_search_rps: u32,
    pub render_mode: GolRenderMode,
    pub running: bool,
    pub age_shading: bool,
    pub trails: bool,
    pub overlay_bbox: bool,
    pub overlay_heat: bool,
    pub scanlines: bool,
    pub paused: bool,
    pub paused_by_attractor: bool,
    pub wrap: bool,
    pub rule: String,
    pub generation: u64,
    pub alive: usize,
    pub period: Option<u32>,
    pub auto_stop_policy: AutoStopPolicy,
    pub last_attractor: Option<AttractorEvent>,
    pub tick_ms: u64,
    pub seed_source: GolSeedSource,
    pub search_rps: u32,
    pub leaderboard: Vec<VisualizerRuleEntry>,
    pub last_score: Option<f32>,
    pub seed_show_grid: bool,
    pub seed_show_bbox: bool,
    pub seed_show_halo: bool,
    pub seed_show_components: bool,
    pub seed_show_inset: bool,
    pub seed_scanline: bool,
    pub seed_zoom: u8,
    pub seed_snapshots_written: u64,
    pub seed_snapshots_dropped: u64,
    pub seed_snapshot_queue_depth: usize,
    pub seed_last_snapshot_path: Option<String>,
    pub snapshots_written: u64,
    pub snapshots_dropped: u64,
    pub snapshot_queue_depth: usize,
    pub last_snapshot_path: Option<String>,
    #[serde(skip)]
    pub petri_hidden: bool,
    #[serde(skip)]
    pub pending_reseed: bool,
    #[serde(skip)]
    pub pending_apply: bool,
    #[serde(skip)]
    pub pending_snapshot: bool,
    #[serde(skip)]
    pub pending_run: bool,
    #[serde(skip)]
    pub pending_close: bool,
    #[serde(skip)]
    pub pending_hide: bool,
    #[serde(skip)]
    pub pending_show: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Metrics {
    pub last_render_ms: u128,
    pub frame_count: u64,
    pub last_action: Option<Action>,
}

#[derive(Clone, Debug)]
pub struct CommandLine {
    pub input: String,
}

impl CommandLine {
    pub fn new() -> Self {
        Self {
            input: String::new(),
        }
    }
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
    pub settings: Settings,
    pub debug: bool,
    #[serde(skip)]
    pub yank: Option<String>,
    #[serde(skip)]
    pub yank_kind: YankKind,
    #[serde(skip)]
    pub command_line: Option<CommandLine>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum YankKind {
    #[default]
    Char,
    Line,
}

pub struct ActionOutcome {
    pub should_exit: bool,
    pub state_changed: bool,
}

impl AppState {
    pub fn new(workspace_root: PathBuf, editor: Buffer, notes: Buffer) -> Self {
        let settings = Settings::default();
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
                mode: VisualizerMode::SimOnly,
                seed_encoder: SeedEncoderId::AsciiBytes,
                seed_view: SeedViewMode::Genome,
                seed_plate_mode: SeedPreviewMode::Solid,
                seed_params: SeedParams::default(),
                seed_stats: SeedStats::default(),
                seed_hash: 0,
                input_hash: 0,
                seed_search_active: false,
                seed_search_rps: 0,
                render_mode: GolRenderMode::HalfBlock,
                running: false,
                age_shading: true,
                trails: true,
                overlay_bbox: false,
                overlay_heat: false,
                scanlines: false,
                paused: false,
                paused_by_attractor: false,
                wrap: settings.gol.wrap,
                rule: "B3/S23".to_string(),
                generation: 0,
                alive: 0,
                period: None,
                auto_stop_policy: AutoStopPolicy::Fixed,
                last_attractor: None,
                tick_ms: settings.gol.tick_ms,
                seed_source: settings.gol.seed_source,
                search_rps: 0,
                leaderboard: Vec::new(),
                last_score: None,
                seed_show_grid: false,
                seed_show_bbox: false,
                seed_show_halo: true,
                seed_show_components: false,
                seed_show_inset: true,
                seed_scanline: false,
                seed_zoom: 1,
                seed_snapshots_written: 0,
                seed_snapshots_dropped: 0,
                seed_snapshot_queue_depth: 0,
                seed_last_snapshot_path: None,
                snapshots_written: 0,
                snapshots_dropped: 0,
                snapshot_queue_depth: 0,
                last_snapshot_path: None,
                petri_hidden: false,
                pending_reseed: false,
                pending_apply: false,
                pending_snapshot: false,
                pending_run: false,
                pending_close: false,
                pending_hide: false,
                pending_show: false,
            },
            metrics: Metrics {
                last_render_ms: 0,
                frame_count: 0,
                last_action: None,
            },
            prompt: None,
            show_help: false,
            status: None,
            settings,
            debug: false,
            yank: None,
            yank_kind: YankKind::Char,
            command_line: None,
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
            if let Some(text) = yank {
                state.yank_kind = if text.contains('\n') {
                    YankKind::Line
                } else {
                    YankKind::Char
                };
                state.yank = Some(text);
            } else {
                state.yank = None;
                state.yank_kind = YankKind::Char;
            }
            state.mode = Mode::Normal;
        }
        Action::YankLine => {
            if let Some(buf) = state.focused_buffer_mut() {
                state.yank = Some(buf.yank_line());
                state.yank_kind = YankKind::Line;
            }
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
            let yank_kind = state.yank_kind;
            if let (Some(yank), Some(buf)) = (yank, state.focused_buffer_mut()) {
                if is_normal && yank_kind == YankKind::Line {
                    buf.paste_line_below(&yank);
                } else {
                    if is_normal {
                        buf.append();
                    }
                    buf.insert_str(&yank);
                }
                buf.ensure_visible();
            }
        }
        Action::PasteLineAbove => {
            let yank = state.yank.clone();
            let yank_kind = state.yank_kind;
            if let (Some(yank), Some(buf)) = (yank, state.focused_buffer_mut()) {
                if yank_kind == YankKind::Line {
                    buf.paste_line_above(&yank);
                } else {
                    let mut text = yank;
                    if !text.ends_with('\n') {
                        text.push('\n');
                    }
                    buf.paste_line_above(&text);
                }
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
        Action::OpenLineAbove => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.open_line_above();
                buf.ensure_visible();
                state.mode = Mode::Insert;
            }
        }
        Action::OpenLineBelow => {
            if let Some(buf) = state.focused_buffer_mut() {
                buf.open_line_below();
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
        Action::CommandPromptOpen => {
            state.command_line = Some(CommandLine::new());
        }
        Action::CommandPromptCancel => {
            state.command_line = None;
        }
        Action::CommandPromptBackspace => {
            if let Some(cmd) = state.command_line.as_mut() {
                cmd.input.pop();
            }
        }
        Action::CommandPromptExecute => {
            if let Some(cmd) = state.command_line.take() {
                handle_command_line(state, &cmd.input);
            }
        }
        Action::CommandPromptInput(ch) => {
            if let Some(cmd) = state.command_line.as_mut() {
                cmd.input.push(ch);
            }
        }
        Action::VisualizerReseed => {
            state.visualizer.seed = state.visualizer.seed.wrapping_add(1);
            state.visualizer.pending_reseed = true;
        }
        Action::VisualizerApply => {
            if state.visualizer.seed_search_active {
                state.visualizer.pending_apply = true;
            } else {
                state.visualizer.variant = state.visualizer.variant.wrapping_add(1);
                state.visualizer.pending_reseed = true;
            }
        }
        Action::VisualizerToggleSearch => {
            state.visualizer.seed_search_active = !state.visualizer.seed_search_active;
            state.status = Some(if state.visualizer.seed_search_active {
                "Seed search ON".into()
            } else {
                "Seed search OFF".into()
            });
        }
        Action::VisualizerToggleWrap => {
            state.visualizer.wrap = !state.visualizer.wrap;
        }
        Action::VisualizerToggleSeedSource => {
            state.visualizer.seed_source = match state.visualizer.seed_source {
                GolSeedSource::Editor => GolSeedSource::Notes,
                GolSeedSource::Notes => GolSeedSource::Editor,
            };
            state.visualizer.pending_reseed = true;
            state.status = Some(format!(
                "Seed source: {:?}",
                state.visualizer.seed_source
            ));
        }
        Action::VisualizerSnapshot => {
            state.visualizer.pending_snapshot = true;
        }
        Action::VisualizerPause => {
            state.visualizer.paused = !state.visualizer.paused;
            state.visualizer.paused_by_attractor = false;
        }
        Action::VisualizerCycleAutoStop => {
            state.visualizer.auto_stop_policy = state.visualizer.auto_stop_policy.next();
            state.status = Some(format!(
                "Auto-stop: {}",
                state.visualizer.auto_stop_policy
            ));
        }
        Action::VisualizerSpeedUp => {
            state.visualizer.tick_ms = state.visualizer.tick_ms.saturating_sub(10).max(30);
        }
        Action::VisualizerSpeedDown => {
            state.visualizer.tick_ms = (state.visualizer.tick_ms + 10).min(1000);
        }
        Action::VisualizerRun => {
            state.visualizer.pending_run = true;
            state.visualizer.pending_snapshot = true;
            state.status = Some("Petri dish queued".into());
        }
        Action::VisualizerStop => {
            state.visualizer.pending_close = true;
            state.status = Some("Petri dish closing".into());
        }
        Action::PetriShow => {
            state.visualizer.pending_show = true;
            state.status = Some("Petri dish showing".into());
        }
        Action::VisualizerCycleRenderMode => {
            state.visualizer.seed_plate_mode = state.visualizer.seed_plate_mode.next();
            state.status = Some(format!(
                "Plate mode: {}",
                state.visualizer.seed_plate_mode.label()
            ));
        }
        Action::VisualizerToggleAgeShading => {
            state.visualizer.age_shading = !state.visualizer.age_shading;
            state.status = Some(format!(
                "Age shading: {}",
                if state.visualizer.age_shading { "ON" } else { "OFF" }
            ));
        }
        Action::VisualizerToggleTrails => {
            state.visualizer.trails = !state.visualizer.trails;
            state.status = Some(format!(
                "Trails: {}",
                if state.visualizer.trails { "ON" } else { "OFF" }
            ));
        }
        Action::VisualizerToggleBBox => {
            state.visualizer.overlay_bbox = !state.visualizer.overlay_bbox;
            state.status = Some(format!(
                "BBox: {}",
                if state.visualizer.overlay_bbox { "ON" } else { "OFF" }
            ));
        }
        Action::VisualizerToggleHeat => {
            state.visualizer.overlay_heat = !state.visualizer.overlay_heat;
            state.status = Some(format!(
                "Heat: {}",
                if state.visualizer.overlay_heat { "ON" } else { "OFF" }
            ));
        }
        Action::VisualizerToggleScanlines => {
            state.visualizer.scanlines = !state.visualizer.scanlines;
            state.status = Some(format!(
                "Scanlines: {}",
                if state.visualizer.scanlines { "ON" } else { "OFF" }
            ));
        }
        Action::VisualizerCycleSeedView => {
            state.visualizer.seed_view = state.visualizer.seed_view.next();
            state.status = Some(format!(
                "Seed view: {}",
                state.visualizer.seed_view.label()
            ));
        }
        Action::VisualizerToggleSeedView => {
            state.visualizer.seed_view = state.visualizer.seed_view.toggle_plate();
            state.status = Some(format!(
                "Seed view: {}",
                state.visualizer.seed_view.label()
            ));
        }
        Action::VisualizerCycleSeedOverlays => {
            cycle_seed_overlays(&mut state.visualizer);
            state.status = Some(format!(
                "Overlays: {}",
                seed_overlay_label(&state.visualizer)
            ));
        }
        Action::VisualizerCycleEncoder => {
            state.visualizer.seed_encoder = state.visualizer.seed_encoder.next();
            state.visualizer.pending_reseed = true;
            state.status = Some(format!(
                "Encoder: {}",
                state.visualizer.seed_encoder.label()
            ));
        }
        Action::ToggleSyntax => {
            state.settings.highlight.enabled = !state.settings.highlight.enabled;
        }
        Action::ToggleDebug => {
            state.debug = !state.debug;
            state.status = Some(if state.debug {
                "Debug ON".into()
            } else {
                "Debug OFF".into()
            });
        }
        Action::ShowHelp => state.show_help = true,
        Action::HideHelp => state.show_help = false,
    }

    ActionOutcome {
        should_exit,
        state_changed: changed,
    }
}

fn handle_command_line(state: &mut AppState, input: &str) {
    let cmd = input.trim().to_lowercase();
    if cmd.is_empty() {
        return;
    }
    match cmd.as_str() {
        "gol run" | "run gol" | "life run" | "gol start" | "run life" => {
            state.visualizer.pending_run = true;
            state.visualizer.pending_snapshot = true;
            state.status = Some("Petri dish queued".into());
        }
        "gol hide" | "hide gol" | "petri hide" | "hide petri" => {
            state.visualizer.pending_hide = true;
            state.status = Some("Petri dish hiding".into());
        }
        "gol show" | "show gol" | "petri show" | "show petri" => {
            state.visualizer.pending_show = true;
            state.status = Some("Petri dish showing".into());
        }
        "gol stop" | "run stop" | "life stop" => {
            state.visualizer.pending_close = true;
            state.status = Some("Petri dish closing".into());
        }
        "gol seed" | "seed view" => {
            state.visualizer.seed_view = state.visualizer.seed_view.next();
            state.status = Some(format!(
                "Seed view: {}",
                state.visualizer.seed_view.label()
            ));
        }
        "gol encoder" | "seed encoder" => {
            state.visualizer.seed_encoder = state.visualizer.seed_encoder.next();
            state.visualizer.pending_reseed = true;
            state.status = Some(format!(
                "Encoder: {}",
                state.visualizer.seed_encoder.label()
            ));
        }
        other => {
            state.status = Some(format!("Unknown command: {other}"));
        }
    }
}

fn cycle_seed_overlays(state: &mut VisualizerState) {
    const PRESETS: &[(bool, bool, bool, bool, bool)] = &[
        (false, false, false, false, false),
        (true, false, false, false, false),
        (true, false, true, false, false),
        (true, false, true, true, false),
        (true, true, true, true, false),
        (true, true, true, true, true),
    ];
    let current = (
        state.seed_show_grid,
        state.seed_show_bbox,
        state.seed_show_halo,
        state.seed_show_components,
        state.seed_show_inset,
    );
    let idx = PRESETS
        .iter()
        .position(|preset| *preset == current)
        .unwrap_or(0);
    let next = PRESETS[(idx + 1) % PRESETS.len()];
    state.seed_show_grid = next.0;
    state.seed_show_bbox = next.1;
    state.seed_show_halo = next.2;
    state.seed_show_components = next.3;
    state.seed_show_inset = next.4;
}

fn seed_overlay_label(state: &VisualizerState) -> String {
    let mut parts = Vec::new();
    if state.seed_show_grid {
        parts.push("GRID");
    }
    if state.seed_show_halo {
        parts.push("HALO");
    }
    if state.seed_show_components {
        parts.push("COMP");
    }
    if state.seed_show_bbox {
        parts.push("BBOX");
    }
    if state.seed_show_inset {
        parts.push("INSET");
    }
    if parts.is_empty() {
        "OFF".into()
    } else {
        parts.join("+")
    }
}
