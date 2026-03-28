use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nit_core::seed::SeedInput;
use nit_core::seed::SeedViewMode;
use nit_core::{
    encode_seed, AppState, EncodedSeed, GolSeedSource, PaneId, SeedEncoderId, SeedParams,
};
use nit_utils::hashing::SplitMix64;

use crate::gol_render::GolRenderState;
use crate::seed_render::SeedRenderCache;
use crate::seed_snapshot::{
    pack_grid_bits, seed_snapshot_name_base, SeedGenomePreview, SeedSnapshotManager,
    SeedSnapshotManagerConfig, SeedSnapshotMetadata, SeedSnapshotRequest,
};
use nit_gol::snapshot::now_iso8601;

const DEFAULT_DEBOUNCE_MS: u64 = 120;

pub struct SeedRuntime {
    size: (usize, usize),
    render_state: GolRenderState,
    encoded: Option<EncodedSeed>,
    input: SeedInputOwned,
    last_seed_source: GolSeedSource,
    last_buffer_id: usize,
    last_encoder: SeedEncoderId,
    last_params: SeedParams,
    last_variant: u8,
    last_seed_nonce: u64,
    pending_recompute: bool,
    last_edit: Instant,
    debounce: Duration,
    compute: SeedComputeWorker,
    compute_inflight: bool,
    search: SeedSearchWorker,
    search_best: Option<SeedSearchProposal>,
    search_active: bool,
    snapshot: SeedSnapshotManager,
    render_cache: SeedRenderCache,
    last_cache_hash: u64,
    last_cache_size: (usize, usize),
    scanline_phase: u16,
    last_scanline: Instant,
}

impl SeedRuntime {
    pub fn new(state: &AppState) -> Self {
        let snapshot_dir = state.workspace_root.join("gol-snapshots");
        let snapshot_config = SeedSnapshotManagerConfig::new(
            snapshot_dir,
            state.settings.gol.snapshots.max_files,
            state.settings.gol.snapshots.min_interval_ms,
        );
        let input = SeedInputOwned::from_state(state);
        Self {
            size: (0, 0),
            render_state: GolRenderState::new(),
            encoded: None,
            input,
            last_seed_source: state.visualizer.seed_source,
            last_buffer_id: state.active_editor_buffer_id,
            last_encoder: state.visualizer.seed_encoder,
            last_params: state.visualizer.seed_params.clone(),
            last_variant: state.visualizer.variant,
            last_seed_nonce: state.visualizer.seed,
            pending_recompute: true,
            last_edit: Instant::now(),
            debounce: Duration::from_millis(DEFAULT_DEBOUNCE_MS),
            compute: SeedComputeWorker::spawn(),
            compute_inflight: false,
            search: SeedSearchWorker::spawn(),
            search_best: None,
            search_active: false,
            snapshot: SeedSnapshotManager::new(snapshot_config),
            render_cache: SeedRenderCache::default(),
            last_cache_hash: 0,
            last_cache_size: (0, 0),
            scanline_phase: 0,
            last_scanline: Instant::now(),
        }
    }

    pub fn ensure_size(&mut self, width: usize, height: usize, state: &mut AppState) {
        if self.size == (width, height) {
            return;
        }
        self.size = (width, height);
        self.render_state.resize(width, height);
        self.pending_recompute = true;
        self.last_edit = Instant::now();
        state.visualizer.pending_reseed = true;
    }

    pub fn tick(&mut self, state: &mut AppState) {
        self.handle_compute_results(state);
        self.handle_search_events(state);
        self.apply_state_changes(state);
        self.recompute_if_due(state);
        self.tick_scanline(state);
        self.sync_snapshot_stats(state);
    }

    pub fn encoded(&self) -> Option<&EncodedSeed> {
        self.encoded.as_ref()
    }

    pub fn render_state(&self) -> &GolRenderState {
        &self.render_state
    }

    pub fn render_cache(&self) -> &SeedRenderCache {
        &self.render_cache
    }

