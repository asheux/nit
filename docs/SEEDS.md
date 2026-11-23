# Turn a file into a Game of Life seed

A seed is the starting arrangement of live cells. nit can derive it from raw file bytes or from the structure of the code in the editor. The GoL lab lets you inspect that arrangement, run it under different rules and save it for comparison.

## See a file evolve

1. Open a source file with `nit gol path/to/file.rs`. From the normal station, `:gol` opens the visualizer.
2. Enter `:gol encoder token_spectrum` to map the file’s syntax to cells. Enter `:gol seed` repeatedly to cycle the views below.
3. Enter `:gol run` to open the Petri Dish. Press **Space** to pause, then **Enter** to advance one generation. In this popup, **Ctrl+R** reseeds from the current code.
4. Return to the visualizer and click **SNAP** to save the seed as an RLE pattern and JSON metadata under `gol-snapshots/`. In the Petri Dish, **S** captures the current simulation state instead.

| Seed view | What to look for |
| --- | --- |
| GENOME | The encoder’s values before nit places the seed on the board. |
| PLATE | The live cells at generation zero, after density thresholding, placement and symmetry. |
| MAP | Groups of touching live cells. Diagonal neighbors belong to the same group. |
| STATS | Starting density, number of groups and grid dimensions. Density is the share of cells initially alive. |

## Choose what the pattern represents

Use `:gol encoder NAME` to select one encoder directly. With the visualizer focused, **Ctrl+E** cycles encoders and **Ctrl+R** cycles views.

| Encoder | Input represented |
| --- | --- |
| `token_spectrum` | Syntax roles such as declarations, branches and literals. |
| `ast_structure` | Depth and variety of nodes in the syntax tree. |
| `complexity_field` | Nesting, control flow and diversity across successive parts of the file. |
| `structural` | Changes in syntax depth and sequences of roles. |
| `ascii_bytes` | Raw bytes distributed across rows. |
| `hilbert_bits` | Raw bytes laid out along a curve that keeps nearby positions close together. |
| `lifehash16` | A reproducible 16 × 16 pattern derived from the file’s hash. |

The first four need a supported syntax parser. For a file format without one, the byte encoders still give you a pattern. When comparing source revisions, keep the encoder, parameters and rule fixed so the comparison isolates the edit.

## Find another starting pattern

Click **SEED** to cycle symmetry. Click **SEARCH** to try variations in density, jitter, placement, padding and symmetry. **APPLY** selects the best candidate found. The search favors more connected groups and penalizes the gap between actual and target density. A high search score describes the **starting layout**. Run the pattern to see how long it evolves.

The defaults use centered placement, one cell of padding, mirror-X symmetry, target density 0.31 and jitter 0.04. The target sets a threshold. Mirroring and the encoder’s value distribution also affect how many cells actually start alive. [Simulation rules](RULES.md) · [Visualizer controls](KEYBINDINGS.md)

## Read a file’s structural quality report

Open **Code Structural Quality → STATS** for the current file. The report runs four encodings under Conway’s rule, with a limit of 3,000 generations. For each encoding, the reported count ends when the board empties, a previous arrangement repeats or the limit is reached. A repeating pattern can continue indefinitely after the count stops.

The file tier draws on the three syntax encoders listed first above. Take the lowest generation count and add 15% of its gap to the second-lowest, up to a 200-generation increase. The structural encoder contributes to the comparison across encoders.

| Tier | Adjusted generations |
| --- | ---: |
| I · Still Life | 0–50 |
| II · Oscillator | 51–200 |
| III · Spaceship | 201–500 |
| IV · Methuselah | 501–2,000 |
| V · Replicator | 2,001+ |

**Consistency** measures how closely the encoders’ generation counts agree. **Parsimony** flags patterns such as many very small functions or repeated comment lines. A flag caps a simulated tier at IV. Files below 20 significant lines skip simulation, so a small-file auto-pass has no measured generation counts.

Use the tier to compare how code structure maps into this simulation. To judge a program’s behavior or speed, run its tests or benchmark the relevant workload. Enter `:skills` to browse the encoder and scoring references.
