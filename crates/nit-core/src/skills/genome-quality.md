# Interpret a genome tier

Nit encodes a source file into cell grids and runs Conway's Game of Life. For each grid it records how many generations elapsed before extinction, a detected cycle, or the simulation limit. A tier summarizes this experiment. The labels describe score bands. “Replicator,” for example, does not establish that a replicating pattern was observed. Functional correctness, speed and maintainability need their own evidence.

| Tier | Label | Effective generation count |
|---|---|---:|
| I | Still Life | 0–50 |
| II | Oscillator | 51–200 |
| III | Spaceship | 201–500 |
| IV | Methuselah | 501–2,000 |
| V | Replicator | 2,001 or more |

For a simulated file, Nit takes the lowest generation count among three syntax-based encoders. It adds 15% of the gap to the second-lowest count, rounded down, up to 200 additional generations. The table uses this adjusted value. A flagged parsimony check caps the simulated tier at IV.

## Respond to feedback

Aim for Tier III or better with the simplest correct solution. If Nit requests a retry, compare the before and after reports and examine the changed code. Keep improvements that address the task or make the implementation easier to read, maintain or verify. Explain a justified tradeoff if the score stays low. Review behavior with relevant tests and measure performance with a representative workload when it matters.

## EQUILIBRIUM RULE

The parsimony check flags source patterns for review. For example, it can flag more than 35% comment lines in a file with at least 40 nonblank lines, or a file with at least 12 functions where more than half have five or fewer significant body lines. It also checks average body length in larger files and repeated identical line comments. A flag calls for inspecting the source and its purpose.

Keep necessary comments, types and well-named helpers. Avoid splitting clear functions into trivial wrappers, duplicating near-identical bodies, adding unused types, varying signatures or padding comments to change a score. Prefer an abstraction when it simplifies real callers.

Nit skips the simulations for files with fewer than 20 significant lines. The report uses a preset tier and can still contain parsimony findings. A small re-export file is a legitimate result. Keep it minimal.
