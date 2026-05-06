use std::path::Path;

use serde::{Deserialize, Serialize};
use tree_sitter::{Parser, Query, QueryCursor, Tree};

use nit_gol::Grid;
use nit_utils::hashing::stable_hash_bytes;
use nit_utils::rng::SplitMix64;

use crate::config::GolSeedSource;

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedEncoderId {
    AsciiBytes,
    Lifehash16,
    HilbertBits,
    Structural,
    TokenSpectrum,
    AstStructure,
    ComplexityField,
}

impl SeedEncoderId {
    pub fn as_str(self) -> &'static str {
        match self {
            SeedEncoderId::AsciiBytes => "ascii_bytes",
            SeedEncoderId::Lifehash16 => "lifehash16",
            SeedEncoderId::HilbertBits => "hilbert_bits",
            SeedEncoderId::Structural => "structural",
            SeedEncoderId::TokenSpectrum => "token_spectrum",
            SeedEncoderId::AstStructure => "ast_structure",
            SeedEncoderId::ComplexityField => "complexity_field",
        }
    }

    pub fn label(self) -> &'static str {
        self.as_str()
    }

    pub fn from_str_name(s: &str) -> Option<Self> {
        match s {
            "ascii_bytes" => Some(SeedEncoderId::AsciiBytes),
            "lifehash16" => Some(SeedEncoderId::Lifehash16),
            "hilbert_bits" => Some(SeedEncoderId::HilbertBits),
            "structural" => Some(SeedEncoderId::Structural),
            "token_spectrum" => Some(SeedEncoderId::TokenSpectrum),
            "ast_structure" => Some(SeedEncoderId::AstStructure),
            "complexity_field" => Some(SeedEncoderId::ComplexityField),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        match self {
            SeedEncoderId::AsciiBytes => SeedEncoderId::HilbertBits,
            SeedEncoderId::HilbertBits => SeedEncoderId::Lifehash16,
            SeedEncoderId::Lifehash16 => SeedEncoderId::Structural,
            SeedEncoderId::Structural => SeedEncoderId::TokenSpectrum,
            SeedEncoderId::TokenSpectrum => SeedEncoderId::AstStructure,
            SeedEncoderId::AstStructure => SeedEncoderId::ComplexityField,
            SeedEncoderId::ComplexityField => SeedEncoderId::AsciiBytes,
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedViewMode {
    Genome,
    Plate,
    Map,
    Stats,
}

impl SeedViewMode {
    pub fn next(self) -> Self {
        match self {
            SeedViewMode::Genome => SeedViewMode::Plate,
            SeedViewMode::Plate => SeedViewMode::Map,
            SeedViewMode::Map => SeedViewMode::Stats,
            SeedViewMode::Stats => SeedViewMode::Genome,
        }
    }

    pub fn toggle_plate(self) -> Self {
        match self {
            SeedViewMode::Genome => SeedViewMode::Plate,
            SeedViewMode::Plate => SeedViewMode::Genome,
            other => other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SeedViewMode::Genome => "GENOME",
            SeedViewMode::Plate => "PLATE",
            SeedViewMode::Map => "MAP",
            SeedViewMode::Stats => "STATS",
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedPreviewMode {
    Solid,
    HalfBlock,
    Braille,
    Tissue,
    Heatmap,
}

impl SeedPreviewMode {
    pub fn next(self) -> Self {
        match self {
            SeedPreviewMode::Solid => SeedPreviewMode::HalfBlock,
            SeedPreviewMode::HalfBlock => SeedPreviewMode::Braille,
            SeedPreviewMode::Braille => SeedPreviewMode::Tissue,
            SeedPreviewMode::Tissue => SeedPreviewMode::Heatmap,
            SeedPreviewMode::Heatmap => SeedPreviewMode::Solid,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SeedPreviewMode::Solid => "SOLID",
            SeedPreviewMode::HalfBlock => "HALF",
            SeedPreviewMode::Braille => "BRAILLE",
            SeedPreviewMode::Tissue => "TISSUE",
            SeedPreviewMode::Heatmap => "HEAT",
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedSymmetry {
    None,
    MirrorX,
    MirrorY,
    Rotate180,
}

impl SeedSymmetry {
    pub fn next(self) -> Self {
        match self {
            SeedSymmetry::None => SeedSymmetry::MirrorX,
            SeedSymmetry::MirrorX => SeedSymmetry::MirrorY,
            SeedSymmetry::MirrorY => SeedSymmetry::Rotate180,
            SeedSymmetry::Rotate180 => SeedSymmetry::None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SeedSymmetry::None => "none",
            SeedSymmetry::MirrorX => "mirror-x",
            SeedSymmetry::MirrorY => "mirror-y",
            SeedSymmetry::Rotate180 => "rotate-180",
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedPlacement {
    Center,
    TopLeft,
}

impl SeedPlacement {
    pub fn label(self) -> &'static str {
        match self {
            SeedPlacement::Center => "center",
            SeedPlacement::TopLeft => "top-left",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeedParams {
    pub symmetry: SeedSymmetry,
    pub target_density: f32,
    pub padding: u8,
    pub placement: SeedPlacement,
    pub jitter: f32,
}

impl Default for SeedParams {
    fn default() -> Self {
        Self {
            symmetry: SeedSymmetry::MirrorX,
            target_density: 0.31,
            padding: 1,
            placement: SeedPlacement::Center,
            jitter: 0.04,
        }
    }
}

impl SeedParams {
    pub fn summary(&self) -> String {
        format!(
            "sym:{} dens:{:.2} pad:{} place:{} jit:{:.2}",
            self.symmetry.label(),
            self.target_density,
            self.padding,
            self.placement.label(),
            self.jitter
        )
    }

    pub fn fingerprint(&self) -> u64 {
        let mut bytes = Vec::with_capacity(16);
        bytes.push(self.symmetry as u8);
        bytes.push(self.placement as u8);
        bytes.extend_from_slice(
            &(self.target_density.clamp(0.0, 1.0) * 1_000_000.0)
                .round()
                .to_le_bytes(),
        );
        bytes.extend_from_slice(
            &(self.jitter.clamp(0.0, 1.0) * 1_000_000.0)
                .round()
                .to_le_bytes(),
        );
        bytes.push(self.padding);
        stable_hash_bytes(&bytes)
    }
}

pub struct SeedInput<'a> {
    pub text: &'a str,
    pub source: GolSeedSource,
    pub file_path: Option<&'a Path>,
    pub version: u64,
}

#[derive(Clone, Debug)]
pub struct SeedValueGrid {
    width: usize,
    height: usize,
    values: Vec<u8>,
}

impl SeedValueGrid {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            values: vec![0; width.saturating_mul(height)],
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn values(&self) -> &[u8] {
        &self.values
    }

    pub fn values_mut(&mut self) -> &mut [u8] {
        &mut self.values
    }

    pub fn get(&self, x: usize, y: usize) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.values[y * self.width + x]
    }

    pub fn set(&mut self, x: usize, y: usize, value: u8) {
        if x >= self.width || y >= self.height {
            return;
        }
        self.values[y * self.width + x] = value;
    }
}

#[derive(Clone, Debug)]
pub struct SeedBits {
    width: usize,
    height: usize,
    cells: Vec<u8>,
}

impl SeedBits {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![0; width.saturating_mul(height)],
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn get(&self, x: usize, y: usize) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.cells[y * self.width + x] != 0
    }

    pub fn set(&mut self, x: usize, y: usize, value: bool) {
        if x >= self.width || y >= self.height {
            return;
        }
        self.cells[y * self.width + x] = if value { 1 } else { 0 };
    }

    pub fn cells(&self) -> &[u8] {
        &self.cells
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SeedStats {
    pub density: f32,
    pub alive: usize,
    pub components: usize,
    pub base_width: usize,
    pub base_height: usize,
}

#[derive(Clone, Debug)]
pub struct EncodedSeed {
    pub encoder_id: SeedEncoderId,
    pub params: SeedParams,
    pub variant: u8,
    pub input_hash: u64,
    pub seed_hash: u64,
    pub source: GolSeedSource,
    pub base_values: SeedValueGrid,
    pub base_bits: SeedBits,
    pub base_bits_raw: SeedBits,
    pub grid: Grid,
    pub stats: SeedStats,
}

pub trait SeedEncoder {
    fn id(&self) -> SeedEncoderId;
    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid;
}

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
    let base_values = match encoder {
        SeedEncoderId::Structural => StructuralEncoder.encode(input, seed_nonce, variant),
        SeedEncoderId::AsciiBytes => AsciiBytesEncoder.encode(input, seed_nonce, variant),
        SeedEncoderId::Lifehash16 => Lifehash16Encoder.encode(input, seed_nonce, variant),
        SeedEncoderId::HilbertBits => HilbertBitsEncoder.encode(input, seed_nonce, variant),
        SeedEncoderId::TokenSpectrum => TokenSpectrumEncoder.encode(input, seed_nonce, variant),
        SeedEncoderId::AstStructure => AstStructureEncoder.encode(input, seed_nonce, variant),
        SeedEncoderId::ComplexityField => ComplexityFieldEncoder.encode(input, seed_nonce, variant),
    };
    let mut values = base_values.clone();
    apply_jitter(
        values.values_mut(),
        params.jitter,
        input_hash ^ seed_nonce ^ (variant as u64),
    );
    let threshold = density_threshold(params.target_density);
    let mut bits_raw = SeedBits::new(values.width(), values.height());
    for y in 0..values.height() {
        for x in 0..values.width() {
            let alive = values.get(x, y) >= threshold;
            bits_raw.set(x, y, alive);
        }
    }
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

fn density_threshold(target_density: f32) -> u8 {
    let clamped = target_density.clamp(0.0, 1.0);
    let threshold = (1.0 - clamped) * 255.0;
    threshold.round().clamp(0.0, 255.0) as u8
}

fn hash_seed(encoder: SeedEncoderId, params: &SeedParams, variant: u8, bits: &SeedBits) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(encoder.as_str().as_bytes());
    hasher.update(&params.fingerprint().to_le_bytes());
    hasher.update(&[variant]);
    hasher.update(&bits.width().to_le_bytes());
    hasher.update(&bits.height().to_le_bytes());
    hasher.update(bits.cells());
    let hash = hasher.finalize();
    u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap())
}

fn apply_jitter(values: &mut [u8], jitter: f32, seed: u64) {
    let jitter = jitter.clamp(0.0, 1.0);
    if jitter <= f32::EPSILON {
        return;
    }
    let amp = (jitter * 32.0).round() as i16;
    if amp <= 0 {
        return;
    }
    let mut rng = SplitMix64::new(seed);
    let span = (amp * 2 + 1) as u64;
    for value in values.iter_mut() {
        let delta = ((rng.next_u64() >> 48) % span) as i16 - amp;
        let next = (*value as i16 + delta).clamp(0, 255) as u8;
        *value = next;
    }
}

fn apply_symmetry(bits: &mut SeedBits, symmetry: SeedSymmetry) {
    let w = bits.width();
    let h = bits.height();
    match symmetry {
        SeedSymmetry::None => {}
        SeedSymmetry::MirrorX => {
            for y in 0..h {
                for x in 0..w / 2 {
                    let rx = w - 1 - x;
                    let alive = bits.get(x, y) || bits.get(rx, y);
                    bits.set(x, y, alive);
                    bits.set(rx, y, alive);
                }
            }
        }
        SeedSymmetry::MirrorY => {
            for y in 0..h / 2 {
                for x in 0..w {
                    let ry = h - 1 - y;
                    let alive = bits.get(x, y) || bits.get(x, ry);
                    bits.set(x, y, alive);
                    bits.set(x, ry, alive);
                }
            }
        }
        SeedSymmetry::Rotate180 => {
            for y in 0..h {
                for x in 0..w {
                    let rx = w - 1 - x;
                    let ry = h - 1 - y;
                    let alive = bits.get(x, y) || bits.get(rx, ry);
                    bits.set(x, y, alive);
                    bits.set(rx, ry, alive);
                }
            }
        }
    }
}

fn map_bits_to_grid(bits: &SeedBits, width: usize, height: usize, params: &SeedParams) -> Grid {
    let mut grid = Grid::new(width, height);
    if width == 0 || height == 0 || bits.width() == 0 || bits.height() == 0 {
        return grid;
    }
    let padding = params.padding as usize;
    let avail_w = width.saturating_sub(padding.saturating_mul(2)).max(1);
    let avail_h = height.saturating_sub(padding.saturating_mul(2)).max(1);
    let dest_w = avail_w;
    let dest_h = avail_h;
    let offset_x = match params.placement {
        SeedPlacement::Center => width.saturating_sub(dest_w) / 2,
        SeedPlacement::TopLeft => padding.min(width.saturating_sub(1)),
    };
    let offset_y = match params.placement {
        SeedPlacement::Center => height.saturating_sub(dest_h) / 2,
        SeedPlacement::TopLeft => padding.min(height.saturating_sub(1)),
    };
    for dy in 0..dest_h {
        let by = dy.saturating_mul(bits.height()) / dest_h.max(1);
        for dx in 0..dest_w {
            let bx = dx.saturating_mul(bits.width()) / dest_w.max(1);
            if bits.get(bx, by) {
                let x = offset_x.saturating_add(dx);
                let y = offset_y.saturating_add(dy);
                if x < width && y < height {
                    grid.set(x, y, true);
                }
            }
        }
    }
    grid
}

/// Counts connected components using 8-connectivity (Moore neighborhood).
/// Diagonally adjacent alive cells are considered part of the same component.
fn count_components(grid: &Grid) -> usize {
    let w = grid.width();
    let h = grid.height();
    if w == 0 || h == 0 {
        return 0;
    }
    let mut visited = vec![false; w.saturating_mul(h)];
    let mut components = 0usize;
    let mut stack = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if visited[idx] || !grid.get(x, y) {
                continue;
            }
            components += 1;
            visited[idx] = true;
            stack.push((x, y));
            while let Some((cx, cy)) = stack.pop() {
                for ny in cy.saturating_sub(1)..=(cy + 1).min(h - 1) {
                    for nx in cx.saturating_sub(1)..=(cx + 1).min(w - 1) {
                        let nidx = ny * w + nx;
                        if !visited[nidx] && grid.get(nx, ny) {
                            visited[nidx] = true;
                            stack.push((nx, ny));
                        }
                    }
                }
            }
        }
    }
    components
}

struct AsciiBytesEncoder;

impl SeedEncoder for AsciiBytesEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::AsciiBytes
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let size = 32usize;
        let mut grid = SeedValueGrid::new(size, size);
        let bytes = input.text.as_bytes();
        let mut rng = SplitMix64::new(seed_nonce ^ stable_hash_bytes(bytes) ^ (variant as u64));
        let len = bytes.len();
        for idx in 0..size * size {
            let base = if len == 0 { 0 } else { bytes[idx % len] };
            let mix = (rng.next_u64() & 0xff) as u8;
            let value = base.wrapping_add((idx as u8).wrapping_mul(31)) ^ mix;
            let x = idx % size;
            let y = idx / size;
            grid.set(x, y, value);
        }
        grid
    }
}

struct Lifehash16Encoder;

impl SeedEncoder for Lifehash16Encoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::Lifehash16
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let size = 16usize;
        let mut grid = SeedValueGrid::new(size, size);
        let bytes = input.text.as_bytes();
        let mut rng =
            SplitMix64::new(seed_nonce ^ stable_hash_bytes(bytes) ^ (variant as u64) ^ 0x16_u64);
        for idx in 0..size * size {
            let value = (rng.next_u64() & 0xff) as u8;
            let x = idx % size;
            let y = idx / size;
            grid.set(x, y, value);
        }
        grid
    }
}

struct HilbertBitsEncoder;

impl SeedEncoder for HilbertBitsEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::HilbertBits
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let order = 5u32;
        let size = 1usize << order;
        let mut grid = SeedValueGrid::new(size, size);
        let bytes = input.text.as_bytes();
        let len = bytes.len();
        let mut rng =
            SplitMix64::new(seed_nonce ^ stable_hash_bytes(bytes) ^ (variant as u64) ^ 0x5eed_u64);
        for idx in 0..size * size {
            let (x, y) = hilbert_index_to_xy(order, idx as u32);
            let base = if len == 0 { 0 } else { bytes[idx % len] };
            let mix = (rng.next_u64() & 0xff) as u8;
            let value = base ^ mix;
            grid.set(x as usize, y as usize, value);
        }
        grid
    }
}

// ---------------------------------------------------------------------------
// Structural encoder — maps semantic token-role features to GoL genomes.
//
// Operates on a **filtered token-role sequence** from tree-sitter, stripping
// whitespace entirely. Four channels are computed on the semantic tokens:
//   1. Role diversity  (35%) — count of distinct token roles per chunk.
//   2. AST depth       (25%) — nesting depth from tree-sitter, not brackets.
//   3. Role entropy    (20%) — Shannon entropy of role distribution per window.
//   4. Role n-gram     (20%) — uniqueness of role 4-grams.
//
// Tokens are mapped to a 32×32 grid via Hilbert curve. The result: code with
// varied structure, mixed role types, and unique patterns produces rich GoL
// genomes. Uniform/repetitive code produces flat grids that die quickly.
// ---------------------------------------------------------------------------

/// Compact role ID for semantic tokens (whitespace excluded).
const ROLE_COMMENT: u8 = 0;
const ROLE_PUNCTUATION: u8 = 1;
const ROLE_OPERATOR: u8 = 2;
const ROLE_KEYWORD: u8 = 3;
const ROLE_VARIABLE: u8 = 4;
const ROLE_TYPE: u8 = 5;
const ROLE_STRING: u8 = 6;
const ROLE_FUNCTION: u8 = 7;
const ROLE_MACRO: u8 = 8;
const ROLE_COUNT: usize = 9;

/// N-gram size and search distance for role-based uniqueness analysis.
const STRUCTURAL_ROLE_NGRAM: usize = 4;
const STRUCTURAL_ROLE_NGRAM_SEARCH: usize = 256;

/// Map a SeedHighlight to a compact role ID.
fn highlight_to_role(h: SeedHighlight) -> u8 {
    match h {
        SeedHighlight::Comment => ROLE_COMMENT,
        SeedHighlight::Punctuation => ROLE_PUNCTUATION,
        SeedHighlight::Operator => ROLE_OPERATOR,
        SeedHighlight::Keyword => ROLE_KEYWORD,
        SeedHighlight::Variable => ROLE_VARIABLE,
        SeedHighlight::Type => ROLE_TYPE,
        SeedHighlight::StringLiteral => ROLE_STRING,
        SeedHighlight::Function => ROLE_FUNCTION,
        SeedHighlight::Macro => ROLE_MACRO,
    }
}

/// A semantic token: role + AST depth at that position.
struct SemanticToken {
    role: u8,
    depth: u8, // 0-255, normalized
}

/// Extract a whitespace-free sequence of semantic tokens with AST depth.
fn extract_semantic_tokens(text: &str, tree: &Tree, lang: SeedLanguage) -> Vec<SemanticToken> {
    let groups = seed_highlight_bytes(text, lang, tree);

    // Compute per-byte AST depth.
    let byte_depths = ast_depth_per_byte(tree, text.len());

    // Build token sequence: one entry per non-whitespace "token run".
    // A token run is a contiguous sequence of bytes with the same highlight group.
    let mut tokens = Vec::with_capacity(text.len() / 4);
    let mut i = 0;
    while i < groups.len() {
        let group = match groups[i] {
            Some(g) => g,
            None => {
                i += 1;
                continue; // skip whitespace
            }
        };
        // Find end of this token run (same highlight group).
        let start = i;
        while i < groups.len() && groups[i] == Some(group) {
            i += 1;
        }
        // Use max depth within this token's span.
        let max_d = byte_depths[start..i].iter().copied().max().unwrap_or(0);
        tokens.push(SemanticToken {
            role: highlight_to_role(group),
            depth: max_d,
        });
    }
    tokens
}

/// Compute per-byte AST depth (0-255 normalized).
fn ast_depth_per_byte(tree: &Tree, byte_count: usize) -> Vec<u8> {
    let mut depths = vec![0u32; byte_count];
    let mut max_depth = 0u32;
    let mut stack = vec![(tree.root_node(), 0u32)];
    while let Some((node, depth)) = stack.pop() {
        let start = node.start_byte().min(byte_count);
        let end = node.end_byte().min(byte_count);
        for d in depths[start..end].iter_mut() {
            *d = (*d).max(depth);
        }
        max_depth = max_depth.max(depth);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push((child, depth + 1));
        }
    }
    let scale = if max_depth > 0 {
        255.0 / max_depth as f32
    } else {
        0.0
    };
    depths
        .iter()
        .map(|&d| (d as f32 * scale).round().min(255.0) as u8)
        .collect()
}

struct StructuralEncoder;

impl SeedEncoder for StructuralEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::Structural
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let order = 5u32;
        let size = 1usize << order; // 32
        let total = size * size; // 1024
        let mut grid = SeedValueGrid::new(size, size);
        let bytes = input.text.as_bytes();

        if bytes.is_empty() {
            return grid;
        }

        // Try AST-aware path; fall back to filtered-byte path.
        let tokens = match seed_parse(input.text, input.file_path) {
            Some((tree, lang)) => extract_semantic_tokens(input.text, &tree, lang),
            None => extract_byte_tokens(bytes),
        };

        if tokens.is_empty() {
            return grid;
        }

        // ---- Channel 1: Role diversity (35%) ----
        let diversity = role_diversity(&tokens, total);

        // ---- Channel 2: AST depth gradient (25%) ----
        let depth = token_depth_gradient(&tokens, total);

        // ---- Channel 3: Role entropy (20%) ----
        let entropy = role_entropy(&tokens, total);

        // ---- Channel 4: Role n-gram uniqueness (20%) ----
        let uniqueness = role_ngram_uniqueness(&tokens, total);

        // ---- Map to grid via Hilbert curve ----
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
        apply_structural_noise(&mut grid, size, seed_nonce, bytes, variant);
        grid
    }
}

