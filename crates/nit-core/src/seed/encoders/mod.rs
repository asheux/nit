use nit_utils::hashing::stable_hash_bytes;

use super::grid_types::{EncodedSeed, SeedBits, SeedEncoder, SeedInput, SeedStats, SeedValueGrid};
use super::params::SeedParams;
use super::utils::{
    apply_jitter, apply_symmetry, count_components, density_threshold, hash_seed, map_bits_to_grid,
};
use super::view_modes::SeedEncoderId;

mod ascii;
mod ast_structure;
mod complexity;
mod hilbert;
mod lifehash;
mod structural;
mod token_spectrum;

use ascii::AsciiBytesEncoder;
use hilbert::HilbertBitsEncoder;
use lifehash::Lifehash16Encoder;

pub(crate) use ast_structure::AstStructureEncoder;
pub(crate) use complexity::ComplexityFieldEncoder;
pub(crate) use structural::StructuralEncoder;
pub(crate) use token_spectrum::TokenSpectrumEncoder;

pub fn encode_seed(
    input: &SeedInput<'_>,
    encoder: SeedEncoderId,
    params: &SeedParams,
    seed_nonce: u64,
    variant: u8,
    target_width: usize,
    target_height: usize,
) -> EncodedSeed {
    let input_hash = stable_hash_bytes(input.text.as_bytes());
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
