use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nit_core::{
    actions::Action, AppState, EncodedSeed, GolRenderMode, RuleMode, RuleRef, SelectedRule,
    VisualizerMode,
};
use nit_gol::analyze::{evaluate_rule, RuleEvaluation, RuleScore};
use nit_gol::attractor::{AttractorConfig, AttractorDetector, AttractorEvent, AutoStopPolicy};
use nit_gol::snapshot::SnapshotMetadata;
use nit_gol::snapshot_manager::{
    grid_fingerprint, pack_grid_bits, snapshot_queue_capacity, RuleLogEntry, SnapshotEventKind,
    SnapshotManager, SnapshotManagerConfig, SnapshotRequest,
};
use nit_gol::step::step;
use nit_gol::AttractorExtra;
use nit_gol::{EdgeMode, Grid, Rule};
use nit_utils::hashing::SplitMix64;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use tracing::{info, warn};

use crate::gol_render::{grid_size_for_mode, GolHudState, GolPalette, GolRenderConfig, GolWidget};
use crate::seed_runtime::SeedRuntime;
use crate::theme::Theme;
use crate::widgets::{protocol_picker, rule_picker};

const MIN_WIDTH: u16 = 100;
const MIN_HEIGHT: u16 = 30;

pub struct PetriDishRuntime {
    session: Option<SimSession>,
    render_state: crate::gol_render::GolRenderState,
    size: (usize, usize),
    last_step: Instant,
    last_wrap: bool,
    last_tick_ms: u64,
    last_auto_stop_policy: AutoStopPolicy,
    last_render_mode: GolRenderMode,
    rules_log_path: PathBuf,
    leaderboard: Vec<RuleScore>,
    leaderboard_limit: usize,
    best_score: f32,
    search_rps: u32,
    last_attractor_hash: Option<[u64; 2]>,
    search_paused_for_stability: bool,
    search: SearchWorker,
    snapshot: SnapshotManager,
    warning: Option<String>,
    last_mode: VisualizerMode,
    hidden: bool,
}

pub struct SimSession {
    pub seed_hash: u64,
    pub encoder_id: String,
    pub params_fingerprint: u64,
    pub params_summary: String,
    pub input_hash: u64,
    pub rule_mode: RuleMode,
    pub rule: Rule,
    pub gen: u64,
    pub grid: Grid,
    pub paused: bool,
    pub detector: AttractorDetector,
    pub alive: usize,
    pub period: Option<u32>,
    pub last_attractor: Option<AttractorEvent>,
}