/// Fallback: extract tokens from raw bytes, filtering whitespace.
fn extract_byte_tokens(bytes: &[u8]) -> Vec<SemanticToken> {
    let mut tokens = Vec::with_capacity(bytes.len() / 2);
    let mut depth: u8 = 0;
    let mut max_depth: u8 = 0;
    for &b in bytes {
        match b {
            b'\n' | b'\r' | b'\t' | b' ' => continue, // skip whitespace
            b'(' | b'{' | b'[' => {
                depth = depth.saturating_add(1);
                max_depth = max_depth.max(depth);
            }
            b')' | b'}' | b']' => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        let role = match b {
            b'a'..=b'z' | b'_' => ROLE_VARIABLE,
            b'A'..=b'Z' => ROLE_TYPE,
            b'0'..=b'9' => ROLE_STRING,
            b'(' | b')' | b'{' | b'}' | b'[' | b']' | b';' | b':' | b',' | b'.' => ROLE_PUNCTUATION,
            b'+' | b'-' | b'*' | b'/' | b'=' | b'<' | b'>' | b'!' | b'&' | b'|' => ROLE_OPERATOR,
            b'"' | b'\'' | b'`' => ROLE_STRING,
            _ => ROLE_PUNCTUATION,
        };
        tokens.push(SemanticToken { role, depth });
    }
    // Normalize depth to 0-255 after the fact.
    if max_depth > 0 {
        let scale = 255.0 / max_depth as f32;
        for t in &mut tokens {
            t.depth = (t.depth as f32 * scale).round().min(255.0) as u8;
        }
    }
    tokens
}

/// Chunk tokens into `grid_cells` buckets and apply `f` to each chunk.
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

/// Channel 1: Count of distinct roles per chunk, normalized to 0-255.
fn role_diversity(tokens: &[SemanticToken], grid_cells: usize) -> Vec<f32> {
    chunked_token_map(tokens, grid_cells, |chunk| {
        let mut seen = [false; ROLE_COUNT];
        for t in chunk {
            if (t.role as usize) < ROLE_COUNT {
                seen[t.role as usize] = true;
            }
        }
        let distinct = seen.iter().filter(|&&s| s).count();
        (distinct as f32 / ROLE_COUNT as f32 * 255.0).min(255.0)
    })
}

/// Channel 2: Average AST depth per chunk, directly from token depth values.
fn token_depth_gradient(tokens: &[SemanticToken], grid_cells: usize) -> Vec<f32> {
    chunked_token_map(tokens, grid_cells, |chunk| {
        let n = chunk.len() as f32;
        let sum: f32 = chunk.iter().map(|t| t.depth as f32).sum();
        (sum / n).min(255.0)
    })
}

/// Channel 3: Shannon entropy of role distribution per chunk, normalized to 0-255.
fn role_entropy(tokens: &[SemanticToken], grid_cells: usize) -> Vec<f32> {
    let max_entropy = (ROLE_COUNT as f32).log2(); // ~3.17 bits
    chunked_token_map(tokens, grid_cells, |chunk| {
        let n = chunk.len() as f32;
        let mut freq = [0u32; ROLE_COUNT];
        for t in chunk {
            if (t.role as usize) < ROLE_COUNT {
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

/// Channel 4: Role n-gram uniqueness per chunk, normalized to 0-255.
fn role_ngram_uniqueness(tokens: &[SemanticToken], grid_cells: usize) -> Vec<f32> {
    let ngram = STRUCTURAL_ROLE_NGRAM;
    let search = STRUCTURAL_ROLE_NGRAM_SEARCH;
    // Pre-compute per-token uniqueness.
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

    // Average per chunk.
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

// ---------------------------------------------------------------------------
// Tree-sitter helpers for AST-driven seed encoders.
// ---------------------------------------------------------------------------

/// Lightweight language ID for seed encoding (avoids nit-syntax dependency cycle).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SeedLanguage {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Markdown,
    Html,
    Css,
    Json,
    Toml,
    Yaml,
    Bash,
}

impl SeedLanguage {
    fn detect(file_path: Option<&Path>) -> Option<Self> {
        let path = file_path?;
        let ext = path.extension()?.to_str()?;
        match ext.to_lowercase().as_str() {
            "rs" => Some(Self::Rust),
            "py" => Some(Self::Python),
            "js" | "mjs" | "cjs" | "jsx" => Some(Self::JavaScript),
            "ts" | "tsx" => Some(Self::TypeScript),
            "md" | "markdown" => Some(Self::Markdown),
            "html" | "htm" => Some(Self::Html),
            "css" | "scss" | "sass" => Some(Self::Css),
            "json" | "jsonc" => Some(Self::Json),
            "toml" => Some(Self::Toml),
            "yml" | "yaml" => Some(Self::Yaml),
            "sh" | "bash" | "zsh" | "fish" => Some(Self::Bash),
            _ => None,
        }
    }

    fn ts_language(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::language(),
            Self::Python => tree_sitter_python::language(),
            Self::JavaScript => tree_sitter_javascript::language(),
            Self::TypeScript => tree_sitter_typescript::language_typescript(),
            Self::Markdown => tree_sitter_markdown_fork::language(),
            Self::Html => tree_sitter_html::language(),
            Self::Css => tree_sitter_css::language(),
            Self::Json => tree_sitter_json::language(),
            Self::Toml => tree_sitter_toml::language(),
            Self::Yaml => tree_sitter_yaml::language(),
            Self::Bash => tree_sitter_bash::language(),
        }
    }

    fn highlights_query(self) -> &'static str {
        match self {
            Self::Rust => include_str!("../../nit-syntax/queries/rust/highlights.scm"),
            Self::Python => tree_sitter_python::HIGHLIGHT_QUERY,
            Self::JavaScript => tree_sitter_javascript::HIGHLIGHT_QUERY,
            Self::TypeScript => tree_sitter_typescript::HIGHLIGHT_QUERY,
            Self::Markdown => include_str!("../../nit-syntax/queries/markdown/highlights.scm"),
            Self::Html => tree_sitter_html::HIGHLIGHT_QUERY,
            Self::Css => tree_sitter_css::HIGHLIGHTS_QUERY,
            Self::Json => tree_sitter_json::HIGHLIGHT_QUERY,
            Self::Toml => tree_sitter_toml::HIGHLIGHT_QUERY,
            Self::Yaml => include_str!("../../nit-syntax/queries/yaml/highlights.scm"),
            Self::Bash => tree_sitter_bash::HIGHLIGHT_QUERY,
        }
    }
}

/// Parse source text with tree-sitter for seed encoding.
fn seed_parse(text: &str, file_path: Option<&Path>) -> Option<(Tree, SeedLanguage)> {
    let lang = SeedLanguage::detect(file_path)?;
    let mut parser = Parser::new();
    parser.set_language(lang.ts_language()).ok()?;
    let tree = parser.parse(text, None)?;
    Some((tree, lang))
}

/// Highlight group categories used by seed encoders.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum SeedHighlight {
    Comment,
    Punctuation,
    Operator,
    Keyword,
    Variable,
    Type,
    StringLiteral,
    Function,
    Macro,
}

/// Classify each byte by its highlight group using tree-sitter queries.
fn seed_highlight_bytes(text: &str, lang: SeedLanguage, tree: &Tree) -> Vec<Option<SeedHighlight>> {
    let mut result = vec![None; text.len()];
    let ts_lang = lang.ts_language();
    let query_src = lang.highlights_query();
    let query = match Query::new(ts_lang, query_src) {
        Ok(q) => q,
        Err(_) => return result,
    };

    let groups = map_seed_capture_groups(&query);
    let mut cursor = QueryCursor::new();
    let root = tree.root_node();
    let source = text.as_bytes();

    let mut spans: Vec<(usize, usize, SeedHighlight, usize)> = Vec::new();
    for m in cursor.matches(&query, root, source) {
        for capture in m.captures {
            if let Some(group) = groups.get(capture.index as usize).and_then(|g| *g) {
                let start = capture.node.start_byte();
                let end = capture.node.end_byte();
                if end > start && end <= text.len() {
                    spans.push((start, end, group, m.pattern_index));
                }
            }
        }
    }

    spans.sort_by_key(|s| s.3);

    for (start, end, group, _) in spans {
        for byte in &mut result[start..end] {
            *byte = Some(group);
        }
    }

    result
}

fn map_seed_capture_groups(query: &Query) -> Vec<Option<SeedHighlight>> {
    query
        .capture_names()
        .iter()
        .map(|name| {
            let name = name.as_str();
            let base = name.split('.').next().unwrap_or(name);
            match name {
                "comment" | "comment.documentation" => Some(SeedHighlight::Comment),
                "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => {
                    Some(SeedHighlight::Punctuation)
                }
                "operator" | "keyword.operator" => Some(SeedHighlight::Operator),
                "keyword" | "keyword.control" | "label" => Some(SeedHighlight::Keyword),
                "variable" | "variable.builtin" | "variable.parameter" | "parameter"
                | "property" => Some(SeedHighlight::Variable),
                "type" | "type.builtin" | "constructor" | "namespace" => Some(SeedHighlight::Type),
                "string" | "string.special" | "character" | "number" | "boolean"
                | "constant.builtin" | "escape" => Some(SeedHighlight::StringLiteral),
                "function" | "function.macro" | "function.method" | "method" => {
                    Some(SeedHighlight::Function)
                }
                "macro" | "attribute" | "constant" => Some(SeedHighlight::Macro),
                _ => match base {
                    "comment" => Some(SeedHighlight::Comment),
                    "string" => Some(SeedHighlight::StringLiteral),
                    "keyword" => Some(SeedHighlight::Keyword),
                    "type" => Some(SeedHighlight::Type),
                    "function" => Some(SeedHighlight::Function),
                    "variable" => Some(SeedHighlight::Variable),
                    "punctuation" => Some(SeedHighlight::Punctuation),
                    "constant" => Some(SeedHighlight::Macro),
                    _ => None,
                },
            }
        })
        .collect()
}

/// Classify an AST node kind into a semantic class (0-255).
/// Deterministic and meaningful, unlike a hash of the kind_id.
fn ast_node_class(kind: &str) -> u8 {
    // Declarations: highest weight — they define code structure.
    if kind.contains("declaration")
        || kind.contains("definition")
        || kind.contains("function_item")
        || kind.contains("struct_item")
        || kind.contains("enum_item")
        || kind.contains("trait_item")
        || kind.contains("impl_item")
        || kind.contains("class_")
        || kind.contains("interface_")
        || kind == "module"
    {
        return 255;
    }
    // Control flow: creates branching structure.
    if kind.contains("if_")
        || kind.contains("match_")
        || kind.contains("switch_")
        || kind.contains("while_")
        || kind.contains("for_")
        || kind.contains("loop_")
        || kind.contains("try_")
        || kind.contains("catch_")
    {
        return 210;
    }
    // Expressions: moderate structural weight.
    if kind.contains("expression")
        || kind.contains("call_")
        || kind.contains("binary_")
        || kind.contains("unary_")
        || kind.contains("assignment")
    {
        return 170;
    }
    // Statements / blocks.
    if kind.contains("statement")
        || kind.contains("block")
        || kind == "source_file"
        || kind == "program"
    {
        return 130;
    }
    // Type annotations / parameters.
    if kind.contains("type") || kind.contains("parameter") || kind.contains("argument") {
        return 90;
    }
    // Literals and identifiers.
    if kind.contains("literal")
        || kind.contains("string")
        || kind.contains("number")
        || kind == "identifier"
    {
        return 50;
    }
    // Everything else.
    100
}

// ---------------------------------------------------------------------------
// TokenSpectrum encoder — AST-driven token classification via tree-sitter.
// ---------------------------------------------------------------------------

struct TokenSpectrumEncoder;

impl SeedEncoder for TokenSpectrumEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::TokenSpectrum
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let order = 5u32;
        let size = 1usize << order; // 32
        let total = size * size; // 1024
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
                    .filter(|g| g.is_some()) // skip whitespace (None)
                    .map(|g| seed_highlight_to_value(*g))
                    .collect()
            }
            None => {
                // Fallback: byte-category classification, skipping whitespace.
                bytes
                    .iter()
                    .filter(|&&b| !matches!(b, b'\n' | b'\r' | b'\t' | b' '))
                    .map(|&b| byte_category_value(b))
                    .collect()
            }
        };

        if values.is_empty() {
            return grid;
        }

        // Map to 32x32 grid via Hilbert curve, averaging per cell.
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

