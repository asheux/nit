use nit_core::{GolRenderMode, VisualizerMode};
use nit_gol::Grid;
use ratatui::{buffer::Buffer, layout::Rect};

use super::HalfBlockRenderer;
use crate::gol_render::renderer::GolRenderer;
use crate::gol_render::{GolHudMetrics, GolHudState, GolPalette, GolRenderConfig, GolRenderState};
use crate::theme::Theme;

fn default_cfg(trails: bool) -> GolRenderConfig {
    GolRenderConfig {
        mode: GolRenderMode::HalfBlock,
        age_shading: false,
        trails,
        overlay_bbox: false,
        overlay_heat: false,
        scanlines: false,
        grid_minor: None,
        grid_major: None,
        gol_origin_x: 0,
        gol_origin_y: 0,
        debug_overlay: false,
        braille_enabled: true,
    }
}

fn hud_state<'a>(
    metrics: &'a GolHudMetrics,
    generation: u64,
    alive: usize,
    delta: u32,
) -> GolHudState<'a> {
    GolHudState {
        rule: "B3/S23",
        generation,
        alive,
        period: None,
        mode: VisualizerMode::SimOnly,
        paused: false,
        delta,
        history: &metrics.history,
    }
}

const AREA_1X2: Rect = Rect {
    x: 0,
    y: 0,
    width: 1,
    height: 2,
};

#[test]
fn halfblock_uniform_pixels_use_half_block() {
    let mut grid = Grid::new(1, 2);
    grid.set(0, 0, true);
    let mut state = GolRenderState::new();
    state.seed_from_grid(&grid);
    let palette = GolPalette::from_theme(&Theme::default());
    let metrics = GolHudMetrics::new(1);
    let hud = hud_state(&metrics, 0, 1, 0);
    let cfg = default_cfg(false);
    let mut buf = Buffer::empty(AREA_1X2);
    let mut renderer = HalfBlockRenderer;
    renderer.render(AREA_1X2, &mut buf, &grid, &state, &cfg, &palette, &hud);
    let cell = buf.get(0, 1);
    assert_eq!(cell.symbol(), "▀");
    assert_ne!(cell.fg, cell.bg);
}

#[test]
fn halfblock_trails_use_half_block() {
    let mut prev = Grid::new(1, 2);
    prev.set(0, 0, true);
    let next = Grid::new(1, 2);
    let mut state = GolRenderState::new();
    state.seed_from_grid(&prev);
    state.update_from_step(&prev, &next);
    let palette = GolPalette::from_theme(&Theme::default());
    let metrics = GolHudMetrics::new(1);
    let hud = hud_state(&metrics, 1, 0, 1);
    let cfg = default_cfg(true);
    let mut buf = Buffer::empty(AREA_1X2);
    let mut renderer = HalfBlockRenderer;
    renderer.render(AREA_1X2, &mut buf, &next, &state, &cfg, &palette, &hud);
    assert_eq!(buf.get(0, 1).symbol(), "▀");
}