    pub fn encode_now(&mut self, state: &mut AppState) -> Option<&EncodedSeed> {
        self.refresh_input(state);
        let seed = self.compute_seed(state, self.size.0, self.size.1)?;
        self.encoded = Some(seed);
        self.pending_recompute = false;
        self.update_state_from_seed(state);
        self.encoded.as_ref()
    }

    pub fn encode_for_size(
        &mut self,
        state: &mut AppState,
        width: usize,
        height: usize,
    ) -> Option<EncodedSeed> {
        self.refresh_input(state);
        self.compute_seed(state, width, height)
    }

    pub fn snapshot_current(&mut self, state: &mut AppState) {
        if let Some(seed) = self.encoded.clone() {
            self.queue_snapshot(state, &seed);
        }
    }

    pub fn snapshot_seed(&mut self, state: &mut AppState, seed: &EncodedSeed) {
        self.queue_snapshot(state, seed);
    }

    fn apply_state_changes(&mut self, state: &mut AppState) {
        if state.visualizer.seed_source != self.last_seed_source {
            self.last_seed_source = state.visualizer.seed_source;
            self.refresh_input(state);
            self.pending_recompute = true;
            self.last_edit = Instant::now();
        }

        if state.visualizer.seed_encoder != self.last_encoder
            || state.visualizer.seed_params != self.last_params
        {
            self.last_encoder = state.visualizer.seed_encoder;
            self.last_params = state.visualizer.seed_params.clone();
            self.pending_recompute = true;
            self.last_edit = Instant::now();
        }

        if state.visualizer.variant != self.last_variant
            || state.visualizer.seed != self.last_seed_nonce
        {
            self.last_variant = state.visualizer.variant;
            self.last_seed_nonce = state.visualizer.seed;
            self.pending_recompute = true;
            self.last_edit = Instant::now();
        }

        if state.visualizer.pending_reseed {
            state.visualizer.pending_reseed = false;
            self.input = SeedInputOwned::from_state(state);
            self.last_buffer_id = state.active_editor_buffer_id;
            self.pending_recompute = true;
            self.last_edit = Instant::now();
        }

        if state.active_editor_buffer_id != self.last_buffer_id {
            self.last_buffer_id = state.active_editor_buffer_id;
            self.input = SeedInputOwned::from_state(state);
            self.pending_recompute = true;
            self.last_edit = Instant::now();
        }

        let version = self.current_version(state);
        if version != self.input.version {
            self.refresh_input(state);
            self.pending_recompute = true;
            self.last_edit = Instant::now();
        }

        if state.visualizer.pending_apply {
            state.visualizer.pending_apply = false;
            if let Some(best) = self.search_best.take() {
                state.visualizer.seed_params = best.params.clone();
                self.last_params = state.visualizer.seed_params.clone();
                self.pending_recompute = true;
                self.last_edit = Instant::now();
                state.status = Some(format!("Applied seed params (score {:.2})", best.score));
            } else {
                state.status = Some("No seed proposal".into());
            }
        }

        if state.visualizer.pending_snapshot {
            state.visualizer.pending_snapshot = false;
            if let Some(seed) = self.encoded.clone() {
                self.queue_snapshot(state, &seed);
            } else {
                self.pending_recompute = true;
                self.last_edit = Instant::now();
            }
        }

        if state.visualizer.seed_search_active != self.search_active {
            self.search_active = state.visualizer.seed_search_active;
            if self.search_active {
                self.start_search(state);
            } else {
                self.search.send(SeedSearchCommand::StopSearch);
                state.visualizer.seed_search_rps = 0;
                self.search_best = None;
            }
        }
    }

    fn handle_compute_results(&mut self, state: &mut AppState) {
        let mut latest = None;
        while let Ok(seed) = self.compute.results.try_recv() {
            latest = Some(seed);
        }
        if let Some(seed) = latest {
            self.compute_inflight = false;
            self.encoded = Some(seed);
            self.update_state_from_seed(state);
            if self.search_active {
                self.update_search_seed(state);
            }
        }
    }

