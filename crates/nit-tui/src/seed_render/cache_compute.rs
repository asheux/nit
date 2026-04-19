use nit_core::seed::SeedBits;
use nit_core::{EncodedSeed, SeedEncoderId};
use nit_gol::Grid;

use super::hilbert;

pub(super) const DEFAULT_DENSITY_BLOCK: usize = 4;

const HILBERT_INSET_SIZE: usize = 16;

#[derive(Clone, Debug, Default)]
pub struct BBox {
    pub id: u16,
    pub min_x: usize,
    pub min_y: usize,
    pub max_x: usize,
    pub max_y: usize,
    pub cells: u32,
}

// Precomputed, frame-invariant seed state shared across renderers. Populated once per
// (seed_hash, grid size) change — everything stored here is expensive enough that
// recomputing per frame would stall the redraw loop.
#[derive(Clone, Debug)]
pub struct SeedRenderCache {
    pub seed_hash: u64,
    pub grid_width: usize,
    pub grid_height: usize,
    pub genome_live: usize,
    pub genome_total: usize,
    pub genome_density: f32,
    pub ascii_printable: usize,
    pub ascii_nonprintable: usize,
    pub component_map: Option<Vec<u16>>,
    pub component_bboxes: Vec<BBox>,
    pub local_density: Option<Vec<u8>>,
    pub density_stride: usize,
    pub density_block: usize,
    pub halo_mask: Option<Vec<u8>>,
    pub inset_16x16: Option<SeedBits>,
    pub hilbert_stream: Option<Vec<u8>>,
    pub hilbert_index_by_xy: Option<Vec<u32>>,
    pub hilbert_path_inset: Option<Vec<u8>>,
    pub hilbert_order: u32,
    pub scanline_phase: u16,
}

impl Default for SeedRenderCache {
    fn default() -> Self {
        Self {
            seed_hash: 0,
            grid_width: 0,
            grid_height: 0,
            genome_live: 0,
            genome_total: 0,
            genome_density: 0.0,
            ascii_printable: 0,
            ascii_nonprintable: 0,
            component_map: None,
            component_bboxes: Vec::new(),
            local_density: None,
            density_stride: 0,
            density_block: DEFAULT_DENSITY_BLOCK,
            halo_mask: None,
            inset_16x16: None,
            hilbert_stream: None,
            hilbert_index_by_xy: None,
            hilbert_path_inset: None,
            hilbert_order: 0,
            scanline_phase: 0,
        }
    }
}

impl SeedRenderCache {
    pub fn update(&mut self, seed: &EncodedSeed) {
        let grid_w = seed.grid.width();
        let grid_h = seed.grid.height();
        if self.seed_hash == seed.seed_hash
            && self.grid_width == grid_w
            && self.grid_height == grid_h
        {
            return;
        }
        self.seed_hash = seed.seed_hash;
        self.grid_width = grid_w;
        self.grid_height = grid_h;

        let (map, mut bboxes) = compute_components(&seed.grid);
        bboxes.sort_by(|a, b| b.cells.cmp(&a.cells));
        self.component_map = map;
        self.component_bboxes = bboxes;

        let (density, stride, block) = compute_density(&seed.grid, self.density_block.max(2));
        self.local_density = density;
        self.density_stride = stride;
        self.density_block = block;

        self.halo_mask = compute_halo(&seed.grid);

        let is_lifehash_inset = seed.encoder_id.as_str() == "lifehash16"
            && seed.base_bits_raw.width() == 16
            && seed.base_bits_raw.height() == 16;
        self.inset_16x16 = is_lifehash_inset.then(|| seed.base_bits_raw.clone());

        populate_hilbert(self, seed);
        populate_genome_stats(self, &seed.base_bits_raw);
        populate_ascii_stats(self, seed);
    }
}

