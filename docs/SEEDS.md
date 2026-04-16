# Seed Encoding: Code as Genome

nit encodes the file open in the editor as a genome for the Game of Life simulation. Every keystroke reshapes the pattern. Every refactor changes the organism.

## Encoding Pipeline

```
editor text → encoder → value grid (0-255) → jitter → density threshold → symmetry → simulation grid
```

1. **Encoder** reads the source file and produces a 2D grid of cell values (0-255).
2. **Jitter** adds pseudorandom variation (±32) to prevent deterministic banding.
3. **Threshold** converts values to alive/dead based on target density. Values above `(1 - density) × 255` become alive.
4. **Symmetry** mirrors the grid (mirror-x, mirror-y, rotate-180, or none). Union semantics: if either side is alive, both are alive.
5. **Simulation** receives the final binary grid and runs Conway's Game of Life (or any of the 28 supported rules).

## Encoders

nit ships seven encoders in two categories.

### AST-Driven Encoders

These use tree-sitter to parse the source file and extract structural properties. Supported for 11 languages: Rust, Python, JavaScript, TypeScript, Bash, HTML, CSS, JSON, TOML, YAML, and Markdown. They gracefully fall back to byte-level analysis for unsupported languages.

#### Token Spectrum (default)

- **Grid size:** 32×32
- **Encoder ID:** `token_spectrum`
- **Method:** Classifies each byte of source code by its semantic role using tree-sitter syntax highlighting captures. Nine categories map to distinct value ranges:

| Category | Value Range |
|----------|------------|
| Whitespace | 0-20 |
| Comments | ~35 |
| Punctuation | ~65 |
| Operators | ~95 |
| Keywords | ~125 |
| Variables/parameters | ~153 |
| Types/namespaces | ~178 |
| String/number literals | ~203 |
| Function/method names | ~228 |
| Macros/attributes | ~248 |

- **Grid mapping:** Byte values are chunked and averaged into 1024 cells mapped via Hilbert curve.
- **Anti-gaming property:** Well-structured code has balanced category distributions. No comments means no low-value cushion; everything is alive and collapses. Pure comments produce all-dead grids.
- **Fallback:** When tree-sitter cannot parse the language, falls back to byte-category classification using ASCII ranges.

#### AST Structure

- **Grid size:** 32×32
- **Encoder ID:** `ast_structure`
- **Method:** Walks the syntax tree via depth-first traversal. For each named AST node, computes a cell value from four structural properties:
  - Nesting depth (30%): deeper nesting produces higher values
  - Child count / branching factor (25%): more children produce higher values
  - Byte span (25%): larger nodes (long functions) produce higher values
  - Node kind hash (20%): different node types produce different base values
- **Grid mapping:** Each node occupies grid cells proportional to its byte span. Mapped via Hilbert curve for spatial locality.
- **Anti-gaming property:** Deeply nested code with large functions produces dense blobs that collapse. Flat, modular code with small functions produces separated components that sustain.
- **Fallback:** Uses bracket-counting for depth and byte-category weighting when tree-sitter is unavailable.

#### Complexity Field

- **Grid size:** 32×32
- **Encoder ID:** `complexity_field`
- **Method:** Computes four per-line software metrics and combines them into a spatial heatmap:
  - **Nesting depth** (25%): Maximum AST nesting level per line, normalized to 0-255.
  - **Cyclomatic complexity** (30%): Decision points per function (if, match, while, for, boolean operators). Assigned to all lines the function spans. Normalized across all functions.
  - **Token entropy** (25%): Shannon entropy of highlight group categories per line. Diverse token usage scores high; uniform lines score low.
  - **Identifier uniqueness** (20%): Ratio of unique identifiers to total identifiers per scope. High ratio means diverse naming; low ratio means repetitive variables.
- **Grid mapping:** Y-axis maps to line number, X-axis maps to column position. Values blend the four metrics (80%) with per-byte token values (20%).
- **Anti-gaming property:** Requires simultaneously low cyclomatic complexity, high token entropy, high identifier uniqueness, and moderate nesting. This combination naturally describes well-written code. Optimizing one axis hurts the others.
- **Fallback:** Uses bracket-counting for nesting and byte-level Shannon entropy when tree-sitter is unavailable. Complexity and uniqueness layers default to neutral (128).

### Hybrid Encoder

#### Structural

- **Grid size:** 32×32
- **Encoder ID:** `structural`
- **Method:** Extracts four per-byte features without requiring a parser:
  - Local Shannon entropy (35%): Sliding-window (64 bytes) entropy normalized to 0-255.
  - Bracket nesting depth (25%): Counts `(){}[]` nesting, normalized by max depth.
  - Byte-category token signal (20%): Maps each byte to a structural weight by character class (letters=200, brackets=180, digits=160, operators=140, etc.).
  - N-gram uniqueness (20%): Distance to nearest repeated 4-gram within a 512-byte lookback window.
