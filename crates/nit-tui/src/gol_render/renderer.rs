use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

use nit_core::{GolRenderMode, VisualizerMode};
use nit_gol::Grid;

use super::{braille::BrailleRenderer, halfblock::HalfBlockRenderer, palette::GolPalette, solid::SolidRenderer};

pub const MAX_AGE: u8 = 12;
pub const MAX_DECAY: u8 = 10;
pub const HUD_HISTORY_LEN: usize = 32;

#[derive(Clone, Copy, Debug)]
pub struct GolRenderConfig {
    pub mode: GolRenderMode,
    pub age_shading: bool,
    pub trails: bool,
    pub overlay_bbox: bool,
    pub overlay_heat: bool,
    pub scanlines: bool,
    pub braille_enabled: bool,
}

impl GolRenderConfig {
    pub fn effective_mode(&self) -> GolRenderMode {
        self.mode.effective(self.braille_enabled)
    }
}

#[derive(Clone, Debug)]
pub struct GolRenderState {
    width: usize,
    height: usize,
    age: Vec<u8>,
    decay: Vec<u8>,
    hud: GolHudMetrics,
}

impl GolRenderState {
    pub fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            age: Vec::new(),
            decay: Vec::new(),
            hud: GolHudMetrics::new(HUD_HISTORY_LEN),
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        if self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        let len = width.saturating_mul(height);
        self.age.resize(len, 0);
        self.decay.resize(len, 0);
        self.hud.reset();
    }

    pub fn seed_from_grid(&mut self, grid: &Grid) -> usize {
        let width = grid.width();
        let height = grid.height();
        self.resize(width, height);
        let cells = grid.cells();
        let mut alive = 0usize;
        for (idx, &cell) in cells.iter().enumerate() {
            if cell != 0 {
                alive += 1;
                self.age[idx] = 1;
            } else {
                self.age[idx] = 0;
            }
            self.decay[idx] = 0;
        }
        self.hud.reset();
        self.hud.push_alive(alive);
        alive
    }

    pub fn update_from_step(&mut self, prev: &Grid, next: &Grid) -> (usize, u32) {
        let width = next.width();
        let height = next.height();
        if self.width != width || self.height != height {
            self.resize(width, height);
        }
        let prev_cells = prev.cells();
        let next_cells = next.cells();
        let mut alive = 0usize;
        let mut delta = 0u32;
        for idx in 0..next_cells.len() {
            let was_alive = prev_cells[idx] != 0;
            let is_alive = next_cells[idx] != 0;
            if was_alive != is_alive {
                delta = delta.saturating_add(1);
            }
            if is_alive {
                alive += 1;
            }
            if is_alive {
                let age = self.age[idx];
                self.age[idx] = if was_alive {
                    age.saturating_add(1).min(MAX_AGE)
                } else {
                    1
                };
                self.decay[idx] = 0;
            } else {
                self.age[idx] = 0;
                if was_alive {
                    self.decay[idx] = MAX_DECAY;
                } else if self.decay[idx] > 0 {
                    self.decay[idx] = self.decay[idx].saturating_sub(1);
                }
            }
        }
        self.hud.delta = delta;
        self.hud.push_alive(alive);
        (alive, delta)
    }

    pub fn age(&self) -> &[u8] {
        &self.age
    }

    pub fn decay(&self) -> &[u8] {
        &self.decay
    }

    pub fn hud_metrics(&self) -> &GolHudMetrics {
        &self.hud
    }
}

#[derive(Clone, Debug)]
pub struct GolHudMetrics {
    history: AliveHistory,
    delta: u32,
}

impl GolHudMetrics {
    pub fn new(history_len: usize) -> Self {
        Self {
            history: AliveHistory::new(history_len),
            delta: 0,
        }
    }

    pub fn reset(&mut self) {
        self.history.clear();
        self.delta = 0;
    }

    pub fn push_alive(&mut self, alive: usize) {
        let value = alive.min(u16::MAX as usize) as u16;
        self.history.push(value);
    }

    pub fn history(&self) -> &AliveHistory {
        &self.history
    }

    pub fn delta(&self) -> u32 {
        self.delta
    }
}

#[derive(Clone, Debug)]
pub struct AliveHistory {
    buf: Vec<u16>,
    head: usize,
    filled: bool,
}

impl AliveHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0; capacity],
            head: 0,
            filled: false,
        }
    }

    pub fn push(&mut self, value: u16) {
        if self.buf.is_empty() {
            return;
        }
        self.buf[self.head] = value;
        self.head = (self.head + 1) % self.buf.len();
        if self.head == 0 {
            self.filled = true;
        }
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.filled = false;
    }

    pub fn len(&self) -> usize {
        if self.filled {
            self.buf.len()
        } else {
            self.head
        }
    }

    pub fn get(&self, idx: usize) -> Option<u16> {
        let len = self.len();
        if idx >= len {
            return None;
        }
        if !self.filled {
            return Some(self.buf[idx]);
        }
        let pos = (self.head + idx) % self.buf.len();
        Some(self.buf[pos])
    }
}