impl PetriDishRuntime {
    pub fn new(state: &AppState) -> Self {
        let snapshot_dir = state.workspace_root.join("gol-snapshots");
        let rules_log_path = snapshot_dir.join("rules.ndjson");
        let snapshot_config = SnapshotManagerConfig {
            dir: snapshot_dir,
            max_files: state.settings.gol.snapshots.max_files,
            min_interval_ms: state.settings.gol.snapshots.min_interval_ms,
            queue_capacity: snapshot_queue_capacity(),
        };
        Self {
            session: None,
            render_state: crate::gol_render::GolRenderState::new(),
            size: (0, 0),
            last_step: Instant::now(),
            last_wrap: state.visualizer.wrap,
            last_tick_ms: state.visualizer.tick_ms,
            last_auto_stop_policy: state.visualizer.auto_stop_policy,
            last_render_mode: state
                .visualizer
                .render_mode
                .effective(state.settings.gol.braille_enabled),
            rules_log_path,
            leaderboard: Vec::new(),
            leaderboard_limit: 10,
            best_score: f32::MIN,
            search_rps: 0,
            last_attractor_hash: None,
            search_paused_for_stability: false,
            search: SearchWorker::spawn(),
            snapshot: SnapshotManager::new(snapshot_config),
            warning: None,
            last_mode: state.visualizer.mode,
            hidden: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.session.is_some()
    }

    pub fn is_visible(&self) -> bool {
        self.session.is_some() && !self.hidden
    }

    pub fn handle_pending_requests(
        &mut self,
        state: &mut AppState,
        seed_runtime: &mut SeedRuntime,
        screen: Rect,
    ) {
        if state.visualizer.pending_close {
            state.visualizer.pending_close = false;
            self.close(state);
        }
        if state.visualizer.pending_hide {
            state.visualizer.pending_hide = false;
            self.hide(state);
        }
        if state.visualizer.pending_show {
            state.visualizer.pending_show = false;
            self.show(state);
        }
        if state.visualizer.pending_run {
            state.visualizer.pending_run = false;
            self.open_or_reseed(state, seed_runtime, screen);
        }
        if state.visualizer.pending_rule_change {
            state.visualizer.pending_rule_change = false;
            self.apply_rule_change(state, seed_runtime, screen);
        }
    }

    pub fn handle_key(
        &mut self,
        key: &KeyEvent,
        state: &mut AppState,
        seed_runtime: &mut SeedRuntime,
        screen: Rect,
    ) -> bool {
        if self.warning.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ')) {
                self.warning = None;
                return true;
            }
            return true;
        }
        let Some(session) = self.session.as_mut() else {
            return false;
        };

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R')) {
            self.reseed_from_current(state, seed_runtime, screen);
            return true;
        }
        if ctrl && matches!(key.code, KeyCode::Char('p') | KeyCode::Char('P')) {
            let _ = nit_core::apply_action(state, Action::OpenRulePicker);
            return true;
        }
        if ctrl
            && matches!(
                key.code,
                KeyCode::Enter | KeyCode::Char('\n') | KeyCode::Char('\r')
            )
        {
            if session.paused {
                self.step_once(state);
            }
            return true;
        }

        match key.code {
            KeyCode::Esc => {
                self.close(state);
                true
            }
            KeyCode::F(2) => {
                let _ = nit_core::apply_action(state, Action::OpenRulePicker);
                true
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                let _ = nit_core::apply_action(state, Action::OpenProtocolPicker);
                true
            }
            KeyCode::Char(' ') => {
                session.paused = !session.paused;
                state.visualizer.paused = session.paused;
                state.visualizer.paused_by_attractor = false;
                true
            }
            KeyCode::Enter => {
                if session.paused {
                    self.step_once(state);
                }
                true
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                state.visualizer.tick_ms = state.visualizer.tick_ms.saturating_sub(10).max(30);
                true
            }
            KeyCode::Char('-') => {
                state.visualizer.tick_ms = (state.visualizer.tick_ms + 10).min(1000);
                true
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.queue_snapshot(state, SnapshotTrigger::Manual);
                true
            }
            KeyCode::Char('h') | KeyCode::Char('H') => {
                self.hide(state);
                true
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                state.visualizer.wrap = !state.visualizer.wrap;
                true
            }
            KeyCode::Char('o') | KeyCode::Char('O') => {
                state.visualizer.auto_stop_policy = state.visualizer.auto_stop_policy.next();
                state.status = Some(format!("Auto-stop: {}", state.visualizer.auto_stop_policy));
                true
            }
            KeyCode::Char('g') | KeyCode::Char('G') => {
                if !state.settings.gol.search.enabled {
                    state.status = Some("Rule search disabled (config)".into());
                } else {
                    state.visualizer.mode = match state.visualizer.mode {
                        VisualizerMode::SimOnly => VisualizerMode::Search,
                        VisualizerMode::Search => VisualizerMode::SimOnly,
                    };
                }
                true
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.apply_best_rule(state);
                true
            }
            _ => false,
        }
    }

    pub fn tick(&mut self, state: &mut AppState) {
        self.handle_worker_events(state);
        self.apply_state_changes(state);
        self.step_if_due(state);
        self.sync_state(state);
    }

    pub fn render(&mut self, frame: &mut Frame, screen: Rect, state: &AppState, theme: &Theme) {
        if let Some(message) = &self.warning {
            let area = centered_rect(screen, 70, 20);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border_focused))
                .title("LAB WARNING")
                .style(Style::default().bg(theme.selection_bg));
            let paragraph = Paragraph::new(message.as_str())
                .style(Style::default().bg(theme.selection_bg).fg(theme.foreground))
                .block(block);
            frame.render_widget(Clear, area);
            frame.render_widget(paragraph, area);
            return;
        }
        if self.hidden {
            return;
        }

        let Some(session) = self.session.as_ref() else {
            return;
        };
        let layout = self.layout(screen);
        frame.render_widget(Clear, layout.rect);
        let title = "PETRI DISH — Game of Life Simulator";
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border_focused))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(ratatui::style::Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(theme.background));
        frame.render_widget(block, layout.rect);

        let palette = GolPalette::from_theme(theme);
        let rule_label = state.rule_catalog.label_for_rule(session.rule);
        let hud_metrics = self.render_state.hud_metrics();
        let hud = GolHudState {
            rule: &rule_label,
            generation: session.gen,
            alive: session.alive,
            period: session.period,
            mode: state.visualizer.mode,
            paused: session.paused,
            delta: hud_metrics.delta(),
            history: hud_metrics.history(),
        };
        let cfg = GolRenderConfig {
            mode: state.visualizer.render_mode,
            age_shading: state.visualizer.age_shading,
            trails: state.visualizer.trails,
            overlay_bbox: state.visualizer.overlay_bbox,
            overlay_heat: state.visualizer.overlay_heat,
            scanlines: state.visualizer.scanlines,
            grid_minor: None,
            grid_major: None,
            gol_origin_x: 0,
            gol_origin_y: 0,
            debug_overlay: state.debug,
            braille_enabled: state.settings.gol.braille_enabled,
        };
        let widget = GolWidget {
            grid: &session.grid,
            state: &self.render_state,
            cfg,
            palette,
            hud,
        };
        frame.render_widget(widget, layout.grid);

        render_metrics(frame, layout.metrics, state, theme, session, self);
        render_footer(frame, layout.footer, theme, session, layout.metrics.x);
        if state.rule_picker.open {
            rule_picker::render(frame, screen, state, theme);
        }
        if state.protocol_picker.open {
            protocol_picker::render(frame, screen, state, theme);
        }
    }

    fn open_or_reseed(
        &mut self,
        state: &mut AppState,
        seed_runtime: &mut SeedRuntime,
        screen: Rect,
    ) {
        if screen.width < MIN_WIDTH || screen.height < MIN_HEIGHT {
            self.warning = Some("Terminal too small for Lab Simulator. Resize to run.".to_string());
            return;
        }
        let layout = self.layout(screen);
        let render_mode = state
            .visualizer
            .render_mode
            .effective(state.settings.gol.braille_enabled);
        let grid_inner_height = layout.grid.height.saturating_sub(1) as usize;
        let (grid_w, grid_h) =
            grid_size_for_mode(layout.grid.width as usize, grid_inner_height, render_mode);
        self.ensure_size(grid_w, grid_h, state);
        let Some(seed) = seed_runtime.encode_for_size(state, grid_w, grid_h) else {
            state.status = Some("Seed not ready".into());
            return;
        };
        seed_runtime.snapshot_seed(state, &seed);
        state.visualizer.pending_snapshot = false;
        if self.session.is_some() {
            self.hidden = false;
            state.visualizer.petri_hidden = false;
            self.reseed_with_seed(state, seed);
        } else {
            self.start_session(state, seed);
        }
    }

    fn reseed_from_current(
        &mut self,
        state: &mut AppState,
        seed_runtime: &mut SeedRuntime,
        screen: Rect,
    ) {
        let layout = self.layout(screen);
        let render_mode = state
            .visualizer
            .render_mode
            .effective(state.settings.gol.braille_enabled);
        let grid_inner_height = layout.grid.height.saturating_sub(1) as usize;
        let (grid_w, grid_h) =
            grid_size_for_mode(layout.grid.width as usize, grid_inner_height, render_mode);
        self.ensure_size(grid_w, grid_h, state);
        let Some(seed) = seed_runtime.encode_for_size(state, grid_w, grid_h) else {
            state.status = Some("Seed not ready".into());
            return;
        };
        self.reseed_with_seed(state, seed);
    }

    fn start_session(&mut self, state: &mut AppState, seed: EncodedSeed) {
        let mut rule_mode = state.visualizer.rule_mode.clone();
        rule_mode.reset();
        let rule = rule_mode.current_rule().rule;
        let edge = self.current_edge(state);
        let mut detector = AttractorDetector::new(AttractorConfig {
            policy: state.visualizer.auto_stop_policy,
            ..AttractorConfig::default()
        });
        detector.seed_with_context(&seed.grid, 0, rule, edge, Self::protocol_extra(&rule_mode));
        let alive = self.render_state.seed_from_grid(&seed.grid);
        let session = SimSession {
            seed_hash: seed.seed_hash,
            encoder_id: seed.encoder_id.as_str().to_string(),
            params_fingerprint: seed.params.fingerprint(),
            params_summary: seed.params.summary(),
            input_hash: seed.input_hash,
            rule_mode,
            rule,
            gen: 0,
            grid: seed.grid,
            paused: false,
            detector,
            alive,
            period: None,
            last_attractor: None,
        };
        state.visualizer.rule = session.rule.to_string();
        state.visualizer.running = true;
        state.visualizer.paused = false;
        state.visualizer.paused_by_attractor = false;
        self.hidden = false;
        state.visualizer.petri_hidden = false;
        state.status = Some("Petri dish running".into());
        self.session = Some(session);
        self.last_step = Instant::now();
        if state.visualizer.mode == VisualizerMode::Search {
            self.start_search(state);
        }
    }

    fn reseed_with_seed(&mut self, state: &mut AppState, seed: EncodedSeed) {
        let edge = self.current_edge(state);
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.rule_mode = state.visualizer.rule_mode.clone();
        session.rule_mode.reset();
        session.rule = session.rule_mode.current_rule().rule;
        session.seed_hash = seed.seed_hash;
        session.encoder_id = seed.encoder_id.as_str().to_string();
        session.params_fingerprint = seed.params.fingerprint();
        session.params_summary = seed.params.summary();
        session.input_hash = seed.input_hash;
        session.grid = seed.grid;
        session.gen = 0;
        session.alive = self.render_state.seed_from_grid(&session.grid);
        session.period = None;
        session.last_attractor = None;
        session.detector.reset();
        session.detector.seed_with_context(
            &session.grid,
            session.gen,
            session.rule,
            edge,
            Self::protocol_extra(&session.rule_mode),
        );
        state.visualizer.paused = false;
        state.visualizer.paused_by_attractor = false;
        self.last_attractor_hash = None;
        state.status = Some("Petri dish reseeded".into());
        self.last_step = Instant::now();
        if state.visualizer.mode == VisualizerMode::Search {
            self.restart_search(state);
        }
    }

    fn close(&mut self, state: &mut AppState) {
        self.session = None;
        self.stop_search();
        state.visualizer.mode = VisualizerMode::SimOnly;
        state.visualizer.running = false;
        self.hidden = false;
        state.visualizer.petri_hidden = false;
        state.visualizer.paused = false;
        state.visualizer.paused_by_attractor = false;
        state.rule_picker.open = false;
        state.rule_picker.query.clear();
        state.rule_picker.selected = 0;
        state.protocol_picker.open = false;
        state.protocol_picker.selected = 0;
        state.protocol_picker.custom_input.clear();
        state.protocol_picker.custom_error = None;
        state.protocol_picker.custom_preview = None;
        state.status = Some("Petri dish closed".into());
    }

    fn hide(&mut self, state: &mut AppState) {
        if self.session.is_none() {
            state.status = Some("Petri dish not running".into());
            return;
        }
        self.hidden = true;
        state.visualizer.petri_hidden = true;
        state.status = Some("Petri dish hidden".into());
    }

    fn show(&mut self, state: &mut AppState) {
        if self.session.is_none() {
            state.status = Some("Petri dish not running".into());
            return;
        }
        self.hidden = false;
        state.visualizer.petri_hidden = false;
        state.status = Some("Petri dish shown".into());
    }

    fn ensure_size(&mut self, width: usize, height: usize, state: &mut AppState) {
        if self.size == (width, height) {
            return;
        }
        self.size = (width, height);
        self.render_state.resize(width, height);
        let edge = self.current_edge(state);
        if let Some(session) = self.session.as_mut() {
            session.grid = session.grid.clone_with_size(width, height);
            session.alive = self.render_state.seed_from_grid(&session.grid);
            session.detector.reset();
            session.detector.seed_with_context(
                &session.grid,
                session.gen,
                session.rule,
                edge,
                Self::protocol_extra(&session.rule_mode),
            );
        }
    }

    fn apply_state_changes(&mut self, state: &mut AppState) {
        if state.visualizer.mode != self.last_mode {
            self.last_mode = state.visualizer.mode;
            match state.visualizer.mode {
                VisualizerMode::Search => self.start_search(state),
                VisualizerMode::SimOnly => self.stop_search(),
            }
        }

        if state.visualizer.wrap != self.last_wrap {
            self.last_wrap = state.visualizer.wrap;
            let edge = self.current_edge(state);
            if let Some(session) = self.session.as_mut() {
                session.period = None;
                session.last_attractor = None;
                session.detector.reset();
                session.detector.seed_with_context(
                    &session.grid,
                    session.gen,
                    session.rule,
                    edge,
                    Self::protocol_extra(&session.rule_mode),
                );
            }
            if state.visualizer.mode == VisualizerMode::Search {
                self.restart_search(state);
            }
        }

        if state.visualizer.auto_stop_policy != self.last_auto_stop_policy {
            self.last_auto_stop_policy = state.visualizer.auto_stop_policy;
            if let Some(session) = self.session.as_mut() {
                session.detector.set_policy(self.last_auto_stop_policy);
            }
        }

        if state.visualizer.tick_ms != self.last_tick_ms {
            self.last_tick_ms = state.visualizer.tick_ms;
        }

        let render_mode = state
            .visualizer
            .render_mode
            .effective(state.settings.gol.braille_enabled);
        if render_mode != self.last_render_mode {
            self.last_render_mode = render_mode;
        }
    }

    fn step_if_due(&mut self, state: &mut AppState) {
        let edge = self.current_edge(state);
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if session.paused || self.size.0 == 0 || self.size.1 == 0 {
            return;
        }
        let interval = Duration::from_millis(state.visualizer.tick_ms.max(10));
        if self.last_step.elapsed() < interval {
            return;
        }
        let current_rule = session.rule_mode.current_rule().rule;
        let next = step(&session.grid, current_rule, edge);
        let next_gen = session.gen.saturating_add(1);
        session.rule_mode.advance_one_gen();
        let next_rule = session.rule_mode.current_rule().rule;
        let event = session.detector.observe_with_context(
            &session.grid,
            &next,
            next_gen,
            next_rule,
            edge,
            Self::protocol_extra(&session.rule_mode),
        );
        let (alive, _) = self.render_state.update_from_step(&session.grid, &next);
        session.grid = next;
        session.gen = next_gen;
        session.alive = alive;
        session.rule = next_rule;
        if let Some(event) = event {
            self.handle_attractor_event(state, event);
        }
        self.last_step = Instant::now();
    }

    fn step_once(&mut self, state: &mut AppState) {
        let edge = self.current_edge(state);
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let current_rule = session.rule_mode.current_rule().rule;
        let next = step(&session.grid, current_rule, edge);
        let next_gen = session.gen.saturating_add(1);
        session.rule_mode.advance_one_gen();
        let next_rule = session.rule_mode.current_rule().rule;
        let event = session.detector.observe_with_context(
            &session.grid,
            &next,
            next_gen,
            next_rule,
            edge,
            Self::protocol_extra(&session.rule_mode),
        );
        let (alive, _) = self.render_state.update_from_step(&session.grid, &next);
        session.grid = next;
        session.gen = next_gen;
        session.alive = alive;
        session.rule = next_rule;
        if let Some(event) = event {
            self.handle_attractor_event(state, event);
        }
        self.last_step = Instant::now();
    }

    fn sync_state(&mut self, state: &mut AppState) {
        if let Some(session) = self.session.as_ref() {
            if !state.visualizer.pending_rule_change {
                state.visualizer.rule = session.rule.to_string();
                state.visualizer.rule_mode = session.rule_mode.clone();
            }
            state.visualizer.generation = session.gen;
            state.visualizer.alive = session.alive;
            state.visualizer.period = session.period;
            state.visualizer.last_attractor = session.last_attractor.clone();
            state.visualizer.search_rps = self.search_rps;
            state.visualizer.leaderboard = self
                .leaderboard
                .iter()
                .map(|entry| nit_core::VisualizerRuleEntry {
                    rule: entry.rule.to_string(),
                    score: entry.score,
                    period: entry.period,
                })
                .collect();
            let stats = self.snapshot.stats();
            state.visualizer.snapshots_written = stats.written;
            state.visualizer.snapshots_dropped = stats.dropped;
            state.visualizer.snapshot_queue_depth = stats.queue_len;
            state.visualizer.last_snapshot_path =
                stats.last_path.map(|path| path.display().to_string());
            state.visualizer.running = true;
            state.visualizer.paused = session.paused;
        } else {
            state.visualizer.running = false;
        }
    }

    fn current_edge(&self, state: &AppState) -> EdgeMode {
        if state.visualizer.wrap {
            EdgeMode::Toroid
        } else {
            EdgeMode::Dead
        }
    }

    fn protocol_extra(rule_mode: &RuleMode) -> Option<AttractorExtra> {
        match rule_mode {
            RuleMode::Protocol(protocol) => Some(AttractorExtra {
                protocol_hash: protocol.hash(),
                phase_idx: protocol.phase_idx as u32,
                step_in_phase: protocol.step_in_phase,
            }),
            _ => None,
        }
    }

    fn protocol_snapshot_string(rule_mode: &RuleMode) -> Option<String> {
        match rule_mode {
            RuleMode::Protocol(protocol) => Some(protocol.canonical_string()),
            _ => None,
        }
    }

    fn protocol_snapshot_hash(rule_mode: &RuleMode) -> Option<u64> {
        match rule_mode {
            RuleMode::Protocol(protocol) => Some(protocol.hash()),
            _ => None,
        }
    }

    fn protocol_snapshot_phase(rule_mode: &RuleMode) -> Option<(u32, u32)> {
        match rule_mode {
            RuleMode::Protocol(protocol) => {
                Some((protocol.phase_idx as u32, protocol.step_in_phase))
            }
            _ => None,
        }
    }

    fn handle_attractor_event(&mut self, state: &mut AppState, event: AttractorEvent) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let period = match &event {
            AttractorEvent::FixedPoint { .. } => Some(1u64),
            AttractorEvent::Cycle { period, .. } => Some(*period),
        };
        session.period = period.map(|value| value.min(u32::MAX as u64) as u32);
        session.last_attractor = Some(event.clone());

        let log_line = match &event {
            AttractorEvent::FixedPoint { gen } => format!("Fixed point at gen={gen}"),
            AttractorEvent::Cycle {
                gen,
                period,
                transient,
                ..
            } => format!("Cycle detected: transient={transient}, period={period}, gen={gen}"),
        };
        state.receive_log(log_line);

        let should_pause = state.visualizer.auto_stop_policy.should_stop(&event);
        if should_pause {
            session.paused = true;
            state.visualizer.paused = true;
            state.visualizer.paused_by_attractor = true;
            let protocol_phase = match &session.rule_mode {
                RuleMode::Protocol(protocol) => {
                    let phase_label = protocol
                        .current_phase()
                        .label
                        .clone()
                        .unwrap_or_else(|| "Phase".into());
                    Some(format!(
                        "phase {}/{} \"{}\" t={}/{}",
                        protocol.phase_idx + 1,
                        protocol.phase_count(),
                        phase_label,
                        protocol.step_in_phase + 1,
                        protocol.current_phase().steps.max(1)
                    ))
                }
                _ => None,
            };
            state.status = Some(match &event {
                AttractorEvent::FixedPoint { gen } => {
                    match protocol_phase {
                        Some(phase) => format!("Petri paused (fixed point at gen={gen}, {phase})"),
                        None => format!("Petri paused (fixed point at gen={gen})"),
                    }
                }
                AttractorEvent::Cycle { period, transient, gen, .. } => {
                    match protocol_phase {
                        Some(phase) => format!(
                            "Petri paused (cycle p={period} t={transient} gen={gen}, includes protocol phase; {phase})"
                        ),
                        None => format!("Petri paused (cycle p={period} t={transient} gen={gen})"),
                    }
                }
            });
        }

        let grid_hash = grid_fingerprint(&session.grid);
        let is_new_attractor = self.last_attractor_hash != Some(grid_hash);
        self.last_attractor_hash = Some(grid_hash);

        let snapshot_event = event.clone();
        if is_new_attractor
            && (should_pause || attractor_snapshots_enabled(&state.settings.gol.snapshots, &event))
        {
            self.queue_snapshot(state, SnapshotTrigger::Attractor(snapshot_event));
        }

        if matches!(event, AttractorEvent::FixedPoint { .. })
            && state.visualizer.mode == VisualizerMode::Search
            && !self.search_paused_for_stability
        {
            self.search.send(SearchCommand::StopSearch);
            self.search_rps = 0;
            self.search_paused_for_stability = true;
            if !should_pause {
                state.status = Some("Rule search paused (stable)".into());
            }
        }
    }

    fn start_search(&mut self, state: &AppState) {
        self.search_paused_for_stability = false;
        let config = SearchConfig::from_settings(&state.settings.gol.search, state.visualizer.wrap);
        self.search_rps = config.rules_per_second;
        self.leaderboard_limit = config.leaderboard_size;
        let Some(session) = self.session.as_ref() else {
            return;
        };
        self.leaderboard.clear();
        self.best_score = f32::MIN;
        self.search.send(SearchCommand::StartSearch {
            config,
            seed: session.grid.clone(),
            base_rule: session.rule,
        });
    }

    fn restart_search(&mut self, state: &AppState) {
        self.search.send(SearchCommand::StopSearch);
        self.start_search(state);
    }

    fn stop_search(&mut self) {
        self.search.send(SearchCommand::StopSearch);
        self.search_rps = 0;
        self.search_paused_for_stability = false;
    }

    fn apply_best_rule(&mut self, state: &mut AppState) {
        if let Some(best) = self.leaderboard.first() {
            let edge = self.current_edge(state);
            if let Some(session) = self.session.as_mut() {
                let mut rule_ref = RuleRef {
                    id: None,
                    rule: best.rule,
                    name: None,
                };
                if let Some(named) = state.rule_catalog.find_by_rule(best.rule) {
                    rule_ref.id = Some(named.id.clone());
                    rule_ref.name = Some(named.name.clone());
                }
                session.rule_mode = RuleMode::Fixed(rule_ref.clone());
                session.rule = rule_ref.rule;
                info!("Applying best rule {} score={:.2}", best.rule, best.score);
                session.gen = 0;
                session.period = None;
                session.last_attractor = None;
                session.detector.reset();
                session.detector.seed_with_context(
                    &session.grid,
                    session.gen,
                    session.rule,
                    edge,
                    Self::protocol_extra(&session.rule_mode),
                );
                let mut selected = SelectedRule::from_rule(rule_ref.rule);
                if let Some(named) = state.rule_catalog.find_by_rule(rule_ref.rule) {
                    selected.id = Some(named.id.clone());
                    selected.name = Some(named.name.clone());
                } else {
                    selected.id = rule_ref.id;
                    selected.name = rule_ref.name;
                }
                state.gol_rule_selected = selected;
                state.visualizer.rule_mode = session.rule_mode.clone();
                state.visualizer.rule = session.rule.to_string();
                state.visualizer.protocol_name = None;
                state.status = Some(format!("Rule applied {}", session.rule));
            }
        }
    }

    fn handle_worker_events(&mut self, state: &mut AppState) {
        while let Ok(event) = self.search.events.try_recv() {
            match event {
                WorkerEvent::BestRule(eval) => {
                    self.best_score = eval.score;
                    self.upsert_leaderboard(&eval);
                    info!(
                        "New best rule {} score={:.2} period={:?}",
                        eval.rule, eval.score, eval.period
                    );
                    self.queue_best_rule_snapshot(state, &eval);
                    if !self.snapshot.record_rule(RuleLogEntry::from_eval(
                        &eval,
                        self.session.as_ref().map(|s| s.seed_hash).unwrap_or(0),
                        &self.rules_log_path,
                    )) {
                        warn!("Snapshot queue full; dropping rule log entry");
                    }
                }
                WorkerEvent::Leaderboard(entries) => {
                    self.leaderboard = entries;
                }
            }
        }
    }

    fn upsert_leaderboard(&mut self, eval: &RuleEvaluation) {
        let entry = RuleScore {
            rule: eval.rule,
            score: eval.score,
            period: eval.period,
            transient: eval.transient,
            avg_population: eval.avg_population,
            max_population: eval.max_population,
            alive_end: eval.alive_end,
        };
        self.leaderboard.retain(|e| e.rule != eval.rule);
        self.leaderboard.push(entry);
        self.leaderboard.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if self.leaderboard.len() > self.leaderboard_limit {
            self.leaderboard.truncate(self.leaderboard_limit);
        }
    }

    fn queue_snapshot(&mut self, state: &mut AppState, trigger: SnapshotTrigger) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        if !state.settings.gol.snapshots.enabled {
            return;
        }
        if let SnapshotTrigger::Attractor(event) = &trigger {
            let min_period = state.settings.gol.snapshots.min_period as u64;
            let min_transient = state.settings.gol.snapshots.min_transient as u64;
            match event {
                AttractorEvent::FixedPoint { .. } => {
                    if session.gen < min_transient {
                        return;
                    }
                }
                AttractorEvent::Cycle { period, .. } => {
                    if *period < min_period {
                        return;
                    }
                }
            }
        }
        let grid = session.grid.clone();
        if grid.width() > u16::MAX as usize || grid.height() > u16::MAX as usize {
            warn!(
                "Snapshot skipped; grid too large ({}x{})",
                grid.width(),
                grid.height()
            );
            return;
        }
        let grid_hash = grid_fingerprint(&grid);
        let grid_bits = pack_grid_bits(&grid);
        let event_kind = match &trigger {
            SnapshotTrigger::Manual => SnapshotEventKind::Manual,
            SnapshotTrigger::Attractor(AttractorEvent::FixedPoint { .. }) => {
                SnapshotEventKind::FixedPoint
            }
            SnapshotTrigger::Attractor(AttractorEvent::Cycle { .. }) => SnapshotEventKind::Cycle,
        };
        let meta = self.build_snapshot_meta(
            state,
            session,
            session.gen,
            session.period.map(|v| v as u64),
            session.alive,
            None,
            session.last_attractor.clone(),
        );
        let req = SnapshotRequest {
            event: event_kind,
            timestamp: std::time::SystemTime::now(),
            gen: session.gen,
            rule: session.rule.to_string(),
            width: grid.width() as u16,
            height: grid.height() as u16,
            wrap: if state.visualizer.wrap {
                EdgeMode::Toroid
            } else {
                EdgeMode::Dead
            },
            seed_hash: session.seed_hash,
            grid_hash,
            grid_bits,
            period: session.period.map(|value| value as u64),
            transient: match &trigger {
                SnapshotTrigger::Attractor(AttractorEvent::Cycle { transient, .. }) => {
                    Some(*transient)
                }
                _ => None,
            },
            score: None,
            meta,
        };
        let enqueued = self.snapshot.enqueue(req);
        if !enqueued && matches!(trigger, SnapshotTrigger::Manual) {
            state.status = Some("Sim snapshot dropped".into());
        }
    }

    fn queue_best_rule_snapshot(&mut self, state: &mut AppState, eval: &RuleEvaluation) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        if !state.settings.gol.snapshots.enabled {
            return;
        }
        let final_grid = eval.final_grid.clone();
        if final_grid.width() > u16::MAX as usize || final_grid.height() > u16::MAX as usize {
            warn!(
                "Snapshot skipped; grid too large ({}x{})",
                final_grid.width(),
                final_grid.height()
            );
            return;
        }
        let meta = self.build_snapshot_meta(
            state,
            session,
            eval.transient as u64,
            eval.period.map(|v| v as u64),
            eval.alive_end as usize,
            Some(eval.score),
            None,
        );
        let req = SnapshotRequest {
            event: SnapshotEventKind::NewBestRule,
            timestamp: std::time::SystemTime::now(),
            gen: eval.transient as u64,
            rule: eval.rule.to_string(),
            width: final_grid.width() as u16,
            height: final_grid.height() as u16,
            wrap: if state.visualizer.wrap {
                EdgeMode::Toroid
            } else {
                EdgeMode::Dead
            },
            seed_hash: session.seed_hash,
            grid_hash: grid_fingerprint(&final_grid),
            grid_bits: pack_grid_bits(&final_grid),
            period: eval.period.map(|value| value as u64),
            transient: Some(eval.transient as u64),
            score: Some(eval.score),
            meta,
        };
        let _ = self.snapshot.enqueue(req);
    }

    #[allow(clippy::too_many_arguments)]
    fn build_snapshot_meta(
        &self,
        state: &AppState,
        session: &SimSession,
        generation: u64,
        period: Option<u64>,
        alive: usize,
        score: Option<f32>,
        attractor: Option<AttractorEvent>,
    ) -> SnapshotMetadata {
        SnapshotMetadata {
            timestamp: nit_gol::snapshot::now_iso8601(),
            workspace_root: Some(state.workspace_root.display().to_string()),
            file_path: state
                .editor_buffer()
                .path()
                .map(|p| p.display().to_string()),
            seed_source: format!("{:?}", state.visualizer.seed_source),
            seed_hash: session.seed_hash,
            rule: session.rule.to_string(),
            rule_id: state
                .rule_catalog
                .find_by_rule(session.rule)
                .map(|rule| rule.id.clone()),
            protocol: Self::protocol_snapshot_string(&session.rule_mode),
            protocol_hash: Self::protocol_snapshot_hash(&session.rule_mode),
            protocol_phase_idx: Self::protocol_snapshot_phase(&session.rule_mode).map(|v| v.0),
            protocol_step_in_phase: Self::protocol_snapshot_phase(&session.rule_mode).map(|v| v.1),
            generation,
            alive_count: alive,
            period,
            score,
            wrap_mode: if state.visualizer.wrap {
                "toroid".into()
            } else {
                "dead".into()
            },
            tick_ms: state.visualizer.tick_ms,
            attractor,
            encoder_id: Some(session.encoder_id.clone()),
            encoder_params: Some(session.params_summary.clone()),
            params_fingerprint: Some(session.params_fingerprint),
            input_hash: Some(session.input_hash),
            seed_density: Some(state.visualizer.seed_stats.density),
            seed_components: Some(state.visualizer.seed_stats.components),
        }
    }

    fn layout(&self, screen: Rect) -> PetriLayout {
        let rect = centered_rect(screen, 92, 90);
        let inner = Rect {
            x: rect.x.saturating_add(1),
            y: rect.y.saturating_add(1),
            width: rect.width.saturating_sub(2),
            height: rect.height.saturating_sub(2),
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(2)])
            .split(inner);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(90), Constraint::Percentage(10)])
            .split(rows[0]);
        PetriLayout {
            rect,
            grid: cols[0],
            metrics: cols[1],
            footer: rows[1],
        }
    }

    fn apply_rule_change(
        &mut self,
        state: &mut AppState,
        seed_runtime: &mut SeedRuntime,
        screen: Rect,
    ) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.rule_mode = state.visualizer.rule_mode.clone();
        session.rule_mode.reset();
        session.rule = session.rule_mode.current_rule().rule;
        self.reseed_from_current(state, seed_runtime, screen);
    }
}

