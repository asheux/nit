//! `ComplexityFieldEncoder` — projects the AST chunked-metric tuple onto a
//! 32-row complexity heatmap.

use crate::seed::grid_types::{SeedEncoder, SeedInput, SeedValueGrid};
use crate::seed::utils::{apply_structural_noise, normalize_grid};
use crate::seed::view_modes::SeedEncoderId;

use crate::seed::encoders::ast_features::{compute_ast_features, AstFeatures, AstNodeFeature};

use super::metrics::chunk_metrics;

pub(crate) struct ComplexityFieldEncoder;

impl SeedEncoder for ComplexityFieldEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::ComplexityField
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let size = 32usize;
        let mut grid = SeedValueGrid::new(size, size);

        let Some(features) = compute_ast_features(input.text, input.file_path) else {
            return grid;
        };
        if features.nodes.is_empty() {
            return grid;
        }

        for (gy, window) in chunk_nodes(&features, size).iter().enumerate() {
            let m = chunk_metrics(window);
            let value =
                (m.nesting * 0.25 + m.cognitive * 0.30 + m.entropy * 0.25 + m.diversity * 0.20)
                    .clamp(0.0, 255.0) as u8;
            for gx in 0..size {
                grid.set(gx, gy, value);
            }
        }

        normalize_grid(&mut grid);
        apply_structural_noise(&mut grid, size, seed_nonce, features.feature_hash, variant);
        grid
    }
}

// Chunk by node-index, not source-line, so comment / whitespace shifts
// don't move the boundary between buckets.
fn chunk_nodes(features: &AstFeatures, size: usize) -> Vec<&[AstNodeFeature]> {
    let nodes = features.nodes.as_slice();
    let chunk = nodes.len().div_ceil(size).max(1);
    let mut out = Vec::with_capacity(size);
    for gy in 0..size {
        let start = gy * chunk;
        if start >= nodes.len() {
            out.push(&nodes[nodes.len()..nodes.len()]);
            continue;
        }
        let end = (start + chunk).min(nodes.len());
        out.push(&nodes[start..end]);
    }
    out
}