fn populate_hilbert(cache: &mut SeedRenderCache, seed: &EncodedSeed) {
    cache.hilbert_stream = None;
    cache.hilbert_index_by_xy = None;
    cache.hilbert_path_inset = None;
    cache.hilbert_order = 0;
    if seed.encoder_id != SeedEncoderId::HilbertBits {
        return;
    }
    let w = seed.base_bits.width().max(1);
    let h = seed.base_bits.height().max(1);
    let total = w.saturating_mul(h);
    if total == 0 {
        return;
    }
    let order = hilbert::order_for_width(w);
    let HilbertBuffers {
        stream,
        index_by_xy,
        path_inset,
    } = build_hilbert_buffers(seed, w, h, total, order);
    cache.hilbert_stream = Some(stream);
    cache.hilbert_index_by_xy = Some(index_by_xy);
    cache.hilbert_path_inset = Some(path_inset);
    cache.hilbert_order = order;
}

fn populate_genome_stats(cache: &mut SeedRenderCache, bits: &SeedBits) {
    let total = bits.width().saturating_mul(bits.height());
    let mut live = 0usize;
    for gy in 0..bits.height() {
        for gx in 0..bits.width() {
            if bits.get(gx, gy) {
                live += 1;
            }
        }
    }
    cache.genome_live = live;
    cache.genome_total = total.max(1);
    cache.genome_density = live as f32 / cache.genome_total as f32;
}

fn populate_ascii_stats(cache: &mut SeedRenderCache, seed: &EncodedSeed) {
    let values = &seed.base_values;
    let mut printable = 0usize;
    let mut nonprintable = 0usize;
    for vy in 0..values.height() {
        for vx in 0..values.width() {
            if (0x20u8..=0x7eu8).contains(&values.get(vx, vy)) {
                printable += 1;
            } else {
                nonprintable += 1;
            }
        }
    }
    cache.ascii_printable = printable;
    cache.ascii_nonprintable = nonprintable;
}

struct HilbertBuffers {
    stream: Vec<u8>,
    index_by_xy: Vec<u32>,
    path_inset: Vec<u8>,
}

fn build_hilbert_buffers(
    seed: &EncodedSeed,
    w: usize,
    h: usize,
    total: usize,
    order: u32,
) -> HilbertBuffers {
    let inset = HILBERT_INSET_SIZE;
    let mut stream = vec![0u8; total];
    let mut index_by_xy = vec![0u32; total];
    let mut path_inset = vec![0u8; inset * inset];
    let denom = total.saturating_sub(1).max(1) as u32;
    for (idx, cell) in stream.iter_mut().enumerate() {
        let idx_u32 = idx as u32;
        let (x, y) = hilbert::index_to_xy(order, idx_u32);
        let xi = x as usize;
        let yi = y as usize;
        if xi >= w || yi >= h {
            continue;
        }
        *cell = u8::from(seed.base_bits.get(xi, yi));
        index_by_xy[yi * w + xi] = idx_u32;
        let ix = xi.saturating_mul(inset) / w;
        let iy = yi.saturating_mul(inset) / h;
        let intensity = (idx_u32.saturating_mul(255) / denom) as u8;
        let inset_idx = iy.saturating_mul(inset) + ix;
        if inset_idx < path_inset.len() && intensity > path_inset[inset_idx] {
            path_inset[inset_idx] = intensity;
        }
    }
    HilbertBuffers {
        stream,
        index_by_xy,
        path_inset,
    }
}

fn compute_components(grid: &Grid) -> (Option<Vec<u16>>, Vec<BBox>) {
    let w = grid.width();
    let h = grid.height();
    if w == 0 || h == 0 {
        return (None, Vec::new());
    }
    let mut map = vec![u16::MAX; w * h];
    let mut bboxes = Vec::new();
    let mut stack = Vec::new();
    let mut next_id: u16 = 0;

    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if grid.cells()[idx] == 0 || map[idx] != u16::MAX {
                continue;
            }
            // u16::MAX is the sentinel for "no component" — stop assigning IDs
            // once we'd collide with it, even if more live cells remain.
            if next_id == u16::MAX {
                break;
            }
            let bbox = flood_fill(grid, &mut map, &mut stack, next_id, x, y);
            bboxes.push(bbox);
            next_id = next_id.wrapping_add(1);
        }
    }

    if bboxes.is_empty() {
        (None, Vec::new())
    } else {
        (Some(map), bboxes)
    }
}