/// Map a SeedHighlight to the specified value range for TokenSpectrum.
fn seed_highlight_to_value(group: Option<SeedHighlight>) -> u8 {
    match group {
        None => 10, // Whitespace/uncovered → 0-20
        Some(g) => match g {
            SeedHighlight::Comment => 35,        // 21-50
            SeedHighlight::Punctuation => 65,    // 51-80
            SeedHighlight::Operator => 95,       // 81-110
            SeedHighlight::Keyword => 125,       // 111-140
            SeedHighlight::Variable => 153,      // 141-165
            SeedHighlight::Type => 178,          // 166-190
            SeedHighlight::StringLiteral => 203, // 191-215
            SeedHighlight::Function => 228,      // 216-240
            SeedHighlight::Macro => 248,         // 241-255
        },
    }
}

/// Byte-category fallback for TokenSpectrum when tree-sitter is unavailable.
fn byte_category_value(b: u8) -> u8 {
    match b {
        b'\n' | b'\r' | b'\t' | b' ' => 10, // whitespace
        b'/' => 35,                         // likely comment
        b'(' | b')' | b'{' | b'}' | b'[' | b']' | b';' | b':' | b',' | b'.' => 65, // punctuation
        b'+' | b'-' | b'*' | b'=' | b'<' | b'>' | b'!' | b'&' | b'|' | b'^' | b'%' => 95, // operator
        b'"' | b'\'' | b'`' => 203, // string delimiters
        b'0'..=b'9' => 203,         // number
        b'A'..=b'Z' => 178,         // likely type
        b'a'..=b'z' | b'_' => 153,  // identifier
        _ => 65,
    }
}

