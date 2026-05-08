use super::*;
use crate::seed::SeedEncoderId;

mod hilbert;
use hilbert::{hilbert_index_to_xy, hilbert_order_for};

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
            GolRenderMode::HalfBlock if braille_enabled => GolRenderMode::Braille,
            GolRenderMode::HalfBlock => GolRenderMode::Solid,
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

    /// Resolve to a render mode the terminal can actually display: callers
    /// may have persisted `Braille` in settings, but if the current launch
    /// disables braille (no font, low-color terminal) we degrade silently
    /// to `HalfBlock` instead of rendering empty cells.
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
    pub rule_mode: RuleMode,
    pub protocol_name: Option<String>,
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
    #[serde(skip)]
    pub inspector_enabled: bool,
    #[serde(skip)]
    pub inspect_ascii_x: usize,
    #[serde(skip)]
    pub inspect_ascii_y: usize,
    #[serde(skip)]
    pub inspect_lifehash_x: usize,
    #[serde(skip)]
    pub inspect_lifehash_y: usize,
    #[serde(skip)]
    pub inspect_hilbert_x: usize,
    #[serde(skip)]
    pub inspect_hilbert_y: usize,
    #[serde(skip)]
    pub inspect_ascii_hash: u64,
    #[serde(skip)]
    pub inspect_lifehash_hash: u64,
    #[serde(skip)]
    pub inspect_hilbert_hash: u64,
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
    #[serde(skip)]
    pub pending_rule_change: bool,
}

/// Five overlay states the operator cycles through. Tuple layout matches
/// the field order in [`VisualizerState`]:
/// `(seed_show_grid, seed_show_bbox, seed_show_halo, seed_show_components, seed_show_inset)`.
const SEED_OVERLAY_PRESETS: &[(bool, bool, bool, bool, bool)] = &[
    (false, false, false, false, false),
    (false, false, true, false, false),
    (false, false, true, true, false),
    (false, true, true, true, false),
    (false, true, true, true, true),
];

pub(super) fn cycle_seed_overlays(state: &mut VisualizerState) {
    let current = (
        state.seed_show_grid,
        state.seed_show_bbox,
        state.seed_show_halo,
        state.seed_show_components,
        state.seed_show_inset,
    );
    let idx = SEED_OVERLAY_PRESETS
        .iter()
        .position(|preset| *preset == current)
        .unwrap_or(0);
    let next = SEED_OVERLAY_PRESETS[(idx + 1) % SEED_OVERLAY_PRESETS.len()];
    state.seed_show_grid = next.0;
    state.seed_show_bbox = next.1;
    state.seed_show_halo = next.2;
    state.seed_show_components = next.3;
    state.seed_show_inset = next.4;
}

pub(super) fn seed_overlay_label(state: &VisualizerState) -> String {
    let mut parts = Vec::new();
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

/// Per-encoder inspector cursor accessors. Three encoder families share a
/// single inspector grid (`ascii` for token/AST/complexity fields, `lifehash`
/// for the lifehash encoder, `hilbert` for the structural/hilbert pair),
/// so dispatch lives in one place rather than duplicated across each mutator.
fn inspector_xy_fields(
    viz: &mut VisualizerState,
    encoder: SeedEncoderId,
) -> (&mut usize, &mut usize) {
    match encoder {
        SeedEncoderId::AsciiBytes
        | SeedEncoderId::TokenSpectrum
        | SeedEncoderId::AstStructure
        | SeedEncoderId::ComplexityField => (&mut viz.inspect_ascii_x, &mut viz.inspect_ascii_y),
        SeedEncoderId::Lifehash16 => (&mut viz.inspect_lifehash_x, &mut viz.inspect_lifehash_y),
        SeedEncoderId::HilbertBits | SeedEncoderId::Structural => {
            (&mut viz.inspect_hilbert_x, &mut viz.inspect_hilbert_y)
        }
    }
}

pub(super) fn move_inspector(state: &mut AppState, dx: isize, dy: isize) {
    let w = state.visualizer.seed_stats.base_width;
    let h = state.visualizer.seed_stats.base_height;
    if w == 0 || h == 0 {
        return;
    }
    let encoder = state.visualizer.seed_encoder;
    let (x, y) = inspector_xy_fields(&mut state.visualizer, encoder);
    *x = clamp_signed(*x as isize + dx, 0, (w - 1) as isize) as usize;
    *y = clamp_signed(*y as isize + dy, 0, (h - 1) as isize) as usize;
}

pub(super) fn inspector_dims(state: &AppState) -> (usize, usize) {
    (
        state.visualizer.seed_stats.base_width,
        state.visualizer.seed_stats.base_height,
    )
}

pub(super) fn set_inspector_pos(state: &mut AppState, x: usize, y: usize) {
    let encoder = state.visualizer.seed_encoder;
    let (xf, yf) = inspector_xy_fields(&mut state.visualizer, encoder);
    *xf = x;
    *yf = y;
}

pub(super) fn jump_inspector_to_index(state: &mut AppState, idx: u64) {
    let (w, h) = inspector_dims(state);
    let total = w.saturating_mul(h).max(1) as u64;
    let clamped = idx.min(total.saturating_sub(1));
    let (x, y) = match state.visualizer.seed_encoder {
        SeedEncoderId::HilbertBits | SeedEncoderId::Structural => {
            let order = hilbert_order_for(w);
            let (hx, hy) = hilbert_index_to_xy(order, clamped as u32);
            (hx as usize, hy as usize)
        }
        _ => ((clamped as usize) % w, (clamped as usize) / w),
    };
    set_inspector_pos(state, x, y);
}

fn clamp_signed(value: isize, min: isize, max: isize) -> isize {
    value.clamp(min, max)
}