impl Drop for PetriDishRuntime {
    fn drop(&mut self) {
        self.search.send(SearchCommand::Shutdown);
        self.search.join();
        self.snapshot.shutdown();
    }
}

#[derive(Clone)]
struct PetriLayout {
    rect: Rect,
    grid: Rect,
    metrics: Rect,
    footer: Rect,
}

fn centered_rect(screen: Rect, pct_w: u16, pct_h: u16) -> Rect {
    let w = screen.width.saturating_mul(pct_w) / 100;
    let h = screen.height.saturating_mul(pct_h) / 100;
    let x = screen.x + screen.width.saturating_sub(w) / 2;
    let y = screen.y + screen.height.saturating_sub(h) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

fn render_metrics(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    session: &SimSession,
    runtime: &PetriDishRuntime,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let label = Style::default().fg(theme.title).add_modifier(Modifier::DIM);
    let value = Style::default().fg(theme.foreground);
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Seed", label),
        Span::raw(" "),
        Span::styled(format!("{:08x}", session.seed_hash as u32), value),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Enc", label),
        Span::raw(" "),
        Span::styled(session.encoder_id.clone(), value),
    ]));
    let rule_label = state.rule_catalog.label_for_rule(session.rule);
    lines.push(Line::from(vec![
        Span::styled("Rule", label),
        Span::raw(" "),
        Span::styled(rule_label, value),
    ]));
    if let RuleMode::Protocol(protocol) = &session.rule_mode {
        let proto_name = state
            .visualizer
            .protocol_name
            .clone()
            .unwrap_or_else(|| "Protocol".into());
        let phase = protocol.current_phase();
        let phase_label = phase.label.clone().unwrap_or_else(|| "Phase".into());
        let phase_idx = protocol.phase_idx + 1;
        let phase_total = protocol.phase_count();
        let step = protocol.step_in_phase.saturating_add(1);
        let phase_steps = phase.steps.max(1);
        lines.push(Line::from(vec![
            Span::styled("Proto", label),
            Span::raw(" "),
            Span::styled(
                format!(
                    "{proto_name} | Phase {phase_idx}/{phase_total} \"{phase_label}\" | t={step}/{phase_steps}"
                ),
                value,
            ),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("Gen", label),
        Span::raw(" "),
        Span::styled(session.gen.to_string(), value),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Alive", label),
        Span::raw(" "),
        Span::styled(session.alive.to_string(), value),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Wrap", label),
        Span::raw(" "),
        Span::styled(
            if state.visualizer.wrap {
                "Torus"
            } else {
                "Dead"
            },
            value,
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("AutoStop", label),
        Span::raw(" "),
        Span::styled(state.visualizer.auto_stop_policy.to_string(), value),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Search", label),
        Span::raw(" "),
        Span::styled(
            if state.visualizer.mode == VisualizerMode::Search {
                "ON"
            } else {
                "OFF"
            },
            value,
        ),
    ]));
    if !runtime.leaderboard.is_empty() {
        lines.push(Line::from(vec![Span::styled("Top Rules", label)]));
        for entry in runtime.leaderboard.iter().take(3) {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(entry.rule.to_string(), value),
                Span::raw(" "),
                Span::styled(format!("{:.1}", entry.score), label),
            ]));
        }
    }
    let para = Paragraph::new(lines)
        .style(Style::default().bg(theme.background).fg(theme.foreground))
        .block(
            Block::default()
                .borders(Borders::LEFT)
                .border_style(Style::default().fg(theme.border)),
        );
    frame.render_widget(para, area);
}

