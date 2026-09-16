# Seed Encoding: Code as Genome

nit encodes the file open in the editor as a genome for the Game of Life
simulation. Every keystroke reshapes the pattern, and every refactor
changes the organism. This doc covers the encoders, seed parameters and
views, hashing, seed search, genome tiers, and the parsimony rule that
scores agent edits. It is for contributors and for anyone tuning genome
feedback.

## Encoding Pipeline

```
editor text → encoder → value grid (0-255) → jitter → density threshold → symmetry → simulation grid
```

1. Encoder: reads the source and produces a 2D grid of cell values from 0
   to 255.
2. Jitter: adds a pseudorandom offset of up to `jitter × 32` (rounded) to
   each cell, so equal values do not band.
3. Threshold: a cell is alive when its value is at least
   `(1 - target_density) × 255`.
4. Symmetry: mirrors the grid (mirror-x, mirror-y, rotate-180, or none)
   with union semantics. If either side is alive, both are.
5. Simulation: places the binary grid on the board with `placement` and
   `padding`, then runs Conway's Life or any of the 28 built-in rules.

## Encoders

nit ships seven encoders. Four are scored: the three AST-driven encoders
set the genome tier, and `structural` adds a fourth quality signal. The
three byte-level encoders are display only. `Ctrl+E` cycles them in this
order: `ascii_bytes`, `hilbert_bits`, `lifehash16`, `structural`,
`token_spectrum`, `ast_structure`, `complexity_field`.

### AST-Driven Encoders

These encoders parse the file with tree-sitter and read one shared feature
projection (`encoders/ast_features.rs`): an entry per significant AST node
with its role band, a 0-255 kind weight, depth, and control-flow depth.
Comments, attributes, and macro invocations are left out, so comment
padding, renames, and whitespace changes cannot move the seed. Each scored
encoder then stretches its grid to the full 0-255 range and adds noise
keyed on a hash of the features, never on the raw bytes.

The encoders score every language in the `LANGUAGES` table that has a
tree-sitter grammar arm in `nit-core/src/seed/encoders/lang.rs` (28 of the
29 highlighted languages). Dockerfile and Wolfram are detected but skipped.
There is no byte-level fallback: when tree-sitter cannot parse a file, the
scored encoders return a uniform grid.

#### Token Spectrum (default)

- Grid size: 32×32
- Encoder ID: `token_spectrum`
- Method: one value per AST node, taken from the node's semantic role band.

| Role band | Value |
|---|---|
| Declaration | 240 |
| Control flow | 200 |
| Expression | 160 |
| Statement | 130 |
| Type | 95 |
| Literal | 60 |
| Other | 25 |

- Grid mapping: node values are chunked in order, averaged into 1024 cells,
  and laid out along a Hilbert curve.
- Anti-gaming property: the pattern comes from the spread of roles across
  the file. Identifier length, comments, and whitespace change nothing;
  only adding, removing, or restructuring code does.

#### AST Structure

- Grid size: 32×32
- Encoder ID: `ast_structure`
- Method: nodes are chunked in order into 1024 cells. Each cell mixes the
  chunk's average kind weight (30%), its maximum depth (30%), and how many
  of the seven role bands it spans (25%), plus a small constant baseline.
- Grid mapping: Hilbert curve. Cells past the last node repeat a fill
  value so the tail is not empty.
- Anti-gaming property: deeply nested code with large functions produces
  dense blobs that collapse. Flat, modular code with small functions
  produces separated components that sustain.

#### Complexity Field

- Grid size: 32×32
- Encoder ID: `complexity_field`
- Method: nodes are chunked in order into 32 windows, one per grid row.
  Each window scores four metrics:
  - Nesting depth (25%): maximum AST depth, normalized against 15.
  - Cognitive complexity (30%): each control-flow node counts 1 plus its
    control-flow ancestors, capped at 36.
  - Role entropy (25%): Shannon entropy of the role bands in the window.
  - Role diversity (20%): distinct role bands out of seven.
- Grid mapping: the Y axis is the window index, so it follows node order
  rather than line number. Every column in a row shares the row value
  before noise is added.