// ---------------------------------------------------------------------------
// AstStructure encoder — structural properties from AST traversal.
// ---------------------------------------------------------------------------

struct AstStructureEncoder;

impl SeedEncoder for AstStructureEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::AstStructure
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let order = 5u32;
        let size = 1usize << order; // 32
        let total = size * size; // 1024
        let mut grid = SeedValueGrid::new(size, size);
        let bytes = input.text.as_bytes();

        if bytes.is_empty() {
            return grid;
        }

        let parsed = seed_parse(input.text, input.file_path);

        match parsed {
            Some((tree, _lang)) => {
                let file_size = bytes.len().max(1) as f32;

                // Collect node data via DFS traversal.
                let mut node_fills: Vec<(u8, u32)> = Vec::new(); // (value, byte_span)
                let mut stack = vec![(tree.root_node(), 0u32, 0u32)]; // (node, depth, sibling_idx)

                while let Some((node, depth, _sibling_index)) = stack.pop() {
                    if node.is_named() {
                        let child_count = node.named_child_count() as u32;
                        let byte_span = (node.end_byte().saturating_sub(node.start_byte())) as u32;

                        let depth_score = (depth.min(15) as f32 / 15.0 * 255.0) as u64;
                        let branch_score = (child_count.min(20) as f32 / 20.0 * 255.0) as u64;
                        let span_score = (byte_span.min(2000) as f32 / 2000.0 * 255.0) as u64;
                        // Semantic node class instead of pseudo-random kind_id hash.
                        let kind_class = ast_node_class(node.kind()) as u64;

                        let value = ((depth_score * 30
                            + branch_score * 25
                            + span_score * 25
                            + kind_class * 20)
                            / 100)
                            .clamp(0, 255) as u8;

                        node_fills.push((value, byte_span));
                    }

                    // Push children in reverse order so leftmost is processed first.
                    let child_count = node.child_count();
                    let mut named_idx = 0u32;
                    for i in (0..child_count).rev() {
                        if let Some(child) = node.child(i) {
                            stack.push((child, depth + 1, named_idx));
                            if child.is_named() {
                                named_idx += 1;
                            }
                        }
                    }
                }

                // Map nodes to grid cells proportional to byte_span.
                let total_span: u32 = node_fills.iter().map(|(_, s)| *s).sum();
                let total_span = total_span.max(1) as f32;
                let mut cell_idx = 0usize;

                for (value, byte_span) in &node_fills {
                    let cells = ((*byte_span as f32 / total_span) * total as f32)
                        .round()
                        .max(1.0) as usize;
                    for _ in 0..cells {
                        if cell_idx >= total {
                            break;
                        }
                        let (x, y) = hilbert_index_to_xy(order, cell_idx as u32);
                        grid.set(x as usize, y as usize, *value);
                        cell_idx += 1;
                    }
                }

                // Fill remaining cells with the last value or file-size-based default.
                let fill = node_fills.last().map(|(v, _)| *v).unwrap_or(128);
                while cell_idx < total {
                    let (x, y) = hilbert_index_to_xy(order, cell_idx as u32);
                    grid.set(x as usize, y as usize, fill);
                    cell_idx += 1;
                }

                let _ = file_size; // used implicitly via total_span
            }
            None => {
                // Fallback: neutral mid-range grid. Without tree-sitter we
                // cannot extract meaningful AST structure, so we produce a
                // deterministic but moderate signal rather than byte-level noise.
                for cell in 0..total {
                    let (x, y) = hilbert_index_to_xy(order, cell as u32);
                    grid.set(x as usize, y as usize, 128);
                }
            }
        }

        normalize_grid(&mut grid);
        apply_structural_noise(&mut grid, size, seed_nonce, bytes, variant);
        grid
    }
}

