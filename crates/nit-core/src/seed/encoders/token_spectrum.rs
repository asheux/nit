use crate::seed::grid_types::{SeedEncoder, SeedInput, SeedValueGrid};
use crate::seed::utils::{apply_structural_noise, hilbert_index_to_xy, normalize_grid};
use crate::seed::view_modes::SeedEncoderId;

use super::structural::{
    byte_category_value, seed_highlight_bytes, seed_highlight_to_value, seed_parse,
};

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
        let bytes = input.text.as_bytes();

        if bytes.is_empty() {
            return grid;
        }

        // Per-byte values from tree-sitter highlight groups (or fallback),
        // filtering whitespace so only meaningful tokens fill the grid.
        let values: Vec<u8> = match seed_parse(input.text, input.file_path) {
            Some((tree, lang)) => {
                let groups = seed_highlight_bytes(input.text, lang, &tree);
                groups
                    .iter()
                    .filter(|g| g.is_some())
                    .map(|g| seed_highlight_to_value(*g))
                    .collect()
            }
            None => bytes
                .iter()
                .filter(|&&b| !matches!(b, b'\n' | b'\r' | b'\t' | b' '))
                .map(|&b| byte_category_value(b))
                .collect(),
        };

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
        apply_structural_noise(&mut grid, size, seed_nonce, bytes, variant);

        grid
    }
}
