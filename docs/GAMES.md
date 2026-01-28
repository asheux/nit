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