// ---------------------------------------------------------------------------
// ComplexityField encoder — per-line software metrics heatmap.
// ---------------------------------------------------------------------------

struct ComplexityFieldEncoder;

impl SeedEncoder for ComplexityFieldEncoder {
    fn id(&self) -> SeedEncoderId {
        SeedEncoderId::ComplexityField
    }

    fn encode(&self, input: &SeedInput, seed_nonce: u64, variant: u8) -> SeedValueGrid {
        let size = 32usize;
        let mut grid = SeedValueGrid::new(size, size);
        let bytes = input.text.as_bytes();

        if bytes.is_empty() {
            return grid;
        }

        let line_count = input.text.lines().count().max(1);
        let parsed = seed_parse(input.text, input.file_path);

        // Compute the four metric layers.
        let (nesting, complexity, entropy, uniqueness) = match &parsed {
            Some((tree, lang)) => {
                let n = complexity_nesting_depth(tree, line_count);
                let c = complexity_cyclomatic(tree, line_count);
                let e = complexity_token_entropy(input.text, *lang, tree, line_count);
                let u = complexity_identifier_uniqueness(tree, input.text, line_count);
                (n, c, e, u)
            }
            None => {
                // Fallback: neutral mid-range values for all layers.
                // Without tree-sitter we cannot compute meaningful metrics,
                // so we produce a moderate signal rather than byte-level noise.
                let neutral = vec![128.0f32; line_count];
                (neutral.clone(), neutral.clone(), neutral.clone(), neutral)
            }
        };

        // Combine layers into 32x32 grid. X = column position scaled, Y = line scaled.
        for gy in 0..size {
            let line = gy * line_count / size;
            let line = line.min(line_count.saturating_sub(1));
            let n = nesting.get(line).copied().unwrap_or(0.0);
            let c = complexity.get(line).copied().unwrap_or(0.0);
            let e = entropy.get(line).copied().unwrap_or(0.0);
            let u = uniqueness.get(line).copied().unwrap_or(0.0);

            let value = (n * 0.25 + c * 0.30 + e * 0.25 + u * 0.20).clamp(0.0, 255.0) as u8;

            for gx in 0..size {
                grid.set(gx, gy, value);
            }
        }

        // Add X-axis variation from column-level features if we have per-byte data.
        if let Some((tree, lang)) = &parsed {
            let groups = seed_highlight_bytes(input.text, *lang, tree);
            let line_starts = compute_line_starts(input.text);
            for gy in 0..size {
                let line = gy * line_count / size;
                let line = line.min(line_count.saturating_sub(1));
                let start = line_starts.get(line).copied().unwrap_or(0);
                let end = line_starts
                    .get(line + 1)
                    .copied()
                    .unwrap_or(input.text.len());
                let line_len = end.saturating_sub(start).max(1);
                for gx in 0..size {
                    let col = gx * line_len / size;
                    let byte_idx = start + col.min(line_len.saturating_sub(1));
                    if byte_idx < groups.len() {
                        // Skip whitespace bytes — only blend meaningful tokens.
                        if let Some(group) = groups[byte_idx] {
                            let col_value = seed_highlight_to_value(Some(group));
                            let base = grid.get(gx, gy) as u16;
                            // Blend: 80% metrics, 20% column token.
                            let blended =
                                ((base * 80 + col_value as u16 * 20) / 100).min(255) as u8;
                            grid.set(gx, gy, blended);
                        }
                    }
                }
            }
        }

        normalize_grid(&mut grid);
        apply_structural_noise(&mut grid, size, seed_nonce, bytes, variant);
        grid
    }
}