pub struct GolHudState<'a> {
    pub rule: &'a str,
    pub generation: u64,
    pub alive: usize,
    pub period: Option<u32>,
    pub mode: VisualizerMode,
    pub paused: bool,
    pub delta: u32,
    pub history: &'a AliveHistory,
}

pub trait GolRenderer {
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
        match cfg.effective_mode() {
            GolRenderMode::Solid => self
                .solid
                .render(area, buf, grid, state, cfg, palette, hud),
            GolRenderMode::HalfBlock => self
                .half
                .render(area, buf, grid, state, cfg, palette, hud),
            GolRenderMode::Braille => self
                .braille
                .render(area, buf, grid, state, cfg, palette, hud),
        }
    }
}

pub fn grid_size_for_mode(width: usize, height: usize, mode: GolRenderMode) -> (usize, usize) {
    match mode {
        GolRenderMode::Solid => (width, height),
        GolRenderMode::HalfBlock => (width, height.saturating_mul(2)),
        GolRenderMode::Braille => (width.saturating_mul(2), height.saturating_mul(4)),
    }
}

pub(crate) fn render_hud_line(
    area: Rect,
    buf: &mut Buffer,
    palette: &GolPalette,
    hud: &GolHudState<'_>,
) {
    if area.height == 0 {
        return;
    }
    let y = area.y;
    let max_x = area.x.saturating_add(area.width);
    let label_style = Style::default().fg(palette.hud_dim).bg(palette.bg).add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(palette.hud_text).bg(palette.bg);
    let sep_style = Style::default().fg(palette.hud_dim).bg(palette.bg).add_modifier(Modifier::DIM);

    for x in area.x..max_x {
        let cell = buf.get_mut(x, y);
        cell.set_char(' ');
        cell.set_bg(palette.bg);
        cell.set_fg(palette.hud_dim);
    }

    let mut x = area.x;
    x = write_str(buf, x, y, max_x, label_style, "Rule: ");
    x = write_str(buf, x, y, max_x, value_style, hud.rule);
    x = write_str(buf, x, y, max_x, sep_style, " | ");
    x = write_str(buf, x, y, max_x, label_style, "Gen: ");
    x = write_u64(buf, x, y, max_x, value_style, hud.generation, 5);
    x = write_str(buf, x, y, max_x, sep_style, " | ");
    x = write_str(buf, x, y, max_x, label_style, "Alive: ");
    x = write_usize(buf, x, y, max_x, value_style, hud.alive, 4);
    x = write_str(buf, x, y, max_x, sep_style, " | ");
    x = write_str(buf, x, y, max_x, label_style, "Δ: ");
    x = write_u32(buf, x, y, max_x, value_style, hud.delta, 3);
    x = write_str(buf, x, y, max_x, sep_style, " | ");
    x = write_str(buf, x, y, max_x, label_style, "Period: ");
    if let Some(period) = hud.period {
        x = write_u32(buf, x, y, max_x, value_style, period, 2);
    } else {
        x = write_str(buf, x, y, max_x, value_style, "--");
    }
    x = write_str(buf, x, y, max_x, sep_style, " | ");
    x = write_str(buf, x, y, max_x, label_style, "Mode: ");
    let mode_label = match hud.mode {
        VisualizerMode::SimOnly => "SIM",
        VisualizerMode::Search => "SEARCH",
    };
    x = write_str(buf, x, y, max_x, value_style, mode_label);
    if hud.paused {
        x = write_str(buf, x, y, max_x, sep_style, " PAUSED");
    }

    if x < max_x.saturating_sub(2) {
        x = write_str(buf, x, y, max_x, sep_style, " | ");
        let spark_style = Style::default().fg(palette.hud_spark).bg(palette.bg);
        let _ = write_sparkline(buf, x, y, max_x, spark_style, hud.history);
    }
}

pub(crate) fn live_color(
    age: u8,
    neighbors: u8,
    cfg: &GolRenderConfig,
    palette: &GolPalette,
) -> Color {
    let base = if cfg.age_shading {
        match age {
            0 | 1 => palette.live_new,
            2..=4 => palette.live,
            _ => palette.live_old,
        }
    } else {
        palette.live
    };

    if !cfg.overlay_heat {
        return base;
    }

    match neighbors {
        0..=1 => palette.live_old,
        2 | 3 => base,
        4 | 5 => palette.live,
        _ => palette.live_new,
    }
}

pub(crate) fn trail_color(decay: u8, palette: &GolPalette) -> Color {
    if decay == 0 {
        return palette.bg;
    }
    let steps = palette.trail.len().max(1) as u8;
    let idx = ((decay.saturating_sub(1)) * steps) / MAX_DECAY.max(1);
    let clamped = idx.min((palette.trail.len() - 1) as u8) as usize;
    palette.trail[clamped]
}

pub(crate) fn row_bg(row: usize, cfg: &GolRenderConfig, palette: &GolPalette) -> Color {
    if cfg.scanlines && row % 2 == 1 {
        palette.scanline
    } else {
        palette.bg
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
            if xx == x && yy == y {
                continue;
            }
            if grid.get(xx, yy) {
                count = count.saturating_add(1);
            }
        }
    }
    count
}

