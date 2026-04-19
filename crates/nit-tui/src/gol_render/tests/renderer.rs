use crate::gol_render::geometry::{RenderGeometry, RenderMode};
use crate::gol_render::overlay::{cell_bg_halves, gridline_flags};
use crate::gol_render::state::GolRenderConfig;
use crate::gol_render::GolPalette;
use nit_core::GolRenderMode;
use ratatui::layout::Rect;
use ratatui::style::Color;

fn test_palette() -> GolPalette {
    GolPalette {
        bg: Color::Rgb(10, 10, 10),
        live_new: Color::Rgb(40, 40, 40),
        live: Color::Rgb(30, 30, 30),
        live_old: Color::Rgb(20, 20, 20),
        trail: [
            Color::Rgb(12, 12, 12),
            Color::Rgb(11, 11, 11),
            Color::Rgb(10, 10, 10),
        ],
        bbox: Color::Rgb(50, 50, 50),
        hud_dim: Color::Rgb(60, 60, 60),
        hud_text: Color::Rgb(70, 70, 70),
        hud_spark: Color::Rgb(80, 80, 80),
        scanline: Color::Rgb(9, 9, 9),
    }
}

fn checker_cfg() -> GolRenderConfig {
    GolRenderConfig {
        mode: GolRenderMode::HalfBlock,
        age_shading: false,
        trails: false,
        overlay_bbox: false,
        overlay_heat: false,
        scanlines: false,
        grid_minor: Some(1),
        grid_major: None,
        gol_origin_x: 0,
        gol_origin_y: 0,
        debug_overlay: false,
        braille_enabled: true,
    }
}

fn expected_gridline(start: i32, count: u16, spacing: u16) -> bool {
    let spacing = spacing as i32;
    (0..count as i32).any(|offset| (start + offset) % spacing == 0)
}

const GRID_10X10: Rect = Rect {
    x: 0,
    y: 0,
    width: 10,
    height: 10,
};

#[test]
fn gridlines_match_gol_bounds() {
    let spacing = 4u16;
    for mode in [
        RenderMode::Solid,
        RenderMode::HalfBlock,
        RenderMode::Braille,
    ] {
        let geom = RenderGeometry::for_mode(mode, GRID_10X10, 0, 0);
        for ty in 0..GRID_10X10.height {
            for tx in 0..GRID_10X10.width {
                let (v, h) = gridline_flags(&geom, tx, ty, spacing);
                let (gx0, gy0, _gx1, _gy1) = geom.term_cell_bounds_in_gol(tx, ty);
                let expected_v = expected_gridline(gx0, geom.cell_per_term_x, spacing);
                let expected_h = expected_gridline(gy0, geom.cell_per_term_y, spacing);
                assert_eq!(
                    (v, h),
                    (expected_v, expected_h),
                    "mode={mode:?} tx={tx} ty={ty}",
                );
            }
        }
    }
}

#[test]
fn square_pixel_grid_minor_1_checkerboard() {
    let term_rect = Rect {
        x: 0,
        y: 0,
        width: 2,
        height: 1,
    };
    let geom = RenderGeometry::for_mode(RenderMode::HalfBlock, term_rect, 0, 0);
    let palette = test_palette();
    let cfg = checker_cfg();
    let (top0, bottom0) = cell_bg_halves(&geom, 0, 0, &cfg, &palette);
    let (top1, _bottom1) = cell_bg_halves(&geom, 1, 0, &cfg, &palette);
    assert_ne!(top0, bottom0);
    assert_ne!(top0, top1);
}
