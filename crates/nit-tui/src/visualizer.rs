use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use nit_core::{
    AppState, GolRenderMode, GolSearchConfig, GolSearchIntensity, GolSeedSource,
    GolSnapshotsConfig, RuleMode, RuleRef, SelectedRule, VisualizerMode, VisualizerRuleEntry,
};
use nit_gol::{
    analyze::{evaluate_rule, RuleEvaluation, RuleScore},
    attractor::{AttractorConfig, AttractorDetector, AttractorEvent, AutoStopPolicy},
    snapshot::{now_iso8601, SnapshotMetadata},
    snapshot_manager::{
        grid_fingerprint, pack_grid_bits, snapshot_queue_capacity, RuleLogEntry,
        SnapshotEventKind, SnapshotManager, SnapshotManagerConfig, SnapshotRequest,
    },
    step::step,
    EdgeMode, Grid, Rule, AttractorExtra,
};
use nit_utils::hashing::{stable_hash_bytes, SplitMix64};
use tracing::{info, warn};

use crate::gol_render::GolRenderState;

const DEFAULT_LIVE_CHARS: &[char] = &['#', '@', '█', '▓', '▒', '░', '*', '+', 'x', 'X', '%', '&'];
const ASCII_SEED_LIVE_MIN: u8 = 6;

pub struct VisualizerRuntime {
    size: (usize, usize),
    grid: Grid,
    rule_mode: RuleMode,
    rule: Rule,
    generation: u64,
    alive: usize,
    period: Option<u32>,
    last_step: Instant,
    last_seed_hash: u64,
    last_mode: VisualizerMode,
    last_wrap: bool,
    last_tick_ms: u64,
    last_seed_source: GolSeedSource,
    last_auto_stop_policy: AutoStopPolicy,
    last_render_mode: GolRenderMode,
    rules_log_path: PathBuf,
    leaderboard: Vec<RuleScore>,
    leaderboard_limit: usize,
    best_score: f32,
    search_rps: u32,
    last_attractor: Option<AttractorEvent>,
    last_attractor_hash: Option<[u64; 2]>,
    attractor: AttractorDetector,
    search_paused_for_stability: bool,
    render_state: GolRenderState,
    title_cache: String,
    search: SearchWorker,
    snapshot: SnapshotManager,
    events: Receiver<WorkerEvent>,
}

impl VisualizerRuntime {
    pub fn new(state: &AppState) -> Self {
        let mut rule_mode = state.visualizer.rule_mode.clone();
        rule_mode.reset();
        let rule = rule_mode.current_rule().rule;
        let snapshot_dir = state.workspace_root.join("gol-snapshots");
        let rules_log_path = snapshot_dir.join("rules.ndjson");
        let (event_tx, events) = mpsc::channel();
        let attractor = AttractorDetector::new(AttractorConfig {
            policy: state.visualizer.auto_stop_policy,
            ..AttractorConfig::default()
        });
        let snapshot_config = SnapshotManagerConfig {
            dir: snapshot_dir.clone(),
            max_files: state.settings.gol.snapshots.max_files,
            min_interval_ms: state.settings.gol.snapshots.min_interval_ms,
            queue_capacity: snapshot_queue_capacity(),
        };
        let render_mode = state
            .visualizer
            .render_mode
            .effective(state.settings.gol.braille_enabled);
        let mut title_cache = String::with_capacity(64);
        build_title(&mut title_cache, render_mode);
        Self {
            size: (0, 0),
            grid: Grid::new(0, 0),
            rule_mode,
            rule,
            generation: 0,
            alive: 0,
            period: None,
            last_step: Instant::now(),
            last_seed_hash: 0,
            last_mode: state.visualizer.mode,
            last_wrap: state.visualizer.wrap,
            last_tick_ms: state.visualizer.tick_ms,
            last_seed_source: state.visualizer.seed_source,
            last_auto_stop_policy: state.visualizer.auto_stop_policy,
            last_render_mode: render_mode,
            rules_log_path,
            leaderboard: Vec::new(),
            leaderboard_limit: 10,
            best_score: f32::MIN,
            search_rps: 0,
            last_attractor: None,
            last_attractor_hash: None,
            attractor,
            search_paused_for_stability: false,
            render_state: GolRenderState::new(),
            title_cache,
            search: SearchWorker::spawn(event_tx.clone()),
            snapshot: SnapshotManager::new(snapshot_config),
            events,
        }
    }