    fn recompute_if_due(&mut self, state: &mut AppState) {
        if !self.pending_recompute {
            return;
        }
        if self.last_edit.elapsed() < self.debounce {
            return;
        }
        self.refresh_input(state);
        let (w, h) = (self.size.0, self.size.1);
        if w == 0 || h == 0 {
            return;
        }
        self.pending_recompute = false;
        self.compute_inflight = true;
        self.compute.send(SeedComputeRequest::Compute {
            input: self.input.clone(),
            encoder: state.visualizer.seed_encoder,
            params: state.visualizer.seed_params.clone(),
            seed_nonce: state.visualizer.seed,
            variant: state.visualizer.variant,
            width: w,
            height: h,
        });
    }

    fn refresh_input(&mut self, state: &AppState) {
        if self.input.source != state.visualizer.seed_source
            || self.input.version != self.current_version(state)
        {
            self.input = SeedInputOwned::from_state(state);
        }
    }

    fn compute_seed(&self, state: &AppState, width: usize, height: usize) -> Option<EncodedSeed> {
        if width == 0 || height == 0 {
            return None;
        }
        let input = self.input.as_seed_input();
        Some(encode_seed(
            &input,
            state.visualizer.seed_encoder,
            &state.visualizer.seed_params,
            state.visualizer.seed,
            state.visualizer.variant,
            width,
            height,
        ))
    }

    fn update_state_from_seed(&mut self, state: &mut AppState) {
        let Some(seed) = self.encoded.as_ref() else {
            return;
        };
        state.visualizer.seed_hash = seed.seed_hash;
        state.visualizer.input_hash = seed.input_hash;
        state.visualizer.seed_stats = seed.stats.clone();
        reset_inspector_if_seed_changed(state, seed);
        self.render_state.seed_from_grid(&seed.grid);
        let size = (seed.grid.width(), seed.grid.height());
        if self.last_cache_hash != seed.seed_hash || self.last_cache_size != size {
            self.render_cache.update(seed);
            self.last_cache_hash = seed.seed_hash;
            self.last_cache_size = size;
        }
    }

    fn queue_snapshot(&mut self, state: &mut AppState, seed: &EncodedSeed) {
        if seed.grid.width() > u16::MAX as usize || seed.grid.height() > u16::MAX as usize {
            state.status = Some("Seed snapshot skipped; grid too large".into());
            return;
        }
        let name_base = seed_snapshot_name_base(seed.encoder_id.as_str(), seed.seed_hash);
        let source_buffer = match state.visualizer.seed_source {
            GolSeedSource::Editor => state.editor_buffer(),
            GolSeedSource::Notes => state.notes_buffer(),
        };
        let meta = SeedSnapshotMetadata {
            timestamp: now_iso8601(),
            workspace_root: Some(state.workspace_root.display().to_string()),
            file_path: source_buffer.path().map(|p| p.display().to_string()),
            source: format!("{:?}", state.visualizer.seed_source),
            revision: self.input.version,
            encoder_id: seed.encoder_id.as_str().to_string(),
            encoder_params: seed.params.summary(),
            params_fingerprint: seed.params.fingerprint(),
            seed_hash: seed.seed_hash,
            input_hash: seed.input_hash,
            density: seed.stats.density,
            symmetry: seed.params.symmetry.label().to_string(),
            components: seed.stats.components,
            width: seed.grid.width(),
            height: seed.grid.height(),
            view_type: seed_view_type(state),
            render_mode: seed_render_mode(state),
            genome_preview: seed_genome_preview(seed),
        };
        let req = SeedSnapshotRequest {
            timestamp: std::time::SystemTime::now(),
            name_base,
            width: seed.grid.width() as u16,
            height: seed.grid.height() as u16,
            grid_bits: pack_grid_bits(&seed.grid),
            meta,
        };
        let enqueued = self.snapshot.enqueue(req);
        if !enqueued {
            state.status = Some("Seed snapshot dropped".into());
        } else {
            state.status = Some("Seed snapshot queued".into());
        }
    }