- **Grid mapping:** Hilbert curve mapping for spatial locality.
- **Position:** Sits between byte-level and AST-driven encoders. Captures structural patterns at the byte level without needing tree-sitter.

### Byte-Level Encoders

These operate on raw bytes. They do not understand code semantics.

#### ASCII Bytes

- **Grid size:** 32×32
- **Encoder ID:** `ascii_bytes`
- **Method:** Maps text bytes directly into the grid. Each byte's ASCII value is mixed with deterministic pseudo-random noise (`SplitMix64`). Formula: `base_byte + (index * 31) ^ rng_byte`.

#### Hilbert Bits

- **Grid size:** 32×32
- **Encoder ID:** `hilbert_bits`
- **Method:** Same as ASCII Bytes but uses a Hilbert space-filling curve for the byte-to-cell mapping instead of row-major order. Preserves spatial locality: bytes close together in the file remain close on the grid.

#### Lifehash 16

- **Grid size:** 16×16
- **Encoder ID:** `lifehash16`
- **Method:** Pure noise generator seeded by a hash of the input text. The output is deterministic but uniformly distributed. A single character change cascades into an entirely different pattern. Best used with symmetry modes.

## Seed Parameters

| Parameter | Default | Range | Description |
|-----------|---------|-------|-------------|
| `symmetry` | mirror-x | none, mirror-x, mirror-y, rotate-180 | Spatial symmetry applied after encoding. Union semantics. |
| `target_density` | 0.31 | 0.08 - 0.7 | Target proportion of alive cells. Sweet spot: 0.2-0.4. |
| `padding` | 1 | 0+ | Border padding in cells around the placed seed. |
| `placement` | center | center, top-left | Where the seed grid sits on the simulation board. |
| `jitter` | 0.04 | 0.0 - 0.25 | Random perturbation amplitude (±32 at default). |

## Seed Views

Cycle with `Ctrl+R`:

| View | Description |
|------|-------------|
| **GENOME** | Raw encoder output before placement. Encoder-specific rendering (number grid, Hilbert stream, etc.). |
| **PLATE** | Final placed grid ready for simulation. Rendered in the current preview mode (solid, half-block, braille, tissue, heatmap). |
| **MAP** | Component connectivity visualization. Each connected component gets a unique ID. |
| **STATS** | Text statistics: density, component count, base grid dimensions. |

## Seed Hashing and Reproducibility

Every seed is fully reproducible. `hash_seed()` produces a 64-bit identity hash via BLAKE3 incremental hashing. Inputs: encoder ID, parameters fingerprint, variant, grid dimensions, and cell contents.

Two hashes are tracked:
- **input_hash**: Hash of raw editor text bytes. Stable across encoding methods.
- **seed_hash**: Hash of final bits after all transformations. Unique per seed configuration.

Snapshots store the complete encoding context: encoder ID, parameters fingerprint, input hash, seed hash, density, and component count. Any pattern can be exactly recreated from the same source file and parameters.

## Seed Source

The seed source is the editor's open file buffer (`GolSeedSource::Editor`). An alternative source is the notes/scratch buffer (`GolSeedSource::Notes`). Toggle with the `:gol seed source` command.

The seed runtime debounces updates with a 120ms interval (hardcoded in `seed_runtime.rs`). Parameter changes are detected by direct `PartialEq` comparison on `SeedParams`, so arbitrarily small changes trigger recomputation.

## Seed Search

Toggle with `Ctrl+G` or the **SEARCH** title button. A background worker mutates seed parameters (density, jitter, symmetry) and evaluates variants against configurable fitness criteria. The best candidate can be applied with `Ctrl+A` (**APPLY**).

## Commands

| Command | Description |
|---------|-------------|
| `:gol encoder` | Cycle to next encoder |
| `:gol encoder <name>` | Switch to specific encoder by name |
| `:gol seed view` | Cycle seed view (GENOME → PLATE → MAP → STATS) |
| `:gol seed source` | Toggle seed source (editor ↔ notes) |
| `Ctrl+E` | Cycle encoder |
| `Ctrl+R` | Cycle seed view |
| `Ctrl+S` | Cycle symmetry |
| `Ctrl+N` | Snapshot current seed |
| `Ctrl+G` | Toggle seed search |
| `Ctrl+A` | Apply search proposal |

## Parsimony Rule

The genome system includes a **parsimony rule** that detects and penalizes over-engineered code. Without parsimony pressure, agents can game genome scores by inflating code structure — splitting functions into many tiny pieces, padding with comments, or adding unnecessary type declarations purely for token diversity. The parsimony system creates an equilibrium between structural quality and engineering effort.

### How it works

After computing encoder scores, `compute_genome_report` runs a parsimony analysis on the source file's AST. Three independent bloat signals are checked:

| Signal | Threshold | Detects |
|--------|-----------|---------|
| **Over-split functions** | 15+ functions averaging < 3 significant lines | Mass function splitting to inflate AST structure scores |
| **Comment padding** | > 40% of non-blank lines are comments | Adding doc comments / section markers for token diversity |
| **Tiny-function fraction** | 12+ functions with > 50% having ≤ 5 significant lines | Predicate over-extraction, stub duplication |
| **Duplicate comment lines** | ≥ 1 repeated comment line | Copy-pasted doc headers used to pad token diversity |

**Any** of these signals triggers `bloat_detected = true`, which **caps the tier at Methuselah (IV)**. This means Replicator (Tier V) requires genuinely good code, not just well-gamed metrics.

### Soft bottleneck rule

The tier calculation uses a **soft bottleneck** instead of a pure minimum across the three AST-driven encoders. The weakest encoder still dominates, but strong performance on the other encoders provides a modest lift (capped at 200 generations). This reduces the incentive to over-engineer just to boost one lagging encoder.

```
effective_min = raw_min + min(gap_to_next * 15%, 200)
tier = GenomeTier::from_generations(effective_min)
```

### Retry guardrails

When an agent's code degrades genome quality, nit automatically retries:

- **Max 3 retries** per turn (not 10) — avoids retry spirals that compound over-engineering
- **Files < 100 lines are skipped** — small files don't have enough structure for meaningful retry improvement
- Retry prompts warn agents against over-engineering during fixes

### Small-file bypass

Files with fewer than 20 significant lines receive an automatic Tier III (Spaceship) pass with 100% consistency. This prevents agents from padding trivial files (lib.rs, mod.rs, re-exports) with unnecessary code.

### Parsimony metrics in reports

The formatted genome report includes a parsimony line:

```
Parsimony: 12 fns, avg 8.3 lines/fn, 25% tiny, 18% comments
```

When bloat is detected:

```
Parsimony: 20 fns, avg 2.1 lines/fn, 80% tiny, 15% comments [BLOAT — tier capped]
```

### Agent instructions

Agents receive an equilibrium rule in their system prompt that explicitly lists what NOT to do:

- Do not split clear functions into many tiny ones
- Do not extract trivial predicates into their own functions
- Do not copy-paste function bodies to create stubs
- Do not add comments to boost scores — comments must explain non-obvious logic only
- Do not add types or traits that serve no functional purpose
- Do not vary function signatures purely for token diversity

### Key constants

| Constant | Value | Location |
|----------|-------|----------|
| `PARSIMONY_MIN_LINES` | 40 | Minimum significant lines for bloat detection |
| `PARSIMONY_AVG_FN_BODY_THRESHOLD` | 3.0 | Max avg fn body for over-split flag |
| `PARSIMONY_MIN_FN_COUNT` | 15 | Min fn count for over-split flag |
| `PARSIMONY_COMMENT_RATIO_THRESHOLD` | 0.40 | Max comment ratio before flag |
| `PARSIMONY_TINY_FN_LINES` | 5 | Body size threshold for "tiny" |
| `PARSIMONY_TINY_FN_FRACTION_THRESHOLD` | 0.50 | Max fraction of tiny fns before flag |
| `PARSIMONY_TINY_FN_MIN_COUNT` | 12 | Min fn count for tiny-fn flag |
| `PARSIMONY_DUPLICATE_COMMENT_THRESHOLD` | 1 | Min duplicate comment lines before flag |
| `SOFT_BOTTLENECK_MAX_LIFT` | 200 | Max generation lift from soft bottleneck |
| `GENOME_RETRY_LIMIT` | 3 | Max retries per agent turn (defined in `crates/nit-tui/src/app/mod.rs`) |
| `GENOME_RETRY_MIN_LINES` | 120 | Min file lines for retry eligibility (defined in `crates/nit-tui/src/app/mod.rs`) |

## Key Files

| File | Purpose |
|------|---------|
| `crates/nit-core/src/seed.rs` | All encoder implementations, symmetry, jitter, thresholding, hashing, component counting |
| `crates/nit-core/src/genome_report.rs` | Genome tier, parsimony analysis, soft bottleneck, recommendations, agent instructions |
| `crates/nit-tui/src/seed_runtime.rs` | Runtime orchestration, change detection, debounce, search worker, snapshot dispatch |
| `crates/nit-tui/src/seed_render/genome.rs` | GENOME view rendering for all encoders |
| `crates/nit-tui/src/seed_render/renderer.rs` | Render cache, component analysis, preview modes |
| `crates/nit-tui/src/seed_snapshot.rs` | Snapshot I/O, deduplication, metadata persistence |
| `crates/nit-tui/src/app/mod.rs` | Retry logic, retry min-lines gate, genome worker drain |
