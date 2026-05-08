//! Structural encoder + shared tree-sitter / language detection used by every
//! AST-aware encoder in this directory. Maps semantic token-role features to
//! GoL genomes via four channels: role diversity (35%) — count of distinct
//! roles per chunk; AST depth (25%) — nesting depth from tree-sitter, not
//! brackets; role entropy (20%) — Shannon entropy per window; role n-gram
//! (20%) — uniqueness of role 4-grams. Tokens are mapped to a 32×32 grid via
//! Hilbert curve. Varied structure produces rich GoL genomes; uniform code
//! produces flat grids that die quickly.

use std::path::Path;

use tree_sitter::{Parser, Query, QueryCursor, Tree};

use crate::seed::grid_types::{SeedEncoder, SeedInput, SeedValueGrid};
use crate::seed::utils::{apply_structural_noise, hilbert_index_to_xy, normalize_grid};
use crate::seed::view_modes::SeedEncoderId;

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
        let bytes = input.text.as_bytes();

        if bytes.is_empty() {
            return grid;
        }

        let tokens = match seed_parse(input.text, input.file_path) {
            Some((tree, lang)) => extract_semantic_tokens(input.text, &tree, lang),
            None => extract_byte_tokens(bytes),
        };

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
        apply_structural_noise(&mut grid, size, seed_nonce, bytes, variant);
        grid
    }
}

struct SemanticToken {
    role: u8,
    depth: u8,
}

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

// Whitespace-free sequence of (role, AST depth) pairs. A "token run" is a
// contiguous span of bytes sharing one highlight group; we collapse each run
// to a single entry tagged with the maximum AST depth observed within it.
fn extract_semantic_tokens(text: &str, tree: &Tree, lang: SeedLanguage) -> Vec<SemanticToken> {
    let groups = seed_highlight_bytes(text, lang, tree);
    let byte_depths = ast_depth_per_byte(tree, text.len());

    let mut tokens = Vec::with_capacity(text.len() / 4);
    let mut i = 0;
    while i < groups.len() {
        let group = match groups[i] {
            Some(g) => g,
            None => {
                i += 1;
                continue;
            }
        };
        let start = i;
        while i < groups.len() && groups[i] == Some(group) {
            i += 1;
        }
        let max_d = byte_depths[start..i].iter().copied().max().unwrap_or(0);
        tokens.push(SemanticToken {
            role: highlight_to_role(group),
            depth: max_d,
        });
    }
    tokens
}

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

// Fallback for files tree-sitter cannot parse: classify raw bytes by ASCII
// category, tracking nesting via bracket characters.
fn extract_byte_tokens(bytes: &[u8]) -> Vec<SemanticToken> {
    let mut tokens = Vec::with_capacity(bytes.len() / 2);
    let mut depth: u8 = 0;
    let mut max_depth: u8 = 0;
    for &b in bytes {
        match b {
            b'\n' | b'\r' | b'\t' | b' ' => continue,
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
    if max_depth > 0 {
        let scale = 255.0 / max_depth as f32;
        for t in &mut tokens {
            t.depth = (t.depth as f32 * scale).round().min(255.0) as u8;
        }
    }
    tokens
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

fn token_depth_gradient(tokens: &[SemanticToken], grid_cells: usize) -> Vec<f32> {
    chunked_token_map(tokens, grid_cells, |chunk| {
        let n = chunk.len() as f32;
        let sum: f32 = chunk.iter().map(|t| t.depth as f32).sum();
        (sum / n).min(255.0)
    })
}

fn role_entropy(tokens: &[SemanticToken], grid_cells: usize) -> Vec<f32> {
    let max_entropy = (ROLE_COUNT as f32).log2();
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

    fn highlights_query(self) -> &'static str {
        match self {
            Self::Rust => include_str!("../../../../nit-syntax/queries/rust/highlights.scm"),
            Self::Python => tree_sitter_python::HIGHLIGHT_QUERY,
            Self::JavaScript => tree_sitter_javascript::HIGHLIGHT_QUERY,
            Self::TypeScript => tree_sitter_typescript::HIGHLIGHT_QUERY,
            Self::Markdown => {
                include_str!("../../../../nit-syntax/queries/markdown/highlights.scm")
            }
            Self::Html => tree_sitter_html::HIGHLIGHT_QUERY,
            Self::Css => tree_sitter_css::HIGHLIGHTS_QUERY,
            Self::Json => tree_sitter_json::HIGHLIGHT_QUERY,
            Self::Toml => tree_sitter_toml::HIGHLIGHT_QUERY,
            Self::Yaml => include_str!("../../../../nit-syntax/queries/yaml/highlights.scm"),
            Self::Bash => tree_sitter_bash::HIGHLIGHT_QUERY,
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

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum SeedHighlight {
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

pub(super) fn seed_highlight_bytes(
    text: &str,
    lang: SeedLanguage,
    tree: &Tree,
) -> Vec<Option<SeedHighlight>> {
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

// Bands chosen so each highlight group maps to a contiguous, non-overlapping
// 25-30-unit slice of the 0-255 range. Whitespace gets the lowest band.
pub(super) fn seed_highlight_to_value(group: Option<SeedHighlight>) -> u8 {
    match group {
        None => 10,
        Some(g) => match g {
            SeedHighlight::Comment => 35,
            SeedHighlight::Punctuation => 65,
            SeedHighlight::Operator => 95,
            SeedHighlight::Keyword => 125,
            SeedHighlight::Variable => 153,
            SeedHighlight::Type => 178,
            SeedHighlight::StringLiteral => 203,
            SeedHighlight::Function => 228,
            SeedHighlight::Macro => 248,
        },
    }
}

// Byte-category fallback for TokenSpectrum when tree-sitter is unavailable.
pub(super) fn byte_category_value(b: u8) -> u8 {
    match b {
        b'\n' | b'\r' | b'\t' | b' ' => 10,
        b'/' => 35,
        b'(' | b')' | b'{' | b'}' | b'[' | b']' | b';' | b':' | b',' | b'.' => 65,
        b'+' | b'-' | b'*' | b'=' | b'<' | b'>' | b'!' | b'&' | b'|' | b'^' | b'%' => 95,
        b'"' | b'\'' | b'`' => 203,
        b'0'..=b'9' => 203,
        b'A'..=b'Z' => 178,
        b'a'..=b'z' | b'_' => 153,
        _ => 65,
    }
}