- Anti-gaming property: a good score needs low cognitive complexity, high
  entropy, high diversity, and moderate nesting at the same time. That
  combination describes well-written code; optimizing one axis hurts the
  others.

### Hybrid Encoder

#### Structural

- Grid size: 32×32
- Encoder ID: `structural`
- Method: turns each AST node into a token (role band plus scaled depth)
  and computes four per-cell channels: role diversity (35%), depth gradient
  (25%), role entropy (20%), and role 4-gram uniqueness (20%).
- Grid mapping: Hilbert curve.
- Position: scored in the quality report alongside the AST-driven encoders,
  but it does not set the tier. Varied structure gives rich genomes;
  uniform code gives flat grids that die quickly.

### Byte-Level Encoders

These work on raw bytes and know nothing about code.

#### ASCII Bytes

- Grid size: 32×32
- Encoder ID: `ascii_bytes`
- Method: maps text bytes into the grid in row-major order. Each byte is
  mixed with `SplitMix64` noise: `base_byte + (index * 31) ^ rng_byte`.

#### Hilbert Bits

- Grid size: 32×32
- Encoder ID: `hilbert_bits`
- Method: same as ASCII Bytes, but cells follow a Hilbert space-filling
  curve, so bytes near each other in the file stay near each other on the
  grid.

#### Lifehash 16

- Grid size: 16×16
- Encoder ID: `lifehash16`
- Method: pure noise from a PRNG seeded by a hash of the text. The output
  is deterministic but uniformly distributed, so one changed character
  gives an entirely different pattern. Best used with symmetry modes.

## Seed Parameters

| Parameter | Default | Range | Description |
|---|---|---|---|
| `symmetry` | mirror-x | none, mirror-x, mirror-y, rotate-180 | Spatial symmetry applied after encoding. Union semantics. |
| `target_density` | 0.31 | 0.08 - 0.7 | Target share of alive cells. Sweet spot: 0.2-0.4. |
| `padding` | 1 | 0+ | Border padding in cells around the placed seed. |
| `placement` | center | center, top-left | Where the seed sits on the simulation board. |
| `jitter` | 0.04 | 0.0 - 0.25 | Perturbation amplitude: ±1 at the default, ±8 at 0.25. |

## Seed Views

Cycle with `Ctrl+R`:

| View | Description |
|---|---|
| GENOME | Raw encoder output before placement, rendered per encoder (number grid, Hilbert stream, and so on). |
| PLATE | Final placed grid, rendered in the current preview mode (solid, half-block, braille, tissue, heatmap). |
| MAP | Component connectivity. Each connected component gets its own id. |
| STATS | Text statistics: density, component count, base grid dimensions. |

## Seed Hashing and Reproducibility

Every seed is reproducible. `hash_seed()` produces a 64-bit identity hash
with BLAKE3 incremental hashing from the encoder id, the parameters
fingerprint, the variant, the grid dimensions, and the cell contents. The
fingerprint quantizes density and jitter to 1e-6.

Two hashes are tracked:

- `input_hash`: hash of the AST features, or of the raw text when the file cannot be parsed. Stable across encoders.
- `seed_hash`: hash of the final bits after all transformations. Unique
  per seed configuration.

Snapshots store the full encoding context: encoder id, parameters
fingerprint, input hash, seed hash, density, and component count. Any
pattern can be recreated from the same source file and parameters.

All randomness (jitter, encoders, seed search mutations) uses `SplitMix64`
from `crates/nit-utils/src/rng.rs`, a full-period 2^64 PRNG. Jitter takes
the upper bits (`>> 48`) before the modulo to avoid low-bit correlation.

## Seed Source

The seed source is the editor buffer (`GolSeedSource::Editor`) or the
notes buffer (`GolSeedSource::Notes`). Toggle it with `Ctrl+Y`.

The seed runtime debounces updates by 120 ms (`DEFAULT_DEBOUNCE_MS` in
`seed_runtime.rs`). It detects parameter changes by `PartialEq` on
`SeedParams`, so even tiny changes trigger recomputation.

## Seed Search