fn render_footer(frame: &mut Frame, area: Rect, theme: &Theme, session: &SimSession, split_x: u16) {
    if area.height == 0 {
        return;
    }
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let status = if session.paused { "PAUSED" } else { "RUNNING" };
    let line = Line::from(vec![
        Span::styled(
            format!("{status}  "),
            Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Esc close | Space pause | Enter step | S snapshot | Ctrl+R reseed | H hide | F2/Ctrl+P rules | P protocols | G search | +/- speed | T wrap | O auto-stop",
            Style::default().fg(theme.border).add_modifier(Modifier::DIM),
        ),
    ]);
    let para =
        Paragraph::new(line).style(Style::default().bg(theme.background).fg(theme.foreground));
    frame.render_widget(para, inner);

    draw_footer_separator(
        frame.buffer_mut(),
        area,
        split_x,
        theme.border,
        theme.background,
    );
}

fn draw_footer_separator(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    split_x: u16,
    color: ratatui::style::Color,
    background: ratatui::style::Color,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if split_x <= area.x || split_x >= area.x.saturating_add(area.width) {
        return;
    }
    let style = Style::default().fg(color).bg(background);
    let top = area.y;
    let bottom = area.y.saturating_add(area.height);
    for y in top..bottom {
        let ch = if y == top { '┬' } else { '│' };
        let cell = buf.get_mut(split_x, y);
        cell.set_char(ch);
        cell.set_style(style);
    }
}