    pub fn ensure_size(&mut self, width: usize, height: usize, state: &mut AppState) {
        if self.size == (width, height) {
            return;
        }
        self.size = (width, height);
        self.grid = Grid::new(width, height);
        self.render_state.resize(width, height);
        self.reset_simulation(self.current_edge(state));
        if width > 0 && height > 0 {
            let _ = self.reseed(state);
        }
    }

    pub fn tick(&mut self, state: &mut AppState) {
        self.handle_worker_events(state);
        self.apply_state_changes(state);
        self.step_if_due(state);
        self.sync_state(state);
    }

    pub fn grid(&self) -> Option<&Grid> {
        if self.size.0 == 0 || self.size.1 == 0 {
            None
        } else {
            Some(&self.grid)
        }
    }

    pub fn render_state(&self) -> &GolRenderState {
        &self.render_state
    }

    pub fn title_text(&self) -> &str {
        &self.title_cache
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
            self.period = None;
            self.last_attractor = None;
            self.reset_attractor(self.current_edge(state));
            if state.visualizer.mode == VisualizerMode::Search {
                self.restart_search(state);
            }
        }

        if state.visualizer.auto_stop_policy != self.last_auto_stop_policy {
            self.last_auto_stop_policy = state.visualizer.auto_stop_policy;
            self.attractor.set_policy(self.last_auto_stop_policy);
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
            build_title(&mut self.title_cache, render_mode);
        }

        if state.visualizer.seed_source != self.last_seed_source {
            self.last_seed_source = state.visualizer.seed_source;
            if state.visualizer.mode == VisualizerMode::Search && self.size.0 > 0 && self.size.1 > 0 {
                self.update_search_seed(state);
            }
        }

        if state.visualizer.pending_reseed && self.size.0 > 0 && self.size.1 > 0 {
            state.visualizer.pending_reseed = false;
            let ok = self.reseed(state);
            if !ok {
                state.status = Some("Seed source failed".into());
            } else if state.visualizer.paused_by_attractor {
                state.visualizer.paused_by_attractor = false;
                state.visualizer.paused = false;
                state.status = Some("Visualizer resumed (reseed)".into());
            }
            if state.visualizer.mode == VisualizerMode::Search {
                if self.search_paused_for_stability {
                    self.start_search(state);
                } else {
                    self.update_search_seed(state);
                }
            }
        }

        if state.visualizer.pending_rule_change {
            state.visualizer.pending_rule_change = false;
            self.rule_mode = state.visualizer.rule_mode.clone();
            self.rule_mode.reset();
            self.rule = self.rule_mode.current_rule().rule;
            self.reset_simulation(self.current_edge(state));
        }

        if state.visualizer.pending_apply {
            state.visualizer.pending_apply = false;
            if state.visualizer.mode == VisualizerMode::Search {
                self.apply_best_rule(state);
            }
        }

