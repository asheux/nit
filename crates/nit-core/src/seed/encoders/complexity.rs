use crate::seed::grid_types::{SeedEncoder, SeedInput, SeedValueGrid};
use crate::seed::utils::{apply_structural_noise, normalize_grid};
use crate::seed::view_modes::SeedEncoderId;

use super::ast_features::{compute_ast_features, AstFeatures, AstNodeFeature, ROLE_BAND_COUNT};

pub(crate) struct ComplexityFieldEncoder;

impl SeedEncoder for ComplexityFieldEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::ComplexityField
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let size = 32usize;
        let mut grid = SeedValueGrid::new(size, size);

        // No byte fallback. Drop `input.text.lines().count()` — comments and
        // blank lines made it directly gameable. Drop `complexity_token_entropy`
        // (highlight bytes per line) and `complexity_identifier_uniqueness`
        // (identifier *text*). Replace with chunked node-sequence metrics so
        // the projection has no positional / textual sensitivity.
        let Some(features) = compute_ast_features(input.text, input.file_path) else {
            return grid;
        };
        if features.nodes.is_empty() {
            return grid;
        }

        let chunks = chunk_nodes(&features, size);
        for (gy, window) in chunks.iter().enumerate() {
            let n = chunk_nesting(window);
            let c = chunk_cyclomatic(window);
            let e = chunk_band_entropy(window);
            let u = chunk_band_diversity(window);
            let value = (n * 0.25 + c * 0.30 + e * 0.25 + u * 0.20).clamp(0.0, 255.0) as u8;
            for gx in 0..size {
                grid.set(gx, gy, value);
            }
        }

        normalize_grid(&mut grid);
        apply_structural_noise(&mut grid, size, seed_nonce, features.feature_hash, variant);
        grid
    }
}

// Chunk the AST node sequence into `size` windows. Each grid row aggregates
// the metrics over one window. The old per-source-line bucketing was
// fragile to comment / whitespace shifts because the same node could fall
// in different buckets between files; chunking by node-index instead is
// stable as long as the AST node count and ordering are stable.
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

fn chunk_nesting(window: &[AstNodeFeature]) -> f32 {
    let max_depth = window.iter().map(|n| n.depth).max().unwrap_or(0);
    (max_depth as f32 / 15.0 * 255.0).clamp(0.0, 255.0)
}

fn chunk_cyclomatic(window: &[AstNodeFeature]) -> f32 {
    use super::ast_features::RoleBand;
    let control_count = window
        .iter()
        .filter(|n| n.role_band == RoleBand::ControlFlow)
        .count();
    // Cap at 12 — beyond that, additional control-flow nodes don't
    // proportionally increase real complexity; saturating prevents one
    // pathological row from dominating the grid.
    (control_count.min(12) as f32 / 12.0 * 255.0).clamp(0.0, 255.0)
}

fn chunk_band_entropy(window: &[AstNodeFeature]) -> f32 {
    if window.is_empty() {
        return 0.0;
    }
    let mut freq = [0u32; ROLE_BAND_COUNT];
    for node in window {
        freq[node.role_band.as_u8() as usize] += 1;
    }
    let total = window.len() as f32;
    let mut h = 0.0f32;
    for &f in &freq {
        if f > 0 {
            let p = f as f32 / total;
            h -= p * p.log2();
        }
    }
    let max_entropy = (ROLE_BAND_COUNT as f32).log2();
    (h / max_entropy * 255.0).clamp(0.0, 255.0)
}

fn chunk_band_diversity(window: &[AstNodeFeature]) -> f32 {
    if window.is_empty() {
        return 0.0;
    }
    let mut seen = [false; ROLE_BAND_COUNT];
    for node in window {
        seen[node.role_band.as_u8() as usize] = true;
    }
    let distinct = seen.iter().filter(|&&s| s).count();
    (distinct as f32 / ROLE_BAND_COUNT as f32 * 255.0).clamp(0.0, 255.0)
}
