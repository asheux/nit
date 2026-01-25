use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nit_core::{
    AppState, GolSearchConfig, GolSearchIntensity, GolSeedSource, GolSnapshotsConfig,
    VisualizerMode, VisualizerRuleEntry,
};
use nit_gol::{
    analyze::{evaluate_rule, RuleEvaluation, RuleScore},
    attractor::{AttractorConfig, AttractorDetector, AttractorEvent, AutoStopPolicy},
    snapshot::{default_name, now_iso8601, prune_oldest, write_snapshot, SnapshotMetadata},
    step::step,
    EdgeMode, Grid, Rule,
};
use nit_utils::hashing::{stable_hash_bytes, XorShift64};
use tracing::{info, warn};

const DEFAULT_LIVE_CHARS: &[char] = &['#', '@', '█', '▓', '▒', '░', '*', '+', 'x', 'X', '%', '&'];

pub struct VisualizerRuntime {
    size: (usize, usize),
    grid: Grid,
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
    snapshot_dir: PathBuf,
    rules_log_path: PathBuf,
    deduper: SnapshotDeduper,
    leaderboard: Vec<RuleScore>,
    leaderboard_limit: usize,
    best_score: f32,
    search_rps: u32,
    last_attractor: Option<AttractorEvent>,
    attractor: AttractorDetector,
    search_paused_for_stability: bool,
    search: SearchWorker,
    io: SnapshotWorker,
    events: Receiver<WorkerEvent>,
}

impl VisualizerRuntime {
    pub fn new(state: &AppState) -> Self {
        let rule = Rule::parse(&state.visualizer.rule).unwrap_or_else(|_| Rule::conway());
        let snapshot_dir = state.workspace_root.join("gol-snapshots");
        let rules_log_path = snapshot_dir.join("rules.ndjson");
        let (event_tx, events) = mpsc::channel();
        let attractor = AttractorDetector::new(AttractorConfig {
            policy: state.visualizer.auto_stop_policy,
            ..AttractorConfig::default()
        });
        Self {
            size: (0, 0),
            grid: Grid::new(0, 0),
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
            snapshot_dir,
            rules_log_path,
            deduper: SnapshotDeduper::new(128),
            leaderboard: Vec::new(),
            leaderboard_limit: 10,
            best_score: f32::MIN,
            search_rps: 0,
            last_attractor: None,
            attractor,
            search_paused_for_stability: false,
            search: SearchWorker::spawn(event_tx.clone()),
            io: SnapshotWorker::spawn(event_tx),
            events,
        }
    }