        if state.visualizer.pending_snapshot {
            state.visualizer.pending_snapshot = false;
            self.queue_snapshot(
                state,
                SnapshotTrigger::Manual,
                self.grid.clone(),
                self.rule,
                self.generation,
                self.period.map(|value| value as u64),
                self.alive,
                None,
                None,
                false,
                None,
            );
        }
    }

    fn step_if_due(&mut self, state: &mut AppState) {
        if !state.visualizer.running {
            return;
        }
        if state.visualizer.paused || self.size.0 == 0 || self.size.1 == 0 {
            return;
        }
        let interval = Duration::from_millis(state.visualizer.tick_ms.max(10));
        if self.last_step.elapsed() < interval {
            return;
        }

        let edge = self.current_edge(state);
        let current_rule = self.rule_mode.current_rule().rule;
        let next = step(&self.grid, current_rule, edge);
        let next_gen = self.generation.saturating_add(1);
        self.rule_mode.advance_one_gen();
        let next_rule = self.rule_mode.current_rule().rule;
        let event = self.attractor.observe_with_context(
            &self.grid,
            &next,
            next_gen,
            next_rule,
            edge,
            protocol_extra(&self.rule_mode),
        );
        let (alive, _) = self.render_state.update_from_step(&self.grid, &next);
        self.grid = next;
        self.generation = next_gen;
        self.alive = alive;
        self.rule = next_rule;
        if let Some(event) = event {
            self.handle_attractor_event(state, event);
        }

        self.last_step = Instant::now();
    }

    fn sync_state(&mut self, state: &mut AppState) {
        state.visualizer.rule = self.rule.to_string();
        state.visualizer.rule_mode = self.rule_mode.clone();
        state.visualizer.generation = self.generation;
        state.visualizer.alive = self.alive;
        state.visualizer.period = self.period;
        state.visualizer.last_attractor = self.last_attractor.clone();
        state.visualizer.search_rps = self.search_rps;
        state.visualizer.leaderboard = self
            .leaderboard
            .iter()
            .map(|entry| VisualizerRuleEntry {
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
    }

    fn reseed(&mut self, state: &AppState) -> bool {
        let Some((seed_hash, grid)) = self.try_build_seed(state, "keeping previous grid") else {
            return false;
        };
        self.rule_mode = state.visualizer.rule_mode.clone();
        self.rule_mode.reset();
        self.rule = self.rule_mode.current_rule().rule;
        self.grid = grid;
        self.last_seed_hash = seed_hash;
        self.reset_simulation(self.current_edge(state));
        self.alive = self.render_state.seed_from_grid(&self.grid);
        true
    }

    fn try_build_seed(&self, state: &AppState, panic_recovery: &str) -> Option<(u64, Grid)> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            build_seed_grid(state, self.size.0, self.size.1, state.visualizer.seed)
        }))
        .inspect_err(|_| warn!("Seed build panic; {panic_recovery}"))
        .ok()
    }

    fn reset_simulation(&mut self, edge: EdgeMode) {
        self.generation = 0;
        self.period = None;
        self.last_attractor = None;
        self.last_attractor_hash = None;
        self.attractor.reset();
        self.attractor.seed_with_context(
            &self.grid,
            self.generation,
            self.rule,
            edge,
            protocol_extra(&self.rule_mode),
        );
        self.last_step = Instant::now();
    }

    fn reset_attractor(&mut self, edge: EdgeMode) {
        self.last_attractor = None;
        self.last_attractor_hash = None;
        self.attractor.reset();
        self.attractor.seed_with_context(
            &self.grid,
            self.generation,
            self.rule,
            edge,
            protocol_extra(&self.rule_mode),
        );
    }

    fn current_edge(&self, state: &AppState) -> EdgeMode {
        edge_mode_from_wrap(state.visualizer.wrap)
    }

    fn handle_attractor_event(&mut self, state: &mut AppState, event: AttractorEvent) {
        let period = match &event {
            AttractorEvent::FixedPoint { .. } => Some(1u64),
            AttractorEvent::Cycle { period, .. } => Some(*period),
        };
        self.period = period.map(|value| value.min(u32::MAX as u64) as u32);
        self.last_attractor = Some(event.clone());

        let log_line = match &event {
            AttractorEvent::FixedPoint { gen } => format!("Fixed point at gen={gen}"),
            AttractorEvent::Cycle {
                gen,
                period,
                transient,
                ..
            } => format!(
                "Cycle detected: transient={transient}, period={period}, gen={gen}"
            ),
        };
        state.receive_log(log_line);

        let should_pause = state.visualizer.auto_stop_policy.should_stop(&event);
        if should_pause {
            state.visualizer.paused = true;
            state.visualizer.paused_by_attractor = true;
            state.status = Some(pause_status_message(&event, protocol_phase_label(&self.rule_mode)));
        }

        let grid_hash = grid_fingerprint(&self.grid);
        let is_new_attractor = self
            .last_attractor_hash
            .map_or(true, |prev| prev != grid_hash);
        self.last_attractor_hash = Some(grid_hash);

        let snapshot_event = event.clone();
        if is_new_attractor
            && (should_pause || attractor_snapshots_enabled(&state.settings.gol.snapshots, &event))
        {
            self.queue_snapshot(
                state,
                SnapshotTrigger::Attractor(snapshot_event),
                self.grid.clone(),
                self.rule,
                self.generation,
                period,
                self.alive,
                None,
                Some(event.clone()),
                should_pause,
                Some(grid_hash),
            );
        }

        if matches!(event, AttractorEvent::FixedPoint { .. })
            && state.visualizer.mode == VisualizerMode::Search
            && !self.search_paused_for_stability
        {
            self.search.send(SearchCommand::StopSearch);
            self.search_rps = 0;
            self.search_paused_for_stability = true;
            if !should_pause {
                state.status = Some("Search paused (stable)".into());
            }
        }
    }

    fn start_search(&mut self, state: &AppState) {
        self.search_paused_for_stability = false;
        let config = SearchConfig::from_settings(&state.settings.gol.search, state.visualizer.wrap);
        self.search_rps = config.rules_per_second;
        self.leaderboard_limit = config.leaderboard_size;
        let Some((seed_hash, seed)) = self.try_build_seed(state, "search not started") else {
            return;
        };
        self.last_seed_hash = seed_hash;
        self.leaderboard.clear();
        self.best_score = f32::MIN;
        self.search.send(SearchCommand::StartSearch {
            config,
            seed,
            base_rule: self.rule,
        });
    }

    fn restart_search(&mut self, state: &AppState) {
        self.search.send(SearchCommand::StopSearch);
        self.start_search(state);
    }

    fn update_search_seed(&mut self, state: &AppState) {
        let Some((seed_hash, seed)) = self.try_build_seed(state, "search seed unchanged") else {
            return;
        };
        self.last_seed_hash = seed_hash;
        self.search.send(SearchCommand::UpdateSeed { seed });
    }

    fn stop_search(&mut self) {
        self.search.send(SearchCommand::StopSearch);
        self.search_rps = 0;
        self.search_paused_for_stability = false;
    }

    fn apply_best_rule(&mut self, state: &mut AppState) {
        if let Some(best) = self.leaderboard.first() {
            let mut rule_ref = RuleRef {
                id: None,
                rule: best.rule,
                name: None,
            };
            if let Some(named) = state.rule_catalog.find_by_rule(best.rule) {
                rule_ref.id = Some(named.id.clone());
                rule_ref.name = Some(named.name.clone());
            }
            self.rule_mode = RuleMode::Fixed(rule_ref.clone());
            self.rule = rule_ref.rule;
            state.visualizer.rule_mode = self.rule_mode.clone();
            state.visualizer.rule = self.rule.to_string();
            state.visualizer.protocol_name = None;
            let mut selected = SelectedRule::from_rule(rule_ref.rule);
            if let Some(named) = state.rule_catalog.find_by_rule(rule_ref.rule) {
                selected.id = Some(named.id.clone());
                selected.name = Some(named.name.clone());
            } else {
                selected.id = rule_ref.id;
                selected.name = rule_ref.name;
            }
            state.gol_rule_selected = selected;
            info!(
                "Applying best rule {} score={:.2}",
                best.rule, best.score
            );
            let _ = self.reseed(state);
        }
    }

    fn handle_worker_events(&mut self, state: &mut AppState) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                WorkerEvent::BestRule(eval) => {
                    self.best_score = eval.score;
                    self.upsert_leaderboard(&eval);
                    info!(
                        "New best rule {} score={:.2} period={:?}",
                        eval.rule, eval.score, eval.period
                    );
                    let final_grid = eval.final_grid.clone();
                    self.queue_snapshot(
                        state,
                        SnapshotTrigger::BestRule,
                        final_grid,
                        eval.rule,
                        eval.transient as u64,
                        eval.period.map(|value| value as u64),
                        eval.alive_end as usize,
                        Some(eval.score),
                        None,
                        false,
                        None,
                    );
                    if !self.snapshot.record_rule(RuleLogEntry::from_eval(
                        &eval,
                        self.last_seed_hash,
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
        self.leaderboard.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        if self.leaderboard.len() > self.leaderboard_limit {
            self.leaderboard.truncate(self.leaderboard_limit);
        }
    }

    fn queue_snapshot(
        &mut self,
        state: &mut AppState,
        trigger: SnapshotTrigger,
        grid: Grid,
        rule: Rule,
        generation: u64,
        period: Option<u64>,
        alive: usize,
        score: Option<f32>,
        attractor: Option<AttractorEvent>,
        force: bool,
        grid_hash_override: Option<[u64; 2]>,
    ) {
        if !state.settings.gol.snapshots.enabled {
            return;
        }
        if !force {
            if let SnapshotTrigger::Attractor(event) = &trigger {
                let min_period = state.settings.gol.snapshots.min_period as u64;
                let min_transient = state.settings.gol.snapshots.min_transient as u64;
                match event {
                    AttractorEvent::FixedPoint { .. } => {
                        if generation < min_transient {
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
        }
        if grid.width() > u16::MAX as usize || grid.height() > u16::MAX as usize {
            warn!("Snapshot skipped; grid too large ({}x{})", grid.width(), grid.height());
            return;
        }
        let grid_hash = grid_hash_override.unwrap_or_else(|| grid_fingerprint(&grid));
        let grid_bits = pack_grid_bits(&grid);
        let event_kind = match &trigger {
            SnapshotTrigger::Manual => SnapshotEventKind::Manual,
            SnapshotTrigger::BestRule => SnapshotEventKind::NewBestRule,
            SnapshotTrigger::Attractor(AttractorEvent::FixedPoint { .. }) => {
                SnapshotEventKind::FixedPoint
            }
            SnapshotTrigger::Attractor(AttractorEvent::Cycle { .. }) => SnapshotEventKind::Cycle,
        };
        let meta = SnapshotMetadata {
            timestamp: now_iso8601(),
            workspace_root: Some(state.workspace_root.display().to_string()),
            file_path: state
                .editor_buffer()
                .path()
                .map(|p| p.display().to_string()),
            seed_source: format!("{:?}", state.visualizer.seed_source),
            seed_hash: self.last_seed_hash,
            rule: rule.to_string(),
            rule_id: state
                .rule_catalog
                .find_by_rule(rule)
                .map(|entry| entry.id.clone()),
            protocol: protocol_snapshot_string(&self.rule_mode),
            protocol_hash: protocol_snapshot_hash(&self.rule_mode),
            protocol_phase_idx: protocol_snapshot_phase(&self.rule_mode).map(|value| value.0),
            protocol_step_in_phase: protocol_snapshot_phase(&self.rule_mode).map(|value| value.1),
            generation,
            alive_count: alive,
            period,
            score,
            wrap_mode: wrap_mode_label(state.visualizer.wrap).into(),
            tick_ms: state.visualizer.tick_ms,
            attractor,
        };
        let req = SnapshotRequest {
            event: event_kind,
            timestamp: SystemTime::now(),
            gen: generation,
            rule: rule.to_string(),
            width: grid.width() as u16,
            height: grid.height() as u16,
            wrap: edge_mode_from_wrap(state.visualizer.wrap),
            seed_hash: self.last_seed_hash,
            grid_hash,
            grid_bits,
            period,
            transient: match &trigger {
                SnapshotTrigger::Attractor(AttractorEvent::Cycle { transient, .. }) => {
                    Some(*transient)
                }
                SnapshotTrigger::BestRule => Some(generation),
                _ => None,
            },
            score,
            meta,
        };
        let enqueued = self.snapshot.enqueue(req);
        if !enqueued && matches!(trigger, SnapshotTrigger::Manual) {
            state.status = Some("Snapshot dropped".into());
        }
    }
}

impl Drop for VisualizerRuntime {
    fn drop(&mut self) {
        self.search.send(SearchCommand::Shutdown);
        self.search.join();
        self.snapshot.shutdown();
    }
}

#[derive(Clone, Debug)]
enum SnapshotTrigger {
    Manual,
    BestRule,
    Attractor(AttractorEvent),
}

fn build_title(out: &mut String, mode: GolRenderMode) {
    out.clear();
    out.push_str("VISUALIZER (");
    out.push_str(mode.label());
    out.push_str(")  [ RUN ] [ ASCII ] [ APPLY ] [ SEED ] [ SNAP ] [ SEARCH ]");
}

const fn edge_mode_from_wrap(wrap: bool) -> EdgeMode {
    if wrap {
        EdgeMode::Toroid
    } else {
        EdgeMode::Dead
    }
}

const fn wrap_mode_label(wrap: bool) -> &'static str {
    if wrap {
        "toroid"
    } else {
        "dead"
    }
}

fn protocol_phase_label(rule_mode: &RuleMode) -> Option<String> {
    let RuleMode::Protocol(protocol) = rule_mode else {
        return None;
    };
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

fn pause_status_message(event: &AttractorEvent, protocol_phase: Option<String>) -> String {
    match event {
        AttractorEvent::FixedPoint { gen } => match protocol_phase {
            Some(phase) => format!("Visualizer paused (fixed point at gen={gen}, {phase})"),
            None => format!("Visualizer paused (fixed point at gen={gen})"),
        },
        AttractorEvent::Cycle {
            period,
            transient,
            gen,
            ..
        } => match protocol_phase {
            Some(phase) => format!(
                "Visualizer paused (cycle p={period} t={transient} gen={gen}, includes protocol phase; {phase})"
            ),
            None => format!(
                "Visualizer paused (cycle p={period} t={transient} gen={gen})"
            ),
        },
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

fn build_seed_grid(
    state: &AppState,
    width: usize,
    height: usize,
    seed: u64,
) -> (u64, Grid) {
    if state.visualizer.seed_ascii {
        build_seed_grid_ascii(state, width, height, seed)
    } else {
        build_seed_grid_text(state, width, height, seed)
    }
}

fn build_seed_grid_text(
    state: &AppState,
    width: usize,
    height: usize,
    seed: u64,
) -> (u64, Grid) {
    if width == 0 || height == 0 {
        let seed_hash = stable_hash_bytes(&seed.to_le_bytes());
        return (seed_hash, Grid::new(width, height));
    }
    let buffer = match state.visualizer.seed_source {
        GolSeedSource::Notes => state.notes_buffer(),
        GolSeedSource::Editor => state.editor_buffer(),
    };
    let start_line = buffer.viewport.offset_line;
    let mut lines: Vec<Vec<char>> = Vec::with_capacity(height);
    for row in 0..height {
        let line_idx = start_line + row;
        let mut line = if line_idx < buffer.lines_len() {
            buffer.line_as_string(line_idx)
        } else {
            String::new()
        };
        if line.ends_with('\n') {
            line.pop();
        }
        let mut chars: Vec<char> = line.chars().collect();
        if chars.len() > width {
            chars.truncate(width);
        }
        while chars.len() < width {
            chars.push(' ');
        }
        lines.push(chars);
    }
    let mut seed_text = String::new();
    for (idx, line) in lines.iter().enumerate() {
        for ch in line {
            seed_text.push(*ch);
        }
        if idx + 1 < lines.len() {
            seed_text.push('\n');
        }
    }
    let seed_hash = stable_hash_bytes(seed_text.as_bytes());
    let mut rng = SplitMix64::new(seed_hash ^ seed);
    let live_chars = build_live_chars(&state.settings.gol.seed_live_chars);
    let other_live_percent = state.settings.gol.seed_other_live_percent.min(100);
    let mut grid = Grid::new(width, height);
    for (y, line) in lines.iter().enumerate() {
        for (x, ch) in line.iter().enumerate() {
            let alive = map_char(*ch, &mut rng, &live_chars, other_live_percent);
            grid.set(x, y, alive);
        }
    }
    (seed_hash, grid)
}

fn build_seed_grid_ascii(
    state: &AppState,
    width: usize,
    height: usize,
    seed: u64,
) -> (u64, Grid) {
    if width == 0 || height == 0 {
        let seed_hash = stable_hash_bytes(&seed.to_le_bytes());
        return (seed_hash, Grid::new(width, height));
    }
    let buffer = match state.visualizer.seed_source {
        GolSeedSource::Notes => state.notes_buffer(),
        GolSeedSource::Editor => state.editor_buffer(),
    };
    let start_line = buffer.viewport.offset_line;
    let start_col = buffer.viewport.offset_col;
    let mut grid = Grid::new(width, height);
    let mut seed_text = String::with_capacity(width.saturating_mul(height + 1));

    for row in 0..height {
        let line_idx = start_line + row;
        let mut line = if line_idx < buffer.lines_len() {
            buffer.line_as_string(line_idx)
        } else {
            String::new()
        };
        if line.ends_with('\n') {
            line.pop();
        }
        let mut chars = line.chars();
        for _ in 0..start_col {
            if chars.next().is_none() {
                break;
            }
        }

        let mut x = 0usize;
        while x < width {
            let Some(ch) = chars.next() else {
                break;
            };
            let code = if ch.is_ascii() { ch as u32 } else { 127 };
            let digits = [
                ((code / 100) % 10) as u8,
                ((code / 10) % 10) as u8,
                (code % 10) as u8,
            ];
            for digit in digits {
                if x >= width {
                    break;
                }
                seed_text.push((b'0' + digit) as char);
                let offset = ((seed >> ((x + row) & 0x0f)) & 0x0f) as u8;
                let value = (digit + offset) % 10;
                if value >= ASCII_SEED_LIVE_MIN {
                    grid.set(x, row, true);
                }
                x += 1;
            }
            if x < width {
                seed_text.push(' ');
                x += 1;
            }
        }

        while x < width {
            seed_text.push(' ');
            x += 1;
        }

        if row + 1 < height {
            seed_text.push('\n');
        }
    }

    let seed_hash = stable_hash_bytes(seed_text.as_bytes());
    (seed_hash, grid)
}

fn build_live_chars(configured: &str) -> HashSet<char> {
    if configured.trim().is_empty() {
        return DEFAULT_LIVE_CHARS.iter().copied().collect();
    }
    configured.chars().collect()
}

fn attractor_snapshots_enabled(settings: &GolSnapshotsConfig, event: &AttractorEvent) -> bool {
    if settings.snapshot_on_attractor {
        return true;
    }
    if matches!(event, AttractorEvent::Cycle { .. }) && std::env::var_os("NIT_SNAPSHOT_CYCLE").is_some() {
        return true;
    }
    false
}

fn map_char(
    ch: char,
    rng: &mut SplitMix64,
    live_chars: &HashSet<char>,
    other_live_percent: u8,
) -> bool {
    if live_chars.contains(&ch) {
        return true;
    }
    if ch == '.' || ch.is_whitespace() {
        return false;
    }
    if other_live_percent == 0 {
        return false;
    }
    if other_live_percent >= 100 {
        return true;
    }
    let roll = (rng.next_u64() % 100) as u8;
    roll < other_live_percent
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
    fn from_settings(settings: &GolSearchConfig, wrap: bool) -> Self {
        let rules_per_second = if settings.rules_per_second > 0 {
            settings.rules_per_second
        } else {
            match settings.intensity {
                GolSearchIntensity::Low => 10,
                GolSearchIntensity::Med => 30,
                GolSearchIntensity::High => 80,
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
}

impl SearchWorker {
    fn spawn(event_tx: Sender<WorkerEvent>) -> Self {
        let (tx, cmd_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("nit-gol-search".into())
            .stack_size(search_worker_stack_bytes())
            .spawn(move || search_worker_loop(cmd_rx, event_tx))
            .expect("spawn search worker");
        Self {
            tx,
            handle: Some(handle),
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
    UpdateSeed {
        seed: Grid,
    },
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
                edge_mode_from_wrap(config.wrap),
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

        leaderboard.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
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

fn handle_search_command(
    cmd: SearchCommand,
    search_active: &mut bool,
    config: &mut SearchConfig,
    seed: &mut Grid,
    leaderboard: &mut Vec<RuleScore>,
    best_score: &mut f32,
    base_rule: &mut Rule,
    _event_tx: &Sender<WorkerEvent>,
) -> bool {
    match cmd {
        SearchCommand::StartSearch {
            config: new_config,
            seed: new_seed,
            base_rule: rule,
        } => {
            *config = new_config;
            *seed = new_seed;
            *base_rule = rule;
            *search_active = true;
            leaderboard.clear();
            *best_score = f32::MIN;
        }
        SearchCommand::StopSearch => {
            *search_active = false;
            leaderboard.clear();
            *best_score = f32::MIN;
        }
        SearchCommand::UpdateSeed { seed: new_seed } => {
            *seed = new_seed;
            leaderboard.clear();
            *best_score = f32::MIN;
        }
        SearchCommand::Shutdown => return true,
    }
    false
}

fn sample_rule(rng: &mut SplitMix64, base_rule: Rule) -> Rule {
    let mut births = base_rule.births_mask();
    let mut survives = base_rule.survives_mask();
    if rng.next_u64() % 5 == 0 {
        births = (rng.next_u64() & 0x01ff) as u16;
        survives = (rng.next_u64() & 0x01ff) as u16;
    } else {
        let flips = 1 + (rng.next_u64() % 3) as usize;
        for _ in 0..flips {
            if rng.next_u64() & 1 == 0 {
                let bit = (rng.next_u64() % 9) as u8;
                births ^= 1 << bit;
            } else {
                let bit = (rng.next_u64() % 9) as u8;
                survives ^= 1 << bit;
            }
        }
    }
    Rule::new(births, survives)
}
