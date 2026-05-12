//! Per-window aggregators consumed by `StructuralEncoder`: role diversity,
//! depth gradient, role entropy, role n-gram uniqueness. Each takes a
//! `SemanticToken` slice and returns a `Vec<f32>` of length `grid_cells`.

use crate::seed::encoders::ast_features::ROLE_BAND_COUNT;

const STRUCTURAL_ROLE_NGRAM: usize = 4;
const STRUCTURAL_ROLE_NGRAM_SEARCH: usize = 256;

pub(super) struct SemanticToken {
    pub role: u8,
    pub depth: u8,
}

fn chunked_token_map(
    tokens: &[SemanticToken],
    grid_cells: usize,
    f: impl Fn(&[SemanticToken]) -> f32,
) -> Vec<f32> {
    let chunk = tokens.len().div_ceil(grid_cells).max(1);
    let mut out = vec![0.0f32; grid_cells];
    for (cell, val) in out.iter_mut().enumerate() {
        let start = cell * chunk;
        if start >= tokens.len() {
            break;
        }
        let end = (start + chunk).min(tokens.len());
        *val = f(&tokens[start..end]);
    }
    out
}

pub(super) fn role_diversity(tokens: &[SemanticToken], grid_cells: usize) -> Vec<f32> {
    chunked_token_map(tokens, grid_cells, |chunk| {
        let mut seen = [false; ROLE_BAND_COUNT];
        for t in chunk {
            if (t.role as usize) < ROLE_BAND_COUNT {
                seen[t.role as usize] = true;
            }
        }
        let distinct = seen.iter().filter(|&&s| s).count();
        (distinct as f32 / ROLE_BAND_COUNT as f32 * 255.0).min(255.0)
    })
}

pub(super) fn token_depth_gradient(tokens: &[SemanticToken], grid_cells: usize) -> Vec<f32> {
    chunked_token_map(tokens, grid_cells, |chunk| {
        let n = chunk.len() as f32;
        let sum: f32 = chunk.iter().map(|t| t.depth as f32).sum();
        (sum / n).min(255.0)
    })
}

pub(super) fn role_entropy(tokens: &[SemanticToken], grid_cells: usize) -> Vec<f32> {
    let max_entropy = (ROLE_BAND_COUNT as f32).log2();
    chunked_token_map(tokens, grid_cells, |chunk| {
        let n = chunk.len() as f32;
        let mut freq = [0u32; ROLE_BAND_COUNT];
        for t in chunk {
            if (t.role as usize) < ROLE_BAND_COUNT {
                freq[t.role as usize] += 1;
            }
        }
        let mut h = 0.0f32;
        for &f in &freq {
            if f > 0 {
                let p = f as f32 / n;
                h -= p * p.log2();
            }
        }
        (h / max_entropy * 255.0).clamp(0.0, 255.0)
    })
}

pub(super) fn role_ngram_uniqueness(tokens: &[SemanticToken], grid_cells: usize) -> Vec<f32> {
    let ngram = STRUCTURAL_ROLE_NGRAM;
    let search = STRUCTURAL_ROLE_NGRAM_SEARCH;
    let mut per_token = vec![255.0f32; tokens.len()];
    if tokens.len() >= ngram {
        for i in ngram..tokens.len() {
            let gram: Vec<u8> = tokens[i + 1 - ngram..=i].iter().map(|t| t.role).collect();
            let lookback = search.min(i + 1 - ngram);
            for j in (i.saturating_sub(lookback + ngram)..i.saturating_sub(ngram)).rev() {
                if j + ngram <= tokens.len() {
                    let candidate: Vec<u8> = tokens[j..j + ngram].iter().map(|t| t.role).collect();
                    if candidate == gram {
                        let dist = i - j;
                        per_token[i] = (dist as f32 / search as f32 * 255.0).min(255.0);
                        break;
                    }
                }
            }
        }
    }

    let chunk = tokens.len().div_ceil(grid_cells).max(1);
    let mut out = vec![0.0f32; grid_cells];
    for (cell, val) in out.iter_mut().enumerate() {
        let start = cell * chunk;
        if start >= tokens.len() {
            break;
        }
        let end = (start + chunk).min(tokens.len());
        let n = (end - start) as f32;
        let sum: f32 = per_token[start..end].iter().sum();
        *val = (sum / n).min(255.0);
    }
    out
}