Toggle with `Ctrl+G` or the SEARCH title button. A background worker
mutates symmetry, placement, density, jitter, and padding, and scores each
candidate:

```
score = component_count - 40 * |actual_density - target_density|
```

Apply the best proposal with `Ctrl+A` or the APPLY title button. The title
bar also has SEED (cycle symmetry) and SNAP (snapshot) buttons; see
`docs/KEYBINDINGS.md`.

## Commands

| Command | Description |
|---|---|
| `:gol encoder` | Cycle to the next encoder (alias `:seed encoder`) |
| `:gol encoder <name>` | Switch to a named encoder (alias `:seed encoder <name>`) |
| `:gol seed` | Cycle seed view, GENOME → PLATE → MAP → STATS (alias `:seed view`) |
| `Ctrl+E` | Cycle encoder |
| `Ctrl+R` | Cycle seed view |
| `Ctrl+S` | Cycle symmetry |
| `Ctrl+N` | Snapshot current seed |
| `Ctrl+G` | Toggle seed search |
| `Ctrl+A` | Apply search proposal |
| `Ctrl+Y` | Toggle seed source (editor or notes) |

## Genome Tiers

Each AST-driven encoder's seed is simulated, and its generation count maps
to a tier:

| Tier | Name | Generations |
|---|---|---|
| I | Still Life | 0-50 |
| II | Oscillator | 51-200 |
| III | Spaceship | 201-500 |
| IV | Methuselah | 501-2000 |
| V | Replicator | 2001+ |

### Soft bottleneck rule

The file's tier comes from a soft bottleneck across the three AST-driven
encoders rather than a plain minimum. The weakest encoder still dominates,
but the gap to the next encoder adds a modest lift, capped at 200
generations. This reduces the incentive to over-engineer just to lift one
lagging encoder.

```
effective_min = raw_min + min(gap_to_next * 15%, 200)
tier = GenomeTier::from_generations(effective_min)
```

### Small-file bypass

Files with fewer than 20 significant lines skip simulation and get Tier III
(Spaceship) with 100% consistency. This stops agents padding trivial files
(lib.rs, mod.rs, re-exports) with needless code. The parsimony check still
runs on them.

## Parsimony Rule

Without parsimony pressure, agents can game genome scores by inflating
structure: splitting functions into tiny pieces, padding with comments, or
adding type declarations only for token diversity. The parsimony rule
detects and penalizes that bloat.

### How it works

After the encoder scores, `compute_genome_report` runs a parsimony
analysis on the file's AST. Four bloat signals are checked:

| Signal | Threshold | Detects |
|---|---|---|
| Over-split functions | 40+ significant lines, 15+ functions averaging < 3 significant lines | Mass function splitting to inflate structure scores |
| Comment padding | 40+ non-blank lines, > 35% of them comments | Doc comments and section markers added for token diversity |
| Tiny-function fraction | 12+ functions with > 50% having ≤ 5 significant lines | Predicate over-extraction, stub duplication |
| Duplicate comment lines | ≥ 1 repeated comment line | Copy-pasted doc headers |

Any signal sets `bloat_detected = true`, which caps the tier at Methuselah
(IV). Replicator (V) needs good code, not well-gamed metrics. Softer Info
nudges fire earlier, at 10+ functions and at a 30% comment ratio.

### Retry guardrails

When an agent's edit lowers genome quality, nit retries automatically:

- At most 3 retries per turn (`GENOME_RETRY_LIMIT`). This avoids retry
  spirals that compound over-engineering.
- No retry-side file-size gate. The small-file bypass already filters
  trivial files, and the parsimony detector catches over-engineering on
  the next pass.
- Retry prompts warn agents against over-engineering during fixes.

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

Agents get an equilibrium rule in their system prompt that lists what not
to do:

- Do not split clear functions into many tiny ones.
- Do not extract trivial predicates into their own functions.
- Do not copy-paste function bodies to create stubs.
- Do not add comments to boost scores; comments explain non-obvious logic
  only.
- Do not add types or traits that serve no functional purpose.
- Do not vary function signatures purely for token diversity.

### Key constants

The parsimony thresholds are fields of the `THRESHOLDS` table in
`crates/nit-core/src/genome_report/parsimony.rs`.