/// AST-based nesting depth per line, normalized to 0.0-255.0.
fn complexity_nesting_depth(tree: &tree_sitter::Tree, line_count: usize) -> Vec<f32> {
    let mut per_line = vec![0u32; line_count];
    let mut stack = vec![(tree.root_node(), 0u32)];

    while let Some((node, depth)) = stack.pop() {
        let start_line = node.start_position().row;
        let end_line = node.end_position().row;
        for line in start_line..=end_line.min(line_count.saturating_sub(1)) {
            if line < per_line.len() {
                per_line[line] = per_line[line].max(depth);
            }
        }
        for i in (0..node.child_count()).rev() {
            if let Some(child) = node.child(i) {
                stack.push((child, depth + 1));
            }
        }
    }

    let max_depth = per_line.iter().copied().max().unwrap_or(0).max(1) as f32;
    per_line
        .iter()
        .map(|&d| (d as f32 / max_depth * 255.0).clamp(0.0, 255.0))
        .collect()
}

/// Cyclomatic complexity per function, spread across lines.
fn complexity_cyclomatic(tree: &tree_sitter::Tree, line_count: usize) -> Vec<f32> {
    let mut per_line = vec![0.0f32; line_count];
    let root = tree.root_node();
    let mut func_stack = vec![root];
    let mut max_complexity = 0u32;

    struct FuncInfo {
        start_line: usize,
        end_line: usize,
        complexity: u32,
    }

    let mut functions: Vec<FuncInfo> = Vec::new();

    // Find function nodes at the top level and nested.
    while let Some(node) = func_stack.pop() {
        let kind = node.kind();
        let is_func = matches!(
            kind,
            "function_item"
                | "function_definition"
                | "method_definition"
                | "function_declaration"
                | "arrow_function"
                | "closure_expression"
                | "lambda"
                | "decorated_definition"
        );

        if is_func {
            let complexity = count_decision_points(node);
            max_complexity = max_complexity.max(complexity);
            functions.push(FuncInfo {
                start_line: node.start_position().row,
                end_line: node.end_position().row,
                complexity,
            });
        }

        for i in (0..node.child_count()).rev() {
            if let Some(child) = node.child(i) {
                func_stack.push(child);
            }
        }
    }

    let max_complexity = max_complexity.max(1) as f32;
    for func in &functions {
        let score = (func.complexity as f32 / max_complexity * 255.0).clamp(0.0, 255.0);
        for line in func.start_line..=func.end_line.min(line_count.saturating_sub(1)) {
            if line < per_line.len() {
                per_line[line] = per_line[line].max(score);
            }
        }
    }

    per_line
}

