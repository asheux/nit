//! Single-pass chunk metrics: aggregates nesting / cognitive / entropy /
//! diversity from one walk over the node window. Cognitive complexity
//! follows the SonarSource model — each control-flow node contributes
//! `1 + cf_depth`. The sum is clamped at 36 (≈ a 6-deep, 6-branch hotspot)
//! so one pathological window can't drown the grid.

use crate::seed::encoders::ast_features::{AstNodeFeature, RoleBand, ROLE_BAND_COUNT};

pub(super) struct ChunkMetrics {
    pub nesting: f32,
    pub cognitive: f32,
    pub entropy: f32,
    pub diversity: f32,
}

pub(super) fn chunk_metrics(window: &[AstNodeFeature]) -> ChunkMetrics {
    if window.is_empty() {
        return ChunkMetrics {
            nesting: 0.0,
            cognitive: 0.0,
            entropy: 0.0,
            diversity: 0.0,
        };
    }

    let mut max_depth: u8 = 0;
    let mut cognitive: u32 = 0;
    let mut freq = [0u32; ROLE_BAND_COUNT];
    for node in window {
        if node.depth > max_depth {
            max_depth = node.depth;
        }
        if node.role_band == RoleBand::ControlFlow {
            cognitive = cognitive.saturating_add(1 + node.cf_depth as u32);
        }
        freq[node.role_band.as_u8() as usize] += 1;
    }

    let total = window.len() as f32;
    let max_entropy = (ROLE_BAND_COUNT as f32).log2();
    let mut h = 0.0f32;
    let mut distinct = 0usize;
    for &f in &freq {
        if f > 0 {
            distinct += 1;
            let p = f as f32 / total;
            h -= p * p.log2();
        }
    }

    ChunkMetrics {
        nesting: (max_depth as f32 / 15.0 * 255.0).clamp(0.0, 255.0),
        cognitive: (cognitive.min(36) as f32 / 36.0 * 255.0).clamp(0.0, 255.0),
        entropy: (h / max_entropy * 255.0).clamp(0.0, 255.0),
        diversity: (distinct as f32 / ROLE_BAND_COUNT as f32 * 255.0).clamp(0.0, 255.0),
    }
}
