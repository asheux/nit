use ratatui::{buffer::Buffer, layout::Rect};

use nit_gol::Grid;

use super::palette::GolPalette;
use super::renderer::{
    draw_bbox, live_color, neighbor_count, render_hud_line, row_bg, trail_color, GolHudState,
    GolRenderConfig, GolRenderState, GolRenderer,
};

#[derive(Default)]
pub struct BrailleRenderer;

const BRAILLE_BITS: [[u8; 2]; 4] = [
    [0x01, 0x08],
    [0x02, 0x10],
    [0x04, 0x20],
    [0x40, 0x80],
];

impl GolRenderer for BrailleRenderer {
    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        grid: &Grid,
        state: &GolRenderState,
        cfg: &GolRenderConfig,
        palette: &GolPalette,
        hud: &GolHudState<'_>,
    ) {
        render_hud_line(area, buf, palette, hud);
        let grid_area = Rect {
            x: area.x,
            y: area.y.saturating_add(1),
            width: area.width,
            height: area.height.saturating_sub(1),
        };
        if grid_area.width == 0 || grid_area.height == 0 {
            return;
        }

        let grid_w = grid.width();
        let grid_h = grid.height();
        let cells = grid.cells();
        let age = state.age();
        let decay = state.decay();

        let mut min_x = usize::MAX;
        let mut min_y = usize::MAX;
        let mut max_x = 0usize;
        let mut max_y = 0usize;
        let mut any_alive = false;

        for row in 0..grid_area.height as usize {
            let y = grid_area.y + row as u16;
            let bg = row_bg(row, cfg, palette);
            let base_y = row.saturating_mul(4);
            for col in 0..grid_area.width as usize {
                let x = grid_area.x + col as u16;
                let base_x = col.saturating_mul(2);
                let mut mask = 0u8;
                let mut max_age = 0u8;
                let mut max_decay = 0u8;
                let mut max_neighbors = 0u8;
                let mut any_alive_block = false;

                for dy in 0..4 {
                    let gy = base_y + dy;
                    if gy >= grid_h {
                        continue;
                    }
                    let row_start = gy.saturating_mul(grid_w);
                    for dx in 0..2 {
                        let gx = base_x + dx;
                        if gx >= grid_w {
                            continue;
                        }
                        let idx = row_start + gx;
                        let alive = cells[idx] != 0;
                        let decay_val = decay[idx];
                        let active = alive || (cfg.trails && decay_val > 0);
                        if active {
                            mask |= BRAILLE_BITS[dy][dx];
                        }
                        if alive {
                            any_alive_block = true;
                            max_age = max_age.max(age[idx]);
                            if cfg.overlay_heat {
                                max_neighbors = max_neighbors.max(neighbor_count(grid, gx, gy));
                            }
                            any_alive = true;
                            min_x = min_x.min(gx);
                            min_y = min_y.min(gy);
                            max_x = max_x.max(gx);
                            max_y = max_y.max(gy);
                        } else if cfg.trails && decay_val > 0 {
                            max_decay = max_decay.max(decay_val);
                        }
                    }
                }

                let cell = buf.get_mut(x, y);
                if mask == 0 {
                    cell.set_char(' ');
                    cell.set_bg(bg);
                    cell.set_fg(bg);
                } else {
                    let ch = char::from_u32(0x2800 + mask as u32).unwrap_or(' ');
                    let fg = if any_alive_block {
                        live_color(max_age, max_neighbors, cfg, palette)
                    } else {
                        trail_color(max_decay, palette)
                    };
                    cell.set_char(ch);
                    cell.set_fg(fg);
                    cell.set_bg(bg);
                }
            }
        }

        if cfg.overlay_bbox && any_alive {
            let left = min_x / 2;
            let right = max_x / 2;
            let top = min_y / 4;
            let bottom = max_y / 4;
            draw_bbox(grid_area, buf, left, top, right, bottom, cfg, palette);
        }
    }
}