/// Count decision points within a node's subtree.
fn count_decision_points(node: tree_sitter::Node) -> u32 {
    let mut count = 1u32; // Base complexity = 1
    let mut stack = vec![node];

    while let Some(n) = stack.pop() {
        let kind = n.kind();
        let is_decision = matches!(
            kind,
            "if_expression"
                | "if_statement"
                | "elif_clause"
                | "match_expression"
                | "match_arm"
                | "while_expression"
                | "while_statement"
                | "for_expression"
                | "for_statement"
                | "for_in_statement"
                | "loop_expression"
                | "switch_case"
                | "ternary_expression"
                | "conditional_expression"
                | "except_clause"
                | "catch_clause"
        );
        if is_decision {
            count += 1;
        }

        // Check for logical operators (&&, ||, ??) in binary expressions.
        if kind == "binary_expression" || kind == "boolean_operator" {
            if let Some(op) = n.child_by_field_name("operator") {
                let op_kind = op.kind();
                if op_kind == "&&" || op_kind == "||" || op_kind == "??" {
                    count += 1;
                }
            }
        }

        for i in 0..n.child_count() {
            if let Some(child) = n.child(i) {
                stack.push(child);
            }
        }
    }

    count
}

/// Token entropy per line using highlight groups.
fn complexity_token_entropy(
    text: &str,
    language: SeedLanguage,
    tree: &Tree,
    line_count: usize,
) -> Vec<f32> {
    let groups = seed_highlight_bytes(text, language, tree);
    let line_starts = compute_line_starts(text);
    let num_categories = 12.0f32; // approximate number of distinct highlight categories
    let max_entropy = num_categories.log2();
    let max_entropy = if max_entropy > 0.0 { max_entropy } else { 1.0 };

    let mut per_line = vec![0.0f32; line_count];

    for (line, per_line_val) in per_line.iter_mut().enumerate() {
        let start = line_starts.get(line).copied().unwrap_or(0);
        let end = line_starts.get(line + 1).copied().unwrap_or(text.len());
        if start >= end {
            continue;
        }

        // Count highlight group frequencies on this line.
        let mut freq = [0u32; 16]; // enough for all SeedHighlight variants + None
        let mut total = 0u32;
        for g in &groups[start..end.min(groups.len())] {
            let cat = match g {
                None => 0,
                Some(g) => (*g as u32 % 15) + 1,
            };
            freq[cat as usize] += 1;
            total += 1;
        }

        if total == 0 {
            continue;
        }

        let total_f = total as f32;
        let mut h = 0.0f32;
        for &f in &freq {
            if f > 0 {
                let p = f as f32 / total_f;
                h -= p * p.log2();
            }
        }

        *per_line_val = (h / max_entropy * 255.0).clamp(0.0, 255.0);
    }

    per_line
}

