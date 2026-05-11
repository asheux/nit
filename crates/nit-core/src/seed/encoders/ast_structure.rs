use crate::seed::grid_types::{SeedEncoder, SeedInput, SeedValueGrid};
use crate::seed::utils::{apply_structural_noise, hilbert_index_to_xy, normalize_grid};
use crate::seed::view_modes::SeedEncoderId;

use super::ast_features::{compute_ast_features, AstFeatures};

pub(crate) struct AstStructureEncoder;

impl SeedEncoder for AstStructureEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::AstStructure
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let order = 5u32;
        let size = 1usize << order;
        let total = size * size;
        let mut grid = SeedValueGrid::new(size, size);

        let Some(features) = compute_ast_features(input.text, input.file_path) else {
            return grid;
        };
        if features.nodes.is_empty() {
            return grid;
        }
        fill_grid_from_features(&mut grid, &features, total, order);

        normalize_grid(&mut grid);
        apply_structural_noise(&mut grid, size, seed_nonce, features.feature_hash, variant);
        grid
    }
}

// One value per AST node, each value derived only from structural
// features: kind_weight (the existing 0-255 weight table), depth, and
// role_band. The prior implementation weighted cells by `byte_span` —
// `end_byte() - start_byte()` of the node's source text — which made the
// grid sensitive to identifier length, comment volume, and even
// whitespace inside the node. With every node contributing one cell, the
// projection depends only on what's in the AST.
fn fill_grid_from_features(
    grid: &mut SeedValueGrid,
    features: &AstFeatures,
    total: usize,
    order: u32,
) {
    let nodes = features.nodes.as_slice();
    let chunk = nodes.len().div_ceil(total).max(1);
    for cell in 0..total {
        let start = cell * chunk;
        if start >= nodes.len() {
            // Repeat the final cell value to fill the grid; the prior
            // behaviour did the same to avoid an all-zero tail that GoL
            // would treat as empty space.
            let fill = grid.get(0, 0);
            let (x, y) = hilbert_index_to_xy(order, cell as u32);
            grid.set(x as usize, y as usize, fill);
            continue;
        }
        let end = (start + chunk).min(nodes.len());
        let window = &nodes[start..end];
        // Mix kind_weight, depth, and role_band into a single 0-255 score
        // for the cell. Weights track the prior distribution (kind=30%,
        // depth=30%, branch≈role_band as a structural proxy=25%, plus a
        // baseline of 15) — the absolute numbers are arbitrary; what
        // matters is the spread across cells.
        let avg_kind: u64 =
            window.iter().map(|n| n.kind_weight as u64).sum::<u64>() / window.len() as u64;
        let max_depth: u64 =
            (window.iter().map(|n| n.depth as u64).max().unwrap_or(0) * 17).min(255);
        let band_spread: u64 = {
            let mut seen = [false; super::ast_features::ROLE_BAND_COUNT];
            for node in window {
                seen[node.role_band.as_u8() as usize] = true;
            }
            (seen.iter().filter(|s| **s).count() as u64 * 255)
                / super::ast_features::ROLE_BAND_COUNT as u64
        };
        let value = ((avg_kind * 30 + max_depth * 30 + band_spread * 25 + 15 * 38) / 100)
            .clamp(0, 255) as u8;
        let (x, y) = hilbert_index_to_xy(order, cell as u32);
        grid.set(x as usize, y as usize, value);
    }
}