#[derive(Clone, Debug)]
enum SnapshotTrigger {
    Manual,
    Attractor(AttractorEvent),
}

fn attractor_snapshots_enabled(
    settings: &nit_core::GolSnapshotsConfig,
    event: &AttractorEvent,
) -> bool {
    if settings.snapshot_on_attractor {
        return true;
    }
    if matches!(event, AttractorEvent::Cycle { .. })
        && std::env::var_os("NIT_SNAPSHOT_CYCLE").is_some()
    {
        return true;
    }
    false
}

#[derive(Clone)]
struct SearchConfig {
    rules_per_second: u32,
    max_generations: u32,
    leaderboard_size: usize,
    wrap: bool,
    time_budget_ms_per_tick: u32,
    candidate_pool_size: usize,
}

impl SearchConfig {
    fn from_settings(settings: &nit_core::GolSearchConfig, wrap: bool) -> Self {
        let rules_per_second = if settings.rules_per_second > 0 {
            settings.rules_per_second
        } else {
            match settings.intensity {
                nit_core::GolSearchIntensity::Low => 10,
                nit_core::GolSearchIntensity::Med => 30,
                nit_core::GolSearchIntensity::High => 80,
            }
        };
        Self {
            rules_per_second,
            max_generations: settings.max_generations,
            leaderboard_size: settings.leaderboard_size,
            wrap,
            time_budget_ms_per_tick: settings.time_budget_ms_per_tick,
            candidate_pool_size: settings.candidate_pool_size,
        }
    }
}

