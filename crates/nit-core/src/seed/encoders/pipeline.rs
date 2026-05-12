//! End-to-end seed encoding pipeline: routes the chosen encoder, runs the
//! grid through jitter / threshold / symmetry, and packages the result as
//! `EncodedSeed` with stats. Hot path; everything here is per-encoder, not
//! cross-encoder, so it sits one layer above the individual encoders.

use nit_utils::hashing::stable_hash_bytes;

use crate::seed::grid_types::{
    EncodedSeed, SeedBits, SeedEncoder, SeedInput, SeedStats, SeedValueGrid,
};
use crate::seed::params::SeedParams;
use crate::seed::utils::{
    apply_jitter, apply_symmetry, count_components, density_threshold, hash_seed, map_bits_to_grid,
};
use crate::seed::view_modes::SeedEncoderId;

use super::ascii::AsciiBytesEncoder;
use super::ast_features::compute_ast_features;
use super::hilbert::HilbertBitsEncoder;
use super::lifehash::Lifehash16Encoder;
use super::{AstStructureEncoder, ComplexityFieldEncoder, StructuralEncoder, TokenSpectrumEncoder};

pub fn encode_seed(
    input: &SeedInput<'_>,
    encoder: SeedEncoderId,
    params: &SeedParams,
    seed_nonce: u64,
    variant: u8,
    target_width: usize,
    target_height: usize,
) -> EncodedSeed {
    // The original byte-hash for jitter was the last source-byte leak: even
    // though encoders are AST-only, jitter keyed off raw bytes still moved
    // every cell when an agent added a comment / renamed / reflowed. Prefer
    // the canonical AST feature hash; fall back to the byte hash only when
    // tree-sitter can't parse (unknown extension / plain text).
    let input_hash = compute_ast_features(input.text, input.file_path)
        .map(|f| f.feature_hash)
        .unwrap_or_else(|| stable_hash_bytes(input.text.as_bytes()));
    let base_values = encode_with(encoder, input, seed_nonce, variant);
    let mut values = base_values.clone();
    apply_jitter(
        values.data_mut(),
        params.jitter,
        input_hash ^ seed_nonce ^ (variant as u64),
    );
    let bits_raw = threshold_to_bits(&values, params.target_density);
    let mut bits = bits_raw.clone();
    apply_symmetry(&mut bits, params.symmetry);
    let seed_hash = hash_seed(encoder, params, variant, &bits);
    let grid = map_bits_to_grid(&bits, target_width, target_height, params);
    let alive = grid.alive_count();
    let total = grid.width().saturating_mul(grid.height()).max(1);
    let density = alive as f32 / total as f32;
    let components = count_components(&grid);
    let stats = SeedStats {
        density,
        alive,
        components,
        base_width: bits.width(),
        base_height: bits.height(),
    };
    EncodedSeed {
        encoder_id: encoder,
        params: params.clone(),
        variant,
        input_hash,
        seed_hash,
        source: input.source,
        base_values: values,
        base_bits: bits,
        base_bits_raw: bits_raw,
        grid,
        stats,
    }
}

fn encode_with(
    encoder: SeedEncoderId,
    input: &SeedInput<'_>,
    seed_nonce: u64,
    variant: u8,
) -> SeedValueGrid {
    match encoder {
        SeedEncoderId::Structural => StructuralEncoder.encode(input, seed_nonce, variant),
        SeedEncoderId::AsciiBytes => AsciiBytesEncoder.encode(input, seed_nonce, variant),
        SeedEncoderId::Lifehash16 => Lifehash16Encoder.encode(input, seed_nonce, variant),
        SeedEncoderId::HilbertBits => HilbertBitsEncoder.encode(input, seed_nonce, variant),
        SeedEncoderId::TokenSpectrum => TokenSpectrumEncoder.encode(input, seed_nonce, variant),
        SeedEncoderId::AstStructure => AstStructureEncoder.encode(input, seed_nonce, variant),
        SeedEncoderId::ComplexityField => ComplexityFieldEncoder.encode(input, seed_nonce, variant),
    }
}

fn threshold_to_bits(values: &SeedValueGrid, target_density: f32) -> SeedBits {
    let threshold = density_threshold(target_density);
    let mut bits = SeedBits::new(values.width(), values.height());
    for y in 0..values.height() {
        for x in 0..values.width() {
            bits.set(x, y, values.get(x, y) >= threshold);
        }
    }
    bits
}