fn flood_fill(
    grid: &Grid,
    map: &mut [u16],
    stack: &mut Vec<(usize, usize)>,
    id: u16,
    sx: usize,
    sy: usize,
) -> BBox {
    let w = grid.width();
    let h = grid.height();
    let mut bbox = BBox {
        id,
        min_x: sx,
        min_y: sy,
        max_x: sx,
        max_y: sy,
        cells: 0,
    };
    map[sy * w + sx] = id;
    stack.push((sx, sy));
    while let Some((cx, cy)) = stack.pop() {
        bbox.cells = bbox.cells.saturating_add(1);
        bbox.min_x = bbox.min_x.min(cx);
        bbox.min_y = bbox.min_y.min(cy);
        bbox.max_x = bbox.max_x.max(cx);
        bbox.max_y = bbox.max_y.max(cy);
        enqueue_live_neighbors(grid, map, stack, id, cx, cy, w, h);
    }
    bbox
}

#[allow(clippy::too_many_arguments)]
fn enqueue_live_neighbors(
    grid: &Grid,
    map: &mut [u16],
    stack: &mut Vec<(usize, usize)>,
    id: u16,
    cx: usize,
    cy: usize,
    w: usize,
    h: usize,
) {
    let x0 = cx.saturating_sub(1);
    let x1 = (cx + 1).min(w - 1);
    let y0 = cy.saturating_sub(1);
    let y1 = (cy + 1).min(h - 1);
    for ny in y0..=y1 {
        for nx in x0..=x1 {
            let nidx = ny * w + nx;
            if grid.cells()[nidx] == 0 || map[nidx] != u16::MAX {
                continue;
            }
            map[nidx] = id;
            stack.push((nx, ny));
        }
    }
}

fn compute_density(grid: &Grid, block: usize) -> (Option<Vec<u8>>, usize, usize) {
    let w = grid.width();
    let h = grid.height();
    if w == 0 || h == 0 {
        return (None, 0, block);
    }
    let block = block.max(2);
    let bw = w.div_ceil(block);
    let bh = h.div_ceil(block);
    let mut density = vec![0u8; bw * bh];
    for y in 0..h {
        for x in 0..w {
            if grid.get(x, y) {
                let bx = x / block;
                let by = y / block;
                let idx = by * bw + bx;
                density[idx] = density[idx].saturating_add(1);
            }
        }
    }
    (Some(density), bw, block)
}

fn compute_halo(grid: &Grid) -> Option<Vec<u8>> {
    let w = grid.width();
    let h = grid.height();
    if w == 0 || h == 0 {
        return None;
    }
    let mut halo = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            if grid.get(x, y) {
                bump_halo_neighbors(&mut halo, grid, x, y, w, h);
            }
        }
    }
    Some(halo)
}

fn bump_halo_neighbors(halo: &mut [u8], grid: &Grid, cx: usize, cy: usize, w: usize, h: usize) {
    let x0 = cx.saturating_sub(1);
    let x1 = (cx + 1).min(w - 1);
    let y0 = cy.saturating_sub(1);
    let y1 = (cy + 1).min(h - 1);
    for ny in y0..=y1 {
        for nx in x0..=x1 {
            if (nx == cx && ny == cy) || grid.get(nx, ny) {
                continue;
            }
            let idx = ny * w + nx;
            halo[idx] = halo[idx].saturating_add(1);
        }
    }
}