    pub fn ensure_size(&mut self, width: usize, height: usize, state: &mut AppState) {
        if self.size == (width, height) {
            return;
        }
        self.size = (width, height);
        self.grid = Grid::new(width, height);
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

        if state.visualizer.seed_source != self.last_seed_source {
            self.last_seed_source = state.visualizer.seed_source;
            if state.visualizer.mode == VisualizerMode::Search && self.size.0 > 0 && self.size.1 > 0 {
                self.update_search_seed(state);
            }
        }

        if state.visualizer.pending_reseed {
            if self.size.0 > 0 && self.size.1 > 0 {
                state.visualizer.pending_reseed = false;
                let ok = self.reseed(state);
                if !ok {
                    state.status = Some("Seed source failed".into());
                }
                if ok && state.visualizer.paused_by_attractor {
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
            );
        }
    }

    fn step_if_due(&mut self, state: &mut AppState) {
        if state.visualizer.paused || self.size.0 == 0 || self.size.1 == 0 {
            return;
        }
        let interval = Duration::from_millis(state.visualizer.tick_ms.max(10));
        if self.last_step.elapsed() < interval {
            return;
        }

        let edge = self.current_edge(state);
        let next = step(&self.grid, self.rule, edge);
        let next_gen = self.generation.saturating_add(1);
        let event = self
            .attractor
            .observe(&self.grid, &next, next_gen, self.rule, edge);
        self.grid = next;
        self.generation = next_gen;
        self.alive = self.grid.alive_count();
        if let Some(event) = event {
            self.handle_attractor_event(state, event);
        }

        self.last_step = Instant::now();
    }

    fn sync_state(&mut self, state: &mut AppState) {
        state.visualizer.rule = self.rule.to_string();
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
    }

    fn reseed(&mut self, state: &AppState) -> bool {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            build_seed_grid(state, self.size.0, self.size.1, state.visualizer.seed)
        }));
        let Ok((seed_hash, grid)) = result else {
            warn!("Seed build panic; keeping previous grid");
            return false;
        };
        self.grid = grid;
        self.last_seed_hash = seed_hash;
        self.reset_simulation(self.current_edge(state));
        self.alive = self.grid.alive_count();
        true
    }

    fn reset_simulation(&mut self, edge: EdgeMode) {
        self.generation = 0;
        self.period = None;
        self.last_attractor = None;
        self.attractor.reset();
        self.attractor.seed(&self.grid, self.generation, self.rule, edge);
        self.last_step = Instant::now();
    }

    fn reset_attractor(&mut self, edge: EdgeMode) {
        self.last_attractor = None;
        self.attractor.reset();
        self.attractor.seed(&self.grid, self.generation, self.rule, edge);
    }

    fn current_edge(&self, state: &AppState) -> EdgeMode {
        if state.visualizer.wrap {
            EdgeMode::Toroid
        } else {
            EdgeMode::Dead
        }
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
            state.status = Some(match &event {
                AttractorEvent::FixedPoint { gen } => {
                    format!("Visualizer paused (fixed point at gen={gen})")
                }
                AttractorEvent::Cycle { period, transient, gen, .. } => format!(
                    "Visualizer paused (cycle p={period} t={transient} gen={gen})"
                ),
            });
        }

        let snapshot_event = event.clone();
        if should_pause || attractor_snapshots_enabled(&state.settings.gol.snapshots, &event) {
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
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            build_seed_grid(state, self.size.0, self.size.1, state.visualizer.seed)
        }));
        let Ok((seed_hash, seed)) = result else {
            warn!("Seed build panic; search not started");
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
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            build_seed_grid(state, self.size.0, self.size.1, state.visualizer.seed)
        }));
        let Ok((seed_hash, seed)) = result else {
            warn!("Seed build panic for search update");
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

    fn apply_best_rule(&mut self, state: &AppState) {
        if let Some(best) = self.leaderboard.first() {
            self.rule = best.rule;
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
                    );
                    if !self.io.try_send(IoCommand::RecordRule(RuleLogEntry::from_eval(
                        &eval,
                        self.last_seed_hash,
                        &self.rules_log_path,
                    ))) {
                        warn!("Snapshot queue full; dropping rule log entry");
                    }
                }
                WorkerEvent::Leaderboard(entries) => {
                    self.leaderboard = entries;
                }
                WorkerEvent::SnapshotSaved(path) => {
                    info!("Snapshot saved: {}", path.display());
                }
                WorkerEvent::Error(msg) => {
                    warn!("Visualizer worker error: {}", msg);
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
        let hash = grid.hash();
        if !self.deduper.insert(hash) {
            return;
        }
        let name_base = default_name(rule, generation, hash);
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
        };
        let req = SnapshotRequest {
            dir: self.snapshot_dir.clone(),
            name_base,
            grid,
            rule,
            meta,
            max_files: state.settings.gol.snapshots.max_files,
        };
        if !self.io.try_send(IoCommand::Snapshot(req)) {
            state.status = Some("Snapshot queue full; dropping".into());
            warn!("Snapshot queue full; dropping snapshot");
        }
    }
}

impl Drop for VisualizerRuntime {
    fn drop(&mut self) {
        self.search.send(SearchCommand::Shutdown);
        let _ = self.io.try_send(IoCommand::Shutdown);
        self.search.join();
        self.io.join();
    }
}

#[derive(Clone, Debug)]
enum SnapshotTrigger {
    Manual,
    BestRule,
    Attractor(AttractorEvent),
}