pub(crate) fn draw_bbox(
    grid_area: Rect,
    buf: &mut Buffer,
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
    cfg: &GolRenderConfig,
    palette: &GolPalette,
) {
    if grid_area.width == 0 || grid_area.height == 0 {
        return;
    }
    if right < left || bottom < top {
        return;
    }
    let max_x = (grid_area.width as usize).saturating_sub(1);
    let max_y = (grid_area.height as usize).saturating_sub(1);
    let left = left.min(max_x) as u16;
    let right = right.min(max_x) as u16;
    let top = top.min(max_y) as u16;
    let bottom = bottom.min(max_y) as u16;
    if left > right || top > bottom {
        return;
    }

    let style = Style::default()
        .fg(palette.bbox)
        .add_modifier(Modifier::DIM);

    for x in left..=right {
        let y = top;
        let bg = row_bg(y as usize, cfg, palette);
        let cell = buf.get_mut(grid_area.x + x, grid_area.y + y);
        cell.set_char(if x == left { '┌' } else if x == right { '┐' } else { '─' });
        cell.set_style(style);
        cell.set_bg(bg);
    }

    if bottom != top {
        for x in left..=right {
            let y = bottom;
            let bg = row_bg(y as usize, cfg, palette);
            let cell = buf.get_mut(grid_area.x + x, grid_area.y + y);
            cell.set_char(if x == left { '└' } else if x == right { '┘' } else { '─' });
            cell.set_style(style);
            cell.set_bg(bg);
        }
    }

    for y in (top + 1)..bottom {
        let bg = row_bg(y as usize, cfg, palette);
        let cell_left = buf.get_mut(grid_area.x + left, grid_area.y + y);
        cell_left.set_char('│');
        cell_left.set_style(style);
        cell_left.set_bg(bg);
        if right != left {
            let cell_right = buf.get_mut(grid_area.x + right, grid_area.y + y);
            cell_right.set_char('│');
            cell_right.set_style(style);
            cell_right.set_bg(bg);
        }
    }
}

fn write_str(
    buf: &mut Buffer,
    mut x: u16,
    y: u16,
    max_x: u16,
    style: Style,
    text: &str,
) -> u16 {
    for ch in text.chars() {
        if x >= max_x {
            break;
        }
        let cell = buf.get_mut(x, y);
        cell.set_char(ch);
        cell.set_style(style);
        x = x.saturating_add(1);
    }
    x
}

fn write_u64(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    max_x: u16,
    style: Style,
    value: u64,
    min_width: usize,
) -> u16 {
    write_number(buf, x, y, max_x, style, value, min_width)
}

fn write_u32(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    max_x: u16,
    style: Style,
    value: u32,
    min_width: usize,
) -> u16 {
    write_number(buf, x, y, max_x, style, value as u64, min_width)
}

fn write_usize(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    max_x: u16,
    style: Style,
    value: usize,
    min_width: usize,
) -> u16 {
    write_number(buf, x, y, max_x, style, value as u64, min_width)
}

fn write_number(
    buf: &mut Buffer,
    mut x: u16,
    y: u16,
    max_x: u16,
    style: Style,
    mut value: u64,
    min_width: usize,
) -> u16 {
    let mut digits = [0u8; 20];
    let mut len = 0usize;
    if value == 0 {
        digits[len] = b'0';
        len += 1;
    } else {
        while value > 0 {
            digits[len] = b'0' + (value % 10) as u8;
            len += 1;
            value /= 10;
        }
    }
    let pad = min_width.saturating_sub(len);
    for _ in 0..pad {
        if x >= max_x {
            return x;
        }
        let cell = buf.get_mut(x, y);
        cell.set_char('0');
        cell.set_style(style);
        x = x.saturating_add(1);
    }
    for idx in (0..len).rev() {
        if x >= max_x {
            break;
        }
        let cell = buf.get_mut(x, y);
        cell.set_char(digits[idx] as char);
        cell.set_style(style);
        x = x.saturating_add(1);
    }
    x
}

fn write_sparkline(
    buf: &mut Buffer,
    mut x: u16,
    y: u16,
    max_x: u16,
    style: Style,
    history: &AliveHistory,
) -> u16 {
    let len = history.len();
    if len == 0 {
        return x;
    }
    let mut min = u16::MAX;
    let mut max = 0u16;
    for i in 0..len {
        if let Some(val) = history.get(i) {
            min = min.min(val);
            max = max.max(val);
        }
    }
    let bars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    for i in 0..len {
        if x >= max_x {
            break;
        }
        let value = history.get(i).unwrap_or(0);
        let level = if max == min {
            4
        } else {
            let span = (max - min).max(1) as u32;
            let scaled = ((value.saturating_sub(min)) as u32 * 7) / span;
            scaled as usize
        };
        let ch = bars[level.min(7)];
        let cell = buf.get_mut(x, y);
        cell.set_char(ch);
        cell.set_style(style);
        x = x.saturating_add(1);
    }
    x
}
