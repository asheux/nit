use crate::seed::grid_types::{SeedEncoder, SeedInput, SeedValueGrid};
use crate::seed::utils::{apply_structural_noise, hilbert_index_to_xy, normalize_grid};
use crate::seed::view_modes::SeedEncoderId;

use super::ast_features::{compute_ast_features, RoleBand};

pub(crate) struct TokenSpectrumEncoder;

impl SeedEncoder for TokenSpectrumEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::TokenSpectrum
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let order = 5u32;
        let size = 1usize << order;
        let total = size * size;
        let mut grid = SeedValueGrid::new(size, size);

        // One value per AST node, never per source byte. The old per-byte
        // fan-out scaled with identifier length / comment volume, so longer
        // names or extra comments shifted the chunk boundaries and moved
        // every cell. With one value per node, the projection is identifier-
        // and comment-invariant.
        let Some(features) = compute_ast_features(input.text, input.file_path) else {
            return grid;
        };
        let values: Vec<u8> = features
            .nodes
            .iter()
            .map(|node| role_band_to_value(node.role_band))
            .collect();

        if values.is_empty() {
            return grid;
        }

        let chunk = values.len().div_ceil(total).max(1);
        for cell in 0..total {
            let start = cell * chunk;
            if start >= values.len() {
                break;
            }
            let end = (start + chunk).min(values.len());
            let sum: u32 = values[start..end].iter().map(|&v| v as u32).sum();
            let avg = (sum / (end - start) as u32).min(255) as u8;
            let (x, y) = hilbert_index_to_xy(order, cell as u32);
            grid.set(x as usize, y as usize, avg);
        }

        normalize_grid(&mut grid);
        apply_structural_noise(&mut grid, size, seed_nonce, features.feature_hash, variant);

        grid
    }
}

// Map the 7 semantic bands onto contiguous 0-255 slices. The exact band
// values are arbitrary — the encoder's behaviour comes from the *spread*
// of values across grid chunks, not from any particular numeric meaning.
// Picked at ~32-unit intervals so each band lands in a distinct quadrant
// of the dynamic range, leaving headroom for `normalize_grid` to rescale.
fn role_band_to_value(band: RoleBand) -> u8 {
    match band {
        RoleBand::Declaration => 240,
        RoleBand::ControlFlow => 200,
        RoleBand::Expression => 160,
        RoleBand::Statement => 130,
        RoleBand::Type => 95,
        RoleBand::Literal => 60,
        RoleBand::Other => 25,
    }
}
