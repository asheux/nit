//! `StructuralEncoder` — maps semantic token roles to a 32x32 Hilbert-curve
//! grid. Four channels: role diversity (35%), AST depth gradient (25%),
//! role entropy (20%), role 4-gram uniqueness (20%).

use crate::seed::encoders::ast_features::{compute_ast_features, AstFeatures};
use crate::seed::grid_types::{SeedEncoder, SeedInput, SeedValueGrid};
use crate::seed::utils::{apply_structural_noise, hilbert_index_to_xy, normalize_grid};
use crate::seed::view_modes::SeedEncoderId;

use super::aggregations::{
    role_diversity, role_entropy, role_ngram_uniqueness, token_depth_gradient, SemanticToken,
};

pub(crate) struct StructuralEncoder;

impl SeedEncoder for StructuralEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::Structural
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let order = 5u32;
        let size = 1usize << order;
        let total = size * size;
        let mut grid = SeedValueGrid::new(size, size);

        // No byte fallback — encoders only run when tree-sitter can parse.
        // Returning a uniform grid is the right "unknown" signal.
        let Some(features) = compute_ast_features(input.text, input.file_path) else {
            return grid;
        };
        let tokens = tokens_from_features(&features);
        if tokens.is_empty() {
            return grid;
        }

        let diversity = role_diversity(&tokens, total);
        let depth = token_depth_gradient(&tokens, total);
        let entropy = role_entropy(&tokens, total);
        let uniqueness = role_ngram_uniqueness(&tokens, total);

        for cell in 0..total {
            let d = diversity.get(cell).copied().unwrap_or(0.0);
            let dp = depth.get(cell).copied().unwrap_or(0.0);
            let e = entropy.get(cell).copied().unwrap_or(0.0);
            let u = uniqueness.get(cell).copied().unwrap_or(0.0);
            let value = (d * 0.35 + dp * 0.25 + e * 0.20 + u * 0.20)
                .round()
                .clamp(0.0, 255.0) as u8;
            let (x, y) = hilbert_index_to_xy(order, cell as u32);
            grid.set(x as usize, y as usize, value);
        }

        normalize_grid(&mut grid);
        // Noise key from the AST feature hash, not bytes — immune to
        // comment / identifier / whitespace changes.
        apply_structural_noise(&mut grid, size, seed_nonce, features.feature_hash, variant);
        grid
    }
}

fn tokens_from_features(features: &AstFeatures) -> Vec<SemanticToken> {
    let max_depth = features.nodes.iter().map(|n| n.depth).max().unwrap_or(0);
    let scale = if max_depth > 0 {
        255.0 / max_depth as f32
    } else {
        0.0
    };
    features
        .nodes
        .iter()
        .map(|node| SemanticToken {
            role: node.role_band.as_u8(),
            depth: (node.depth as f32 * scale).round().min(255.0) as u8,
        })
        .collect()
}
