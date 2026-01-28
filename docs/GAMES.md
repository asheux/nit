# Games Engine

## Engine modes

The games engine exposes two execution styles:

- **Interactive stepper** (`TournamentRunner`): round-by-round stepping for inspectability.
- **Batch kernel** (`TournamentKernel`): tight-loop tournament execution for headless runs.

The TUI runs the interactive stepper in a background worker thread; the headless CLI uses the batch kernel.

## Deterministic seeding

All stochastic behavior is derived from a single **run seed**. Per-match RNGs are derived
deterministically so results do not depend on match order or thread scheduling.

Seed derivation:

```
base_strategy_seed = hash(run_seed, role, strategy_id)
match_strategy_seed = hash(base_strategy_seed, match_id, repetition)
noise_seed = hash(hash(run_seed, "noise"), match_id, repetition)
```

This is applied to:
- `RandomStrategy` (and future stochastic strategies)
- noise flips (`noise` in config)

## Parallel logging

When parallel execution is enabled, events/history are written via channel-backed writers.
**NDJSON line order is nondeterministic.** Each event/record includes:

- `match_id` (0-based stable id)
- `match_index` (1-based display index)

Use these fields to reconstruct order.

## Config knobs

```
[engine]
mode = "interactive" | "batch"
parallelism = "auto" | "off" | { threads = N }
progress_interval_ms = 0.. (UI update/log throttling)
```

## Headless CLI

Run without the TUI:

```
nit games run --config games.toml --out output --format pretty
```

## History analysis

The Games history log (`history__*.ndjson`) can be analyzed to produce:
- Per-match summaries (overall + tail-window stats)
- Per-strategy cooperation rates
- Cooperation trajectories for random matchups

TUI command prompt:

```
:games analyze [path] [tail=10000] [samples=50]
```

Outputs are written next to the history log:
- `analysis__<stamp>__.json` (summary + strategy stats)
- `analysis_matches__<stamp>__.csv` and `analysis_matches__<stamp>__.ndjson`
- `analysis_strategies__<stamp>__.csv`
- `analysis_trajectories__<stamp>__.csv`

Random matchups are detected by strategy ids containing `rand` or `random` (case-insensitive).