/// Identifier uniqueness per scope (function body).
fn complexity_identifier_uniqueness(
    tree: &tree_sitter::Tree,
    text: &str,
    line_count: usize,
) -> Vec<f32> {
    let mut per_line = vec![0.0f32; line_count];
    let source = text.as_bytes();
    let root = tree.root_node();
    let mut func_stack = vec![root];

    while let Some(node) = func_stack.pop() {
        let kind = node.kind();
        let is_scope = matches!(
            kind,
            "function_item"
                | "function_definition"
                | "method_definition"
                | "function_declaration"
                | "arrow_function"
                | "closure_expression"
                | "lambda"
                | "block"
        );

        if is_scope {
            // Collect identifiers in this scope.
            let mut idents: Vec<&[u8]> = Vec::new();
            let mut id_stack = vec![node];
            while let Some(n) = id_stack.pop() {
                if n.kind() == "identifier" || n.kind() == "name" {
                    let start = n.start_byte();
                    let end = n.end_byte().min(source.len());
                    if end > start {
                        idents.push(&source[start..end]);
                    }
                }
                for i in 0..n.child_count() {
                    if let Some(child) = n.child(i) {
                        id_stack.push(child);
                    }
                }
            }

            let total = idents.len().max(1);
            let mut sorted = idents.clone();
            sorted.sort_unstable();
            sorted.dedup();
            let unique = sorted.len();
            let ratio = unique as f32 / total as f32;
            let score = (ratio * 255.0).clamp(0.0, 255.0);

            let start_line = node.start_position().row;
            let end_line = node.end_position().row;
            for line in start_line..=end_line.min(line_count.saturating_sub(1)) {
                if line < per_line.len() && score > per_line[line] {
                    per_line[line] = score;
                }
            }
        }

        for i in (0..node.child_count()).rev() {
            if let Some(child) = node.child(i) {
                func_stack.push(child);
            }
        }
    }

    per_line
}

/// Compute line start byte offsets from a string.
fn compute_line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (idx, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

/// Normalize grid values to span the full 0-255 range so that the density
/// threshold works correctly regardless of the encoder's raw value distribution.
fn normalize_grid(grid: &mut SeedValueGrid) {
    let values = grid.values();
    if values.is_empty() {
        return;
    }
    let min_val = values.iter().copied().min().unwrap_or(0);
    let max_val = values.iter().copied().max().unwrap_or(0);
    if min_val == max_val {
        return; // uniform grid — nothing to normalize
    }
    let range = (max_val - min_val) as f32;
    for v in grid.values_mut() {
        *v = ((*v - min_val) as f32 / range * 255.0).round() as u8;
    }
}

/// Shared noise application for all structural/AST encoders.
fn apply_structural_noise(
    grid: &mut SeedValueGrid,
    size: usize,
    seed_nonce: u64,
    bytes: &[u8],
    variant: u8,
) {
    let total = size * size;
    let mut rng =
        SplitMix64::new(seed_nonce ^ stable_hash_bytes(bytes) ^ (variant as u64) ^ 0x57ac_u64);
    for idx in 0..total {
        let x = idx % size;
        let y = idx / size;
        let base = grid.get(x, y) as i16;
        let noise = ((rng.next_u64() >> 56) as i16).wrapping_sub(128) / 10;
        grid.set(x, y, (base + noise).clamp(0, 255) as u8);
    }
}

fn hilbert_index_to_xy(order: u32, index: u32) -> (u32, u32) {
    let mut x = 0u32;
    let mut y = 0u32;
    let mut t = index;
    let mut s = 1u32;
    let n = 1u32 << order;
    while s < n {
        let rx = (t / 2) & 1;
        let ry = (t ^ rx) & 1;
        let (nx, ny) = rot(s, x, y, rx, ry);
        x = nx + s * rx;
        y = ny + s * ry;
        t /= 4;
        s *= 2;
    }
    (x, y)
}

fn rot(n: u32, x: u32, y: u32, rx: u32, ry: u32) -> (u32, u32) {
    if ry == 0 {
        if rx == 1 {
            return (n - 1 - x, n - 1 - y);
        }
        return (y, x);
    }
    (x, y)
}

#[cfg(test)]
#[path = "tests/seed.rs"]
mod tests;
