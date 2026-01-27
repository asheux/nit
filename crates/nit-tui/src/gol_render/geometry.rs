use ratatui::layout::Rect;

use nit_core::GolRenderMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    Solid,
    HalfBlock,
    Braille,
}

impl From<GolRenderMode> for RenderMode {
    fn from(mode: GolRenderMode) -> Self {
        match mode {
            GolRenderMode::Solid => RenderMode::Solid,
            GolRenderMode::HalfBlock => RenderMode::HalfBlock,
            GolRenderMode::Braille => RenderMode::Braille,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RenderGeometry {
    pub mode: RenderMode,
    pub term_rect: Rect,
    pub cell_per_term_x: u16,
    pub cell_per_term_y: u16,
    pub gol_w: u16,
    pub gol_h: u16,
    pub gol_origin_x: i32,
    pub gol_origin_y: i32,
}

impl RenderGeometry {
    pub fn for_mode(
        mode: RenderMode,
        term_rect: Rect,
        gol_origin_x: i32,
        gol_origin_y: i32,
    ) -> Self {
        let (cell_per_term_x, cell_per_term_y) = match mode {
            RenderMode::Solid => (1, 1),
            RenderMode::HalfBlock => (1, 2),
            RenderMode::Braille => (2, 4),
        };
        let gol_w = term_rect.width.saturating_mul(cell_per_term_x);
        let gol_h = term_rect.height.saturating_mul(cell_per_term_y);
        Self {
            mode,
            term_rect,
            cell_per_term_x,
            cell_per_term_y,
            gol_w,
            gol_h,
            gol_origin_x,
            gol_origin_y,
        }
    }

    pub fn gol_to_term(&self, gx: i32, gy: i32) -> Option<(u16, u16)> {
        let rel_x = gx - self.gol_origin_x;
        let rel_y = gy - self.gol_origin_y;
        if rel_x < 0 || rel_y < 0 {
            return None;
        }
        if rel_x >= self.gol_w as i32 || rel_y >= self.gol_h as i32 {
            return None;
        }
        let tx = (rel_x / self.cell_per_term_x as i32) as u16;
        let ty = (rel_y / self.cell_per_term_y as i32) as u16;
        Some((tx, ty))
    }

    pub fn term_cell_bounds_in_gol(&self, tx: u16, ty: u16) -> (i32, i32, i32, i32) {
        let gx0 = self.gol_origin_x + (tx as i32) * (self.cell_per_term_x as i32);
        let gy0 = self.gol_origin_y + (ty as i32) * (self.cell_per_term_y as i32);
        let gx1 = gx0 + self.cell_per_term_x as i32;
        let gy1 = gy0 + self.cell_per_term_y as i32;
        (gx0, gy0, gx1, gy1)
    }
}
