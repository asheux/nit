# ENCODER GUIDE

Nit derives four different cell grids from one source file and runs the same Game of Life rule on each. The resulting generation counts describe the encoded grids. Different counts can arise from a small source edit or a change in grid size. They do not identify a bug or prove an architectural improvement.

| Encoder | Input represented by the grid | Role in the report |
|---|---|---|
| `token_spectrum` | Distribution of token roles | Tier and consistency |
| `ast_structure` | Syntax-tree features such as nesting and branching | Tier and consistency |
| `complexity_field` | Local features such as branching, nesting and identifier use | Tier and consistency |
| `structural` | A separate encoding of source structure | Consistency |

## Compare two reports

1. Compare the same path, language and evaluation settings before and after the change. Check the grid size and each encoder's generations, density and component count. The production simulation stops by 3,000 generations, so two runs at the ceiling cannot be ranked by a longer observed lifetime.
2. Inspect control flow, function boundaries, signatures and repeated expressions in the diff. With a supported parser, the syntax encoders filter out many comments and other non-code nodes. Comments can still affect the separate parsimony check, and a change in line number alone can move the reported location of an existing finding. Preserve comments that explain assumptions and decisions.
3. Read cross-encoder consistency as agreement of the four generation counts. Nit computes one minus their population standard deviation divided by their mean, clamped to the range 0–1. Four equally short nonzero runs can agree perfectly. For a simulated file with a zero mean, the consistency score is zero. Small-file preset reports skip this calculation.
4. Check the code and the task independently. Use tests for behavior, benchmarks for performance, and review for readability and maintenance costs.

## Working targets

Aim for Tier III or better, cyclomatic complexity of 8 or less per function, average nesting depth of 3 or less, and cross-encoder consistency of at least 0.50 where appropriate. Treat these as Nit review targets. Avoid changing names, inventing types or adding helpers solely to reach a threshold. If a retry flags a drop, address concrete issues you can justify and report relevant remaining tradeoffs.

Nit evaluates saved changes automatically. Do not call `evaluate_genome`. Keep small files minimal. The `genome-quality` skill explains the small-file preset, tier calculation and parsimony flags.