| Constant | Value | Meaning |
|---|---|---|
| `THRESHOLDS.min_lines` | 40 | Minimum lines for the over-split and comment-padding signals |
| `THRESHOLDS.avg_fn_body_threshold` | 3.0 | Average function body below which over-split fires |
| `THRESHOLDS.min_fn_count` | 15 | Minimum function count for the over-split flag |
| `THRESHOLDS.comment_ratio_threshold` | 0.35 | Comment ratio above which comment padding fires |
| `THRESHOLDS.tiny_fn_lines` | 5 | Body size at or below which a function is "tiny" |
| `THRESHOLDS.tiny_fn_fraction_threshold` | 0.50 | Tiny fraction above which the flag fires |
| `THRESHOLDS.tiny_fn_min_count` | 12 | Minimum function count for the tiny-function flag |
| `THRESHOLDS.duplicate_comment_threshold` | 1 | Repeated comment lines that trigger the flag |
| `THRESHOLDS.info_fn_count_threshold` | 10 | Function count that emits an Info nudge |
| `THRESHOLDS.comment_ratio_info_threshold` | 0.30 | Comment ratio that emits an Info nudge |
| `SOFT_BOTTLENECK_LIFT_PCT` | 15 | Share of the gap to the next encoder added as lift |
| `SOFT_BOTTLENECK_MAX_LIFT` | 200 | Maximum generation lift from the soft bottleneck |
| `GENOME_MIN_SIGNIFICANT_LINES` | 20 | Small-file auto-pass threshold |
| `GENOME_RETRY_LIMIT` | 3 | Maximum retries per agent turn |

`SOFT_BOTTLENECK_LIFT_PCT` and `GENOME_MIN_SIGNIFICANT_LINES` live in
`crates/nit-core/src/genome_report.rs`, `SOFT_BOTTLENECK_MAX_LIFT` in
`crates/nit-core/src/genome_report/simulation.rs`, and
`GENOME_RETRY_LIMIT` in `crates/nit-tui/src/app/genome_retry.rs`.

## Key Files

| File | Purpose |
|---|---|
| `crates/nit-core/src/seed/` | Encoder implementations and pipeline (`encoders/`, `params.rs`, `grid_types.rs`, `utils.rs`, `view_modes.rs`): symmetry, jitter, thresholding, hashing, component counting |
| `crates/nit-core/src/seed/encoders/ast_features.rs` | Shared AST feature projection used by every scored encoder |
| `crates/nit-core/src/seed/encoders/lang.rs` | `SeedLanguage` and the tree-sitter grammar arms |
| `crates/nit-core/src/genome_report.rs` | Report assembly, tier boundaries, soft bottleneck, small-file bypass |
| `crates/nit-core/src/genome_report/` | Tier scoring, parsimony analysis, recommendations, agent instructions (`format.rs`, `function_scores.rs`, `instructions.rs`, `outlier.rs`, `parsimony.rs`, `recommendations/`, `simulation.rs`, `source_scan.rs`) |
| `crates/nit-core/src/genome_storage/` | Disk-backed report cache (sharded, atomic writes, schema migrations) |
| `crates/nit-tui/src/seed_runtime.rs` | Runtime loop, change detection, debounce, search worker, snapshot dispatch |
| `crates/nit-tui/src/seed_render/` | GENOME view rendering, render cache, component analysis, preview modes |
| `crates/nit-tui/src/seed_snapshot.rs` | Snapshot I/O, deduplication, metadata persistence |
| `crates/nit-tui/src/genome_worker.rs` | Off-thread genome evaluation worker |
| `crates/nit-tui/src/app/genome_retry.rs` | Retry-prompt builder and cap (`GENOME_RETRY_LIMIT = 3`) |
| `crates/nit-tui/src/widgets/visualizer_view.rs` | Visualizer rendering, title bar buttons, click hit detection (`title_button_hit`) |
| `crates/nit-utils/src/rng.rs` | `SplitMix64` PRNG |
| `crates/nit-utils/src/hashing.rs` | BLAKE3 stable hashing (`stable_hash_bytes`) |
