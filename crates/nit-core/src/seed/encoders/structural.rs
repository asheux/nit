//! Structural encoder + shared tree-sitter / language detection used by every
//! AST-aware encoder in this directory. Maps semantic token-role features to
//! GoL genomes via four channels: role diversity (35%) — count of distinct
//! roles per chunk; AST depth (25%) — nesting depth from tree-sitter, not
//! brackets; role entropy (20%) — Shannon entropy per window; role n-gram
//! (20%) — uniqueness of role 4-grams. Tokens are mapped to a 32×32 grid via
//! Hilbert curve. Varied structure produces rich GoL genomes; uniform code
//! produces flat grids that die quickly.

use std::path::Path;

use tree_sitter::{Parser, Tree};

use crate::seed::grid_types::{SeedEncoder, SeedInput, SeedValueGrid};
use crate::seed::utils::{apply_structural_noise, hilbert_index_to_xy, normalize_grid};
use crate::seed::view_modes::SeedEncoderId;

use super::ast_features::{compute_ast_features, AstFeatures, ROLE_BAND_COUNT};

const STRUCTURAL_ROLE_NGRAM: usize = 4;
const STRUCTURAL_ROLE_NGRAM_SEARCH: usize = 256;

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
        // The old byte path was the easiest avenue for gaming the score
        // (every comment / identifier character moved a cell). Returning a
        // uniform grid here is the right "unknown" signal; callers see
        // `density = 0` and treat it as no information.
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
        // Hash-based noise driven by the canonical AST features, not raw
        // source bytes. Same purpose (deterministic per-seed perturbation),
        // immune to comment / identifier / whitespace changes.
        apply_structural_noise(&mut grid, size, seed_nonce, features.feature_hash, variant);
        grid
    }
}

/// Convert canonical AST features into the (role, depth) token stream the
/// Structural encoder's helpers expect. Depth is rescaled to 0-255 to match
/// the prior behaviour of `ast_depth_per_byte`.
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

struct SemanticToken {
    role: u8,
    depth: u8,
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

fn role_diversity(tokens: &[SemanticToken], grid_cells: usize) -> Vec<f32> {
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

fn token_depth_gradient(tokens: &[SemanticToken], grid_cells: usize) -> Vec<f32> {
    chunked_token_map(tokens, grid_cells, |chunk| {
        let n = chunk.len() as f32;
        let sum: f32 = chunk.iter().map(|t| t.depth as f32).sum();
        (sum / n).min(255.0)
    })
}

fn role_entropy(tokens: &[SemanticToken], grid_cells: usize) -> Vec<f32> {
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

fn role_ngram_uniqueness(tokens: &[SemanticToken], grid_cells: usize) -> Vec<f32> {
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

// ---------------------------------------------------------------------------
// Tree-sitter helpers shared by every AST-driven seed encoder. SeedLanguage is
// duplicated here (rather than reused from nit-syntax) to avoid a dependency
// cycle.
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum SeedLanguage {
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
}

pub(super) fn seed_parse(text: &str, file_path: Option<&Path>) -> Option<(Tree, SeedLanguage)> {
    let lang = SeedLanguage::detect(file_path)?;
    let mut parser = Parser::new();
    parser.set_language(lang.ts_language()).ok()?;
    let tree = parser.parse(text, None)?;
    Some((tree, lang))
}

// Deterministic semantic mapping of an AST node kind to a 0-255 weight.
// Declarations carry the most structural signal; literals and identifiers the
// least. Used by AstStructureEncoder to project node identity onto the genome.
pub(super) fn ast_node_class(kind: &str) -> u8 {
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
    if kind.contains("expression")
        || kind.contains("call_")
        || kind.contains("binary_")
        || kind.contains("unary_")
        || kind.contains("assignment")
    {
        return 170;
    }
    if kind.contains("statement")
        || kind.contains("block")
        || kind == "source_file"
        || kind == "program"
    {
        return 130;
    }
    if kind.contains("type") || kind.contains("parameter") || kind.contains("argument") {
        return 90;
    }
    if kind.contains("literal")
        || kind.contains("string")
        || kind.contains("number")
        || kind == "identifier"
    {
        return 50;
    }
    100
}