    fn sync_snapshot_stats(&mut self, state: &mut AppState) {
        let stats = self.snapshot.stats();
        state.visualizer.seed_snapshots_written = stats.written;
        state.visualizer.seed_snapshots_dropped = stats.dropped;
        state.visualizer.seed_snapshot_queue_depth = stats.queue_len;
        state.visualizer.seed_last_snapshot_path =
            stats.last_path.map(|path| path.display().to_string());
    }

    fn current_version(&self, state: &AppState) -> u64 {
        match state.visualizer.seed_source {
            GolSeedSource::Editor => state.editor_buffer().version(),
            GolSeedSource::Notes => state.notes_buffer().version(),
        }
    }

    fn start_search(&mut self, state: &mut AppState) {
        let config = SeedSearchConfig::from_settings(&state.settings.gol.search);
        state.visualizer.seed_search_rps = config.rules_per_second;
        let input = self.input.clone();
        let params = state.visualizer.seed_params.clone();
        let cmd = SeedSearchCommand::StartSearch {
            config,
            input,
            encoder: state.visualizer.seed_encoder,
            params,
            seed_nonce: state.visualizer.seed,
            variant: state.visualizer.variant,
            target_size: self.size,
        };
        self.search.send(cmd);
        self.search_best = None;
    }

    fn update_search_seed(&mut self, state: &AppState) {
        let config = SeedSearchConfig::from_settings(&state.settings.gol.search);
        let cmd = SeedSearchCommand::UpdateInput {
            config,
            input: self.input.clone(),
            encoder: state.visualizer.seed_encoder,
            params: state.visualizer.seed_params.clone(),
            seed_nonce: state.visualizer.seed,
            variant: state.visualizer.variant,
            target_size: self.size,
        };
        self.search.send(cmd);
    }

    fn handle_search_events(&mut self, state: &mut AppState) {
        while let Ok(event) = self.search.events.try_recv() {
            match event {
                SeedSearchEvent::BestProposal(proposal) => {
                    self.search_best = Some(proposal);
                    state.status = Some("Seed proposal updated".into());
                }
            }
        }
    }

    fn tick_scanline(&mut self, state: &AppState) {
        if !state.visualizer.seed_scanline {
            return;
        }
        if state.focus != PaneId::Visualizer {
            return;
        }
        if self.last_scanline.elapsed() < Duration::from_millis(250) {
            return;
        }
        self.last_scanline = Instant::now();
        self.scanline_phase = self.scanline_phase.wrapping_add(1);
        self.render_cache.scanline_phase = self.scanline_phase;
    }
}

impl Drop for SeedRuntime {
    fn drop(&mut self) {
        self.compute.send(SeedComputeRequest::Shutdown);
        self.compute.join();
        self.search.send(SeedSearchCommand::Shutdown);
        self.search.join();
        self.snapshot.shutdown();
    }
}

#[derive(Clone, Debug)]
struct SeedInputOwned {
    text: String,
    source: GolSeedSource,
    file_path: Option<PathBuf>,
    version: u64,
}

impl SeedInputOwned {
    fn from_state(state: &AppState) -> Self {
        let (buffer, source) = match state.visualizer.seed_source {
            GolSeedSource::Editor => (state.editor_buffer(), GolSeedSource::Editor),
            GolSeedSource::Notes => (state.notes_buffer(), GolSeedSource::Notes),
        };
        Self {
            text: buffer.content_as_string(),
            source,
            file_path: buffer.path().cloned(),
            version: buffer.version(),
        }
    }

    fn as_seed_input(&self) -> SeedInput<'_> {
        SeedInput {
            text: &self.text,
            source: self.source,
            file_path: self.file_path.as_deref(),
            version: self.version,
        }
    }
}

enum SeedComputeRequest {
    Compute {
        input: SeedInputOwned,
        encoder: SeedEncoderId,
        params: SeedParams,
        seed_nonce: u64,
        variant: u8,
        width: usize,
        height: usize,
    },
    Shutdown,
}

struct SeedComputeWorker {
    tx: Sender<SeedComputeRequest>,
    results: Receiver<EncodedSeed>,
    handle: Option<JoinHandle<()>>,
}

