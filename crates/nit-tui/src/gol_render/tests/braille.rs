use super::BrailleRenderer;
use crate::gol_render::renderer::GolRenderer;
use crate::gol_render::{GolHudMetrics, GolHudState, GolPalette, GolRenderConfig, GolRenderState};
use crate::theme::Theme;
use nit_core::{GolRenderMode, VisualizerMode};
use nit_gol::Grid;
use ratatui::{buffer::Buffer, layout::Rect};

#[test]
fn braille_uniform_pixels_use_half_block() {
    let mut grid = Grid::new(2, 4);
    grid.set(1, 3, true);
    let mut state = GolRenderState::new();
    state.seed_from_grid(&grid);
    let palette = GolPalette::from_theme(&Theme::default());
    let metrics = GolHudMetrics::new(1);
    let hud = GolHudState {
        rule: "B3/S23",
        generation: 0,
        alive: 1,
        period: None,
        mode: VisualizerMode::SimOnly,
        paused: false,
        delta: 0,
        history: metrics.history(),
    };
    let cfg = GolRenderConfig {
        mode: GolRenderMode::Braille,
        age_shading: false,
        trails: false,
        overlay_bbox: false,
        overlay_heat: false,
        scanlines: false,
        grid_minor: None,
        grid_major: None,
        gol_origin_x: 0,
        gol_origin_y: 0,
        debug_overlay: false,
        braille_enabled: true,
    };
    let area = Rect {
        x: 0,
        y: 0,
        width: 1,
        height: 2,
    };
    let mut buf = Buffer::empty(area);
    let mut renderer = BrailleRenderer;
    renderer.render(area, &mut buf, &grid, &state, &cfg, &palette, &hud);
    let cell = buf.get(0, 1);
    assert_eq!(cell.symbol(), "▄");
}

#[test]
fn braille_trails_use_half_block() {
    let mut prev = Grid::new(2, 4);
    prev.set(0, 0, true);
    let next = Grid::new(2, 4);
    let mut state = GolRenderState::new();
    state.seed_from_grid(&prev);
    state.update_from_step(&prev, &next);
    let palette = GolPalette::from_theme(&Theme::default());
    let metrics = GolHudMetrics::new(1);
    let hud = GolHudState {
        rule: "B3/S23",
        generation: 1,
        alive: 0,
        period: None,
        mode: VisualizerMode::SimOnly,
        paused: false,
        delta: 1,
        history: metrics.history(),
    };
    let cfg = GolRenderConfig {
        mode: GolRenderMode::Braille,
        age_shading: false,
        trails: true,
        overlay_bbox: false,
        overlay_heat: false,
        scanlines: false,
        grid_minor: None,
        grid_major: None,
        gol_origin_x: 0,
        gol_origin_y: 0,
        debug_overlay: false,
        braille_enabled: true,
    };
    let area = Rect {
        x: 0,
        y: 0,
        width: 1,
        height: 2,
    };
    let mut buf = Buffer::empty(area);
    let mut renderer = BrailleRenderer;
    renderer.render(area, &mut buf, &next, &state, &cfg, &palette, &hud);
    let cell = buf.get(0, 1);
    assert_eq!(cell.symbol(), "▀");
}
