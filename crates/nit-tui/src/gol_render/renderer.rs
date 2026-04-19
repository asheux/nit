use ratatui::{
    buffer::{Buffer, Cell},
    layout::Rect,
    style::Color,
    widgets::Widget,
};

use nit_core::GolRenderMode;
use nit_gol::Grid;

use super::braille::BrailleRenderer;
use super::geometry::{RenderGeometry, RenderMode};
use super::halfblock::HalfBlockRenderer;
use super::palette::GolPalette;
use super::solid::SolidRenderer;
use super::state::{GolHudState, GolRenderConfig, GolRenderState};

pub trait GolRenderer {
    #[allow(clippy::too_many_arguments)]
    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        grid: &Grid,
        state: &GolRenderState,
        cfg: &GolRenderConfig,
        palette: &GolPalette,
        hud: &GolHudState<'_>,
    );
}

#[derive(Default)]
pub struct GolRenderPipeline {
    solid: SolidRenderer,
    half: HalfBlockRenderer,
    braille: BrailleRenderer,
}

impl GolRenderPipeline {
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        grid: &Grid,
        state: &GolRenderState,
        cfg: &GolRenderConfig,
        palette: &GolPalette,
        hud: &GolHudState<'_>,
    ) {
        match cfg.mode.effective(cfg.braille_enabled) {
            GolRenderMode::Solid => self.solid.render(area, buf, grid, state, cfg, palette, hud),
            GolRenderMode::HalfBlock => self.half.render(area, buf, grid, state, cfg, palette, hud),
            GolRenderMode::Braille => self
                .braille
                .render(area, buf, grid, state, cfg, palette, hud),
        }
    }
}

pub fn grid_size_for_mode(width: usize, height: usize, mode: GolRenderMode) -> (usize, usize) {
    let term_rect = Rect {
        x: 0,
        y: 0,
        width: width.min(u16::MAX as usize) as u16,
        height: height.min(u16::MAX as usize) as u16,
    };
    let geom = RenderGeometry::for_mode(RenderMode::from(mode), term_rect, 0, 0);
    (geom.gol_w as usize, geom.gol_h as usize)
}

pub(crate) fn grid_area_below_hud(area: Rect) -> Rect {
    Rect {
        x: area.x,
        y: area.y.saturating_add(1),
        width: area.width,
        height: area.height.saturating_sub(1),
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum HalfFill {
    Top,
    Bottom,
    Both,
}

impl HalfFill {
    pub fn from_pair(top: bool, bottom: bool) -> Option<Self> {
        match (top, bottom) {
            (true, true) => Some(Self::Both),
            (true, false) => Some(Self::Top),
            (false, true) => Some(Self::Bottom),
            (false, false) => None,
        }
    }

    pub fn glyph(self) -> char {
        match self {
            Self::Both => '█',
            Self::Top => '▀',
            Self::Bottom => '▄',
        }
    }

    pub fn bg(self, bg_top: Color, bg_bottom: Color) -> Color {
        match self {
            Self::Both | Self::Top => bg_bottom,
            Self::Bottom => bg_top,
        }
    }
}

pub(crate) fn draw_checker_or_empty(
    cell: &mut Cell,
    bg_top: Color,
    bg_bottom: Color,
    use_checker: bool,
) {
    if use_checker {
        cell.set_char('▀');
        cell.set_fg(bg_top);
        cell.set_bg(bg_bottom);
    } else {
        cell.set_char(' ');
        cell.set_fg(bg_bottom);
        cell.set_bg(bg_bottom);
    }
}

pub(crate) fn neighbor_count(grid: &Grid, x: usize, y: usize) -> u8 {
    let width = grid.width();
    let height = grid.height();
    if width == 0 || height == 0 {
        return 0;
    }
    let x0 = x.saturating_sub(1);
    let y0 = y.saturating_sub(1);
    let x1 = (x + 1).min(width - 1);
    let y1 = (y + 1).min(height - 1);
    let mut count = 0u8;
    for yy in y0..=y1 {
        for xx in x0..=x1 {
            let is_self = xx == x && yy == y;
            if !is_self && grid.get(xx, yy) {
                count = count.saturating_add(1);
            }
        }
    }
    count
}

pub struct GolWidget<'a> {
    pub grid: &'a Grid,
    pub state: &'a GolRenderState,
    pub cfg: GolRenderConfig,
    pub palette: GolPalette,
    pub hud: GolHudState<'a>,
}

impl Widget for GolWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        GolRenderPipeline::default().render(
            area,
            buf,
            self.grid,
            self.state,
            &self.cfg,
            &self.palette,
            &self.hud,
        );
    }
}