impl SeedComputeWorker {
    fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<SeedComputeRequest>();
        let (result_tx, results) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("nit-seed-compute".into())
            .spawn(move || {
                while let Ok(mut req) = rx.recv() {
                    // Drain stale requests, keep only the latest.
                    while let Ok(newer) = rx.try_recv() {
                        req = newer;
                    }
                    match req {
                        SeedComputeRequest::Compute {
                            input,
                            encoder,
                            params,
                            seed_nonce,
                            variant,
                            width,
                            height,
                        } => {
                            let si = input.as_seed_input();
                            let seed = encode_seed(
                                &si, encoder, &params, seed_nonce, variant, width, height,
                            );
                            let _ = result_tx.send(seed);
                        }
                        SeedComputeRequest::Shutdown => break,
                    }
                }
            })
            .expect("spawn seed compute worker");
        Self {
            tx,
            results,
            handle: Some(handle),
        }
    }

    fn send(&self, req: SeedComputeRequest) {
        let _ = self.tx.send(req);
    }

    fn join(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

struct SeedSearchWorker {
    tx: Sender<SeedSearchCommand>,
    handle: Option<JoinHandle<()>>,
    events: Receiver<SeedSearchEvent>,
}

impl SeedSearchWorker {
    fn spawn() -> Self {
        let (tx, rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("nit-seed-search".into())
            .spawn(move || seed_search_loop(rx, event_tx))
            .expect("spawn seed search");
        Self {
            tx,
            handle: Some(handle),
            events,
        }
    }

    fn send(&self, cmd: SeedSearchCommand) {
        let _ = self.tx.send(cmd);
    }

    fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Clone, Debug)]
struct SeedSearchConfig {
    rules_per_second: u32,
    time_budget_ms_per_tick: u32,
    candidate_pool_size: usize,
}

impl SeedSearchConfig {
    fn from_settings(settings: &nit_core::GolSearchConfig) -> Self {
        let rules_per_second = if settings.rules_per_second > 0 {
            settings.rules_per_second
        } else {
            match settings.intensity {
                nit_core::GolSearchIntensity::Low => 8,
                nit_core::GolSearchIntensity::Med => 20,
                nit_core::GolSearchIntensity::High => 40,
            }
        };
        Self {
            rules_per_second,
            time_budget_ms_per_tick: settings.time_budget_ms_per_tick,
            candidate_pool_size: settings.candidate_pool_size.max(1),
        }
    }
}

#[derive(Clone, Debug)]
struct SeedSearchProposal {
    params: SeedParams,
    score: f32,
}

enum SeedSearchCommand {
    StartSearch {
        config: SeedSearchConfig,
        input: SeedInputOwned,
        encoder: SeedEncoderId,
        params: SeedParams,
        seed_nonce: u64,
        variant: u8,
        target_size: (usize, usize),
    },
    UpdateInput {
        config: SeedSearchConfig,
        input: SeedInputOwned,
        encoder: SeedEncoderId,
        params: SeedParams,
        seed_nonce: u64,
        variant: u8,
        target_size: (usize, usize),
    },
    StopSearch,
    Shutdown,
}

enum SeedSearchEvent {
    BestProposal(SeedSearchProposal),
}

fn seed_search_loop(cmd_rx: Receiver<SeedSearchCommand>, event_tx: Sender<SeedSearchEvent>) {
    let mut search_active = false;
    let mut config = SeedSearchConfig {
        rules_per_second: 10,
        time_budget_ms_per_tick: 8,
        candidate_pool_size: 8,
    };
    let mut input = None::<SeedInputOwned>;
    let mut encoder = SeedEncoderId::Lifehash16;
    let mut base_params = SeedParams::default();
    let mut seed_nonce = 0u64;
    let mut variant = 0u8;
    let mut target_size = (0usize, 0usize);
    let mut rng = SplitMix64::new(0x5eedcafe);
    let mut best_score = f32::MIN;

    loop {
        if !search_active {
            match cmd_rx.recv() {
                Ok(cmd) => {
                    if handle_seed_search_cmd(
                        cmd,
                        &mut search_active,
                        &mut config,
                        &mut input,
                        &mut encoder,
                        &mut base_params,
                        &mut seed_nonce,
                        &mut variant,
                        &mut target_size,
                        &mut best_score,
                    ) {
                        break;
                    }
                }
                Err(_) => break,
            }
            continue;
        }

        while let Ok(cmd) = cmd_rx.try_recv() {
            if handle_seed_search_cmd(
                cmd,
                &mut search_active,
                &mut config,
                &mut input,
                &mut encoder,
                &mut base_params,
                &mut seed_nonce,
                &mut variant,
                &mut target_size,
                &mut best_score,
            ) {
                return;
            }
        }

        let Some(input) = input.clone() else {
            search_active = false;
            continue;
        };
        let start = Instant::now();
        let budget = Duration::from_millis(config.time_budget_ms_per_tick.max(1) as u64);
        let max_candidates = config.candidate_pool_size;
        let mut evaluated = 0usize;
        while evaluated < max_candidates {
            let candidate = mutate_params(&base_params, &mut rng);
            let seed = encode_seed(
                &input.as_seed_input(),
                encoder,
                &candidate,
                seed_nonce,
                variant,
                target_size.0,
                target_size.1,
            );
            let density_diff = (seed.stats.density - base_params.target_density).abs();
            let score = seed.stats.components as f32 - density_diff * 40.0;
            evaluated += 1;
            if score > best_score {
                best_score = score;
                let _ = event_tx.send(SeedSearchEvent::BestProposal(SeedSearchProposal {
                    params: candidate.clone(),
                    score,
                }));
            }
            if start.elapsed() >= budget {
                break;
            }
        }

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
fn handle_seed_search_cmd(
    cmd: SeedSearchCommand,
    search_active: &mut bool,
    config: &mut SeedSearchConfig,
    input: &mut Option<SeedInputOwned>,
    encoder: &mut SeedEncoderId,
    params: &mut SeedParams,
    seed_nonce: &mut u64,
    variant: &mut u8,
    target_size: &mut (usize, usize),
    best_score: &mut f32,
) -> bool {
    match cmd {
        SeedSearchCommand::StartSearch {
            config: next,
            input: seed_input,
            encoder: next_encoder,
            params: next_params,
            seed_nonce: next_seed,
            variant: next_variant,
            target_size: next_size,
        } => {
            *config = next;
            *input = Some(seed_input);
            *encoder = next_encoder;
            *params = next_params;
            *seed_nonce = next_seed;
            *variant = next_variant;
            *target_size = next_size;
            *best_score = f32::MIN;
            *search_active = true;
        }
        SeedSearchCommand::UpdateInput {
            config: next,
            input: seed_input,
            encoder: next_encoder,
            params: next_params,
            seed_nonce: next_seed,
            variant: next_variant,
            target_size: next_size,
        } => {
            *config = next;
            *input = Some(seed_input);
            *encoder = next_encoder;
            *params = next_params;
            *seed_nonce = next_seed;
            *variant = next_variant;
            *target_size = next_size;
            *best_score = f32::MIN;
            *search_active = true;
        }
        SeedSearchCommand::StopSearch => {
            *search_active = false;
        }
        SeedSearchCommand::Shutdown => return true,
    }
    false
}

fn mutate_params(base: &SeedParams, rng: &mut SplitMix64) -> SeedParams {
    let mut params = base.clone();
    if rng.next_u64() % 100 < 45 {
        params.symmetry = match rng.next_u64() % 4 {
            0 => nit_core::SeedSymmetry::None,
            1 => nit_core::SeedSymmetry::MirrorX,
            2 => nit_core::SeedSymmetry::MirrorY,
            _ => nit_core::SeedSymmetry::Rotate180,
        };
    }
    if rng.next_u64() % 100 < 30 {
        params.placement = if rng.next_u64().is_multiple_of(2) {
            nit_core::SeedPlacement::Center
        } else {
            nit_core::SeedPlacement::TopLeft
        };
    }
    if rng.next_u64() % 100 < 50 {
        let delta = (rng.next_f32() - 0.5) * 0.15;
        params.target_density = (params.target_density + delta).clamp(0.08, 0.7);
    }
    if rng.next_u64() % 100 < 60 {
        let delta = (rng.next_f32() - 0.5) * 0.12;
        params.jitter = (params.jitter + delta).clamp(0.0, 0.25);
    }
    if rng.next_u64() % 100 < 35 {
        let pad = rng.next_u64() % 4;
        params.padding = pad as u8;
    }
    params
}

fn seed_view_type(state: &AppState) -> String {
    match state.visualizer.seed_view {
        SeedViewMode::Genome => "genome".into(),
        SeedViewMode::Plate => "plate".into(),
        SeedViewMode::Map => "map".into(),
        SeedViewMode::Stats => "stats".into(),
    }
}

fn seed_render_mode(state: &AppState) -> Option<String> {
    if state.visualizer.seed_view != SeedViewMode::Plate {
        return None;
    }
    let mode = match state.visualizer.seed_plate_mode {
        nit_core::SeedPreviewMode::Solid => "solid",
        nit_core::SeedPreviewMode::HalfBlock => "halfblock",
        nit_core::SeedPreviewMode::Braille => "braille",
        nit_core::SeedPreviewMode::Tissue => "tissue",
        nit_core::SeedPreviewMode::Heatmap => "heatmap",
    };
    Some(mode.to_string())
}

fn seed_genome_preview(seed: &EncodedSeed) -> Option<SeedGenomePreview> {
    match seed.encoder_id {
        SeedEncoderId::Lifehash16 => {
            if seed.base_bits_raw.width() != 16 || seed.base_bits_raw.height() != 16 {
                return None;
            }
            let mut bits = Vec::with_capacity(16 * 16);
            for y in 0..16usize {
                for x in 0..16usize {
                    bits.push(if seed.base_bits_raw.get(x, y) { 1 } else { 0 });
                }
            }
            Some(SeedGenomePreview {
                lifehash16_bits: Some(bits),
                hilbert_bits_prefix: None,
            })
        }
        SeedEncoderId::HilbertBits => {
            let w = seed.base_bits.width().max(1);
            let h = seed.base_bits.height().max(1);
            let total = w.saturating_mul(h);
            let limit = total.min(128);
            let mut bits = String::with_capacity(limit);
            for i in 0..limit {
                let x = i % w;
                let y = i / w;
                bits.push(if seed.base_bits.get(x, y) { '1' } else { '0' });
            }
            Some(SeedGenomePreview {
                lifehash16_bits: None,
                hilbert_bits_prefix: Some(bits),
            })
        }
        SeedEncoderId::AsciiBytes => None,
    }
}

fn reset_inspector_if_seed_changed(state: &mut AppState, seed: &EncodedSeed) {
    let (w, h) = (seed.base_bits.width(), seed.base_bits.height());
    if w == 0 || h == 0 {
        return;
    }
    let cx = w / 2;
    let cy = h / 2;
    match seed.encoder_id {
        SeedEncoderId::AsciiBytes => {
            if state.visualizer.inspect_ascii_hash != seed.seed_hash {
                state.visualizer.inspect_ascii_hash = seed.seed_hash;
                state.visualizer.inspect_ascii_x = cx;
                state.visualizer.inspect_ascii_y = cy;
            }
        }
        SeedEncoderId::Lifehash16 => {
            if state.visualizer.inspect_lifehash_hash != seed.seed_hash {
                state.visualizer.inspect_lifehash_hash = seed.seed_hash;
                state.visualizer.inspect_lifehash_x = cx;
                state.visualizer.inspect_lifehash_y = cy;
            }
        }
        SeedEncoderId::HilbertBits => {
            if state.visualizer.inspect_hilbert_hash != seed.seed_hash {
                state.visualizer.inspect_hilbert_hash = seed.seed_hash;
                state.visualizer.inspect_hilbert_x = cx;
                state.visualizer.inspect_hilbert_y = cy;
            }
        }
    }
}