fn build_seed_grid(
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
    let mut rng = XorShift64::new(seed_hash ^ seed);
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

fn build_live_chars(configured: &str) -> HashSet<char> {
    if configured.trim().is_empty() {
        return DEFAULT_LIVE_CHARS.iter().copied().collect();
    }
    configured.chars().collect()
}

fn snapshot_queue_capacity() -> usize {
    let from_env = std::env::var("NIT_SNAPSHOT_QUEUE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    from_env.unwrap_or(32).max(4)
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
    rng: &mut XorShift64,
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

struct SnapshotDeduper {
    seen: HashSet<u64>,
    order: VecDeque<u64>,
    capacity: usize,
}

impl SnapshotDeduper {
    fn new(capacity: usize) -> Self {
        Self {
            seen: HashSet::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    fn insert(&mut self, hash: u64) -> bool {
        if self.seen.contains(&hash) {
            return false;
        }
        self.seen.insert(hash);
        self.order.push_back(hash);
        if self.order.len() > self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.seen.remove(&old);
            }
        }
        true
    }
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

#[derive(Clone, serde::Serialize)]
struct RuleLogEntry {
    rule: String,
    score: f32,
    discovered_at: String,
    seed_hash: u64,
    notes: String,
    #[serde(skip)]
    path: PathBuf,
}

impl RuleLogEntry {
    fn from_eval(eval: &RuleEvaluation, seed_hash: u64, path: &Path) -> Self {
        Self {
            rule: eval.rule.to_string(),
            score: eval.score,
            discovered_at: now_iso8601(),
            seed_hash,
            notes: format!(
                "period={:?} transient={} alive_end={}",
                eval.period, eval.transient, eval.alive_end
            ),
            path: path.to_path_buf(),
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

struct SnapshotWorker {
    tx: SyncSender<IoCommand>,
    handle: Option<JoinHandle<()>>,
}

impl SnapshotWorker {
    fn spawn(event_tx: Sender<WorkerEvent>) -> Self {
        let (tx, cmd_rx) = mpsc::sync_channel(snapshot_queue_capacity());
        let handle = thread::Builder::new()
            .name("nit-gol-io".into())
            .stack_size(io_worker_stack_bytes())
            .spawn(move || snapshot_worker_loop(cmd_rx, event_tx))
            .expect("spawn snapshot worker");
        Self {
            tx,
            handle: Some(handle),
        }
    }

    fn try_send(&self, cmd: IoCommand) -> bool {
        match self.tx.try_send(cmd) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(_)) => false,
            Err(mpsc::TrySendError::Disconnected(_)) => false,
        }
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

fn io_worker_stack_bytes() -> usize {
    let override_mb = std::env::var("NIT_GOL_IO_STACK_MB")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    if let Some(mb) = override_mb {
        return worker_stack_bytes("NIT_GOL_IO_STACK_MB", mb, 32);
    }
    worker_stack_bytes("NIT_GOL_STACK_MB", 256, 32)
}

fn worker_stack_bytes(env_key: &str, default_mb: usize, min_mb: usize) -> usize {
    let from_env = std::env::var(env_key)
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    let mb = from_env.unwrap_or(default_mb).max(min_mb);
    mb.saturating_mul(1024 * 1024)
}

#[derive(Clone)]
struct SnapshotRequest {
    dir: PathBuf,
    name_base: String,
    grid: Grid,
    rule: Rule,
    meta: SnapshotMetadata,
    max_files: usize,
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

enum IoCommand {
    Snapshot(SnapshotRequest),
    RecordRule(RuleLogEntry),
    Shutdown,
}

enum WorkerEvent {
    BestRule(RuleEvaluation),
    Leaderboard(Vec<RuleScore>),
    SnapshotSaved(PathBuf),
    Error(String),
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
    let mut rng = XorShift64::new(0x5eed1234);
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

fn snapshot_worker_loop(cmd_rx: Receiver<IoCommand>, event_tx: Sender<WorkerEvent>) {
    loop {
        match cmd_rx.recv() {
            Ok(cmd) => {
                if handle_io_command(cmd, &event_tx) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn handle_io_command(cmd: IoCommand, event_tx: &Sender<WorkerEvent>) -> bool {
    match cmd {
        IoCommand::Snapshot(req) => {
            let res = write_snapshot(&req.dir, &req.name_base, &req.grid, req.rule, &req.meta);
            if let Err(err) = res {
                let _ = event_tx.send(WorkerEvent::Error(err.to_string()));
            } else {
                let _ = prune_oldest(&req.dir, req.max_files);
                let _ = event_tx.send(WorkerEvent::SnapshotSaved(req.dir.join(format!(
                    "{}.rle",
                    req.name_base
                ))));
            }
        }
        IoCommand::RecordRule(entry) => {
            if let Err(err) = append_rule_entry(entry) {
                let _ = event_tx.send(WorkerEvent::Error(err.to_string()));
            }
        }
        IoCommand::Shutdown => return true,
    }
    false
}

fn append_rule_entry(entry: RuleLogEntry) -> std::io::Result<()> {
    if let Some(parent) = entry.path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&entry.path)?;
    let json = serde_json::to_string(&entry)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    writeln!(file, "{}", json)?;
    Ok(())
}

fn sample_rule(rng: &mut XorShift64, base_rule: Rule) -> Rule {
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
