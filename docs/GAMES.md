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
fast_eval = true | false

[history]
enabled = true | false
include_cycle_metadata = true | false
```

## Headless CLI

Run without the TUI:

```
nit games run --config games.toml --out . --format pretty
```

Sweep a parameter grid:

```
nit games sweep --config games.toml --rounds 200,500 --noise 0.0,0.05 --repetitions 1,3
```

## Run layout

Runs are stored under `runs/games/<timestamp>__seed-<seed>/` and include:

- `run_summary.json` (schema v2)
- `definitions.json` and `results.json`
- `events.ndjson` and `history.ndjson` (when enabled)
- `config.toml` snapshot
- `analysis/` outputs

Legacy `run__*.json` summaries under `games-runs/` or `output/` are still readable.

### Migration notes (schema v2)
- `run_summary.json` now includes `run_dir` and expanded `paths` entries
  (`definitions`, `results`, `config`, `analysis_dir`).
- Older schema v1 summaries still load, but those fields will be missing (`null`).

## History analysis

The Games history log (`history.ndjson` or legacy `history__*.ndjson`) can be analyzed to produce:
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

## Fast evaluator (Phase 3)

When `engine.fast_eval = true`, the kernel uses an analytical evaluator if **both**
strategies are deterministic and finite (builtins, FSM, memory‑n) and `noise = 0`.
It performs cycle detection and sums rounds in `O(mu + lambda)` time.

Cycle metadata (transient length, cycle length, cooperation rates) can be emitted
into `history.ndjson` by setting `history.include_cycle_metadata = true`.