struct SearchWorker {
    tx: Sender<SearchCommand>,
    handle: Option<JoinHandle<()>>,
    events: Receiver<WorkerEvent>,
}

impl SearchWorker {
    fn spawn() -> Self {
        let (tx, cmd_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("nit-gol-search".into())
            .stack_size(search_worker_stack_bytes())
            .spawn(move || search_worker_loop(cmd_rx, event_tx))
            .expect("spawn search worker");
        Self {
            tx,
            handle: Some(handle),
            events,
        }
    }

    fn send(&self, cmd: SearchCommand) {
        let _ = self.tx.send(cmd);
    }

    fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn search_worker_stack_bytes() -> usize {
    worker_stack_bytes("NIT_GOL_STACK_MB", 256, 32)
}

fn worker_stack_bytes(env_key: &str, default_mb: usize, min_mb: usize) -> usize {
    let from_env = std::env::var(env_key)
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    let mb = from_env.unwrap_or(default_mb).max(min_mb);
    mb.saturating_mul(1024 * 1024)
}

enum SearchCommand {
    StartSearch {
        config: SearchConfig,
        seed: Grid,
        base_rule: Rule,
    },
    StopSearch,
    Shutdown,
}

enum WorkerEvent {
    BestRule(RuleEvaluation),
    Leaderboard(Vec<RuleScore>),
}

fn search_worker_loop(cmd_rx: Receiver<SearchCommand>, event_tx: Sender<WorkerEvent>) {
    let mut search_active = false;
    let mut config = SearchConfig {
        rules_per_second: 10,
        max_generations: 200,
        leaderboard_size: 10,
        wrap: false,
        time_budget_ms_per_tick: 8,
        candidate_pool_size: 8,
    };
    let mut seed = Grid::new(0, 0);
    let mut rng = SplitMix64::new(0x5eed1234);
    let mut leaderboard: Vec<RuleScore> = Vec::new();
    let mut best_score = f32::MIN;
    let mut base_rule = Rule::conway();
    loop {
        if !search_active {
            match cmd_rx.recv() {
                Ok(cmd) => {
                    if handle_search_command(
                        cmd,
                        &mut search_active,
                        &mut config,
                        &mut seed,
                        &mut leaderboard,
                        &mut best_score,
                        &mut base_rule,
                        &event_tx,
                    ) {
                        break;
                    }
                }
                Err(_) => break,
            }
            continue;
        }

        while let Ok(cmd) = cmd_rx.try_recv() {
            if handle_search_command(
                cmd,
                &mut search_active,
                &mut config,
                &mut seed,
                &mut leaderboard,
                &mut best_score,
                &mut base_rule,
                &event_tx,
            ) {
                return;
            }
        }

        let start = Instant::now();
        let budget = Duration::from_millis(config.time_budget_ms_per_tick.max(1) as u64);
        let max_rules = config.candidate_pool_size.max(1);
        let mut evaluated = 0usize;
        while evaluated < max_rules {
            let rule = sample_rule(&mut rng, base_rule);
            let eval = evaluate_rule(
                &seed,
                rule,
                if config.wrap {
                    EdgeMode::Toroid
                } else {
                    EdgeMode::Dead
                },
                config.max_generations,
            );
            evaluated += 1;
            if eval.score > best_score {
                best_score = eval.score;
                let _ = event_tx.send(WorkerEvent::BestRule(eval.clone()));
            }
            leaderboard.push(RuleScore {
                rule: eval.rule,
                score: eval.score,
                period: eval.period,
                transient: eval.transient,
                avg_population: eval.avg_population,
                max_population: eval.max_population,
                alive_end: eval.alive_end,
            });
            if start.elapsed() >= budget {
                break;
            }
        }

        leaderboard.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        leaderboard.dedup_by(|a, b| a.rule == b.rule);
        if leaderboard.len() > config.leaderboard_size {
            leaderboard.truncate(config.leaderboard_size);
        }
        let _ = event_tx.send(WorkerEvent::Leaderboard(leaderboard.clone()));

        let elapsed = start.elapsed();
        let target = Duration::from_millis(
            (1000u64.saturating_mul(evaluated as u64) / config.rules_per_second.max(1) as u64)
                .max(1),
        );
        let target = if target > budget { target } else { budget };
        if elapsed < target {
            thread::sleep(target - elapsed);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_search_command(
    cmd: SearchCommand,
    search_active: &mut bool,
    config: &mut SearchConfig,
    seed: &mut Grid,
    leaderboard: &mut Vec<RuleScore>,
    best_score: &mut f32,
    base_rule: &mut Rule,
    event_tx: &Sender<WorkerEvent>,
) -> bool {
    match cmd {
        SearchCommand::StartSearch {
            config: next,
            seed: next_seed,
            base_rule: next_rule,
        } => {
            *config = next;
            *seed = next_seed;
            *base_rule = next_rule;
            *leaderboard = Vec::new();
            *best_score = f32::MIN;
            *search_active = true;
        }
        SearchCommand::StopSearch => {
            *search_active = false;
            let _ = event_tx.send(WorkerEvent::Leaderboard(Vec::new()));
        }
        SearchCommand::Shutdown => return true,
    }
    false
}

fn sample_rule(rng: &mut SplitMix64, _base_rule: Rule) -> Rule {
    let births = rng.next_u64() as u32;
    let survives = (rng.next_u64() >> 32) as u32;
    Rule::new((births & 0x1ff) as u16, (survives & 0x1ff) as u16)
}
