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

This is currently applied to:
- noise flips (`noise` in config)

## Parallel logging

When parallel execution is enabled, events/history are written via channel-backed writers.
**NDJSON line order is nondeterministic.** Each event/record includes:

- `match_id` (0-based stable id)
- `match_index` (1-based display index)

Use these fields to reconstruct order.

## Config knobs

```
save_data = true | false

[engine]
mode = "interactive" | "batch"
parallelism = "auto" | "off" | { threads = N }
progress_interval_ms = 0.. (UI update/log throttling)
fast_eval = true | false
accelerator = "auto" | "cpu" | "metal"
score_aggregation = "mean" | "total"
complexity_cost.enabled = true | false
complexity_cost.tm_step_cost = 0.0
complexity_cost.fsm_state_cost = 0.0

[history]
enabled = true | false
include_cycle_metadata = true | false
  # when enabled, history.ndjson also includes per-match TM metrics (if applicable)
```

`save_data = false` disables all persisted run artifacts for normal tournament runs.

`score_aggregation` controls which per-strategy score is shown and aggregated:
- `mean` (default): average payoff per round
- `total`: cumulative payoff across all matches

Tournament scheduling is notebook-compatible:
- ordered pairs are played in both directions (`A vs B` and `B vs A`)
- `self_play` defaults to `true`
- in `mean` mode, the table shows `AggPayoff`, which follows Code-02 and sums matchup means rather than raw cumulative payoff

`accelerator` controls optional GPU offload on macOS:
- `auto` (default): use Metal opportunistically for compatible homogeneous batch paths
- `cpu`: disable Metal and stay on CPU/Rayon
- `metal`: prefer Metal on those same paths, with CPU fallback when the current run shape is unsupported

## Strategies (programs)

### FSM (Moore machine)

FSMs are deterministic Moore machines with a fixed output per state and transitions
based on an input symbol derived from the last round’s observation.

```
[[strategy]]
id = "my_fsm"
type = "auto" # optional: inferred as fsm from fields
states = 4    # alias: num_states
start_state = 1
input_index_base = 1
outputs = ["C","D","D","C"]    # length = states
input_mode = "opponent_last_action"  # default
transitions = [
  # Each row: state index; then next_state for each input symbol
  # For opponent_last_action => alphabet size = 2
  [1, 1, 2],
  [2, 2, 4],
  [3, 3, 1],
  [4, 4, 2],
]
```

Input mode:
- `opponent_last_action` only (notebook-compatible semantics)

Validation rules:
- `outputs.len == states`
- `transitions.len == states`
- each transition row is either `alphabet` entries or `alphabet+1` entries
  (leading state index)
- next states must be valid for selected indexing base:
  - `input_index_base = 0` => `0..(num_states-1)`
  - `input_index_base = 1` => `1..num_states`

### One-sided Turing machine

One-sided TMs are deterministic, bounded-step programs with notebook-compatible
history input semantics.

Per-round semantics:
- empty history returns `C` without running the TM
- `input = FromDigits[Flatten[history], 2]` (global A,B order; no player swap)
- the head starts on the least-significant base-`symbols` digit of that input
- run `OneSidedTuringMachineFunction(tm, input, max_steps_per_round)`
- if the run produces no output in time: action `D` (`halted=false`)
- if output symbol is `0`: action `C`; any non-zero output symbol maps to `D` (`halted=true`)
- each round starts from the same TM definition; there is no persistent streaming tape

#### Explicit transition table

```
[[strategy]]
id = "tm1"
type = "auto" # optional: inferred as tm from TM fields
states = 3
symbols = 2
start_state = 1
blank = 0
fallback_symbol = 0
max_steps_per_round = 256
input_mode = "opponent_last_action"
output_map = ["C","D"]
transitions = [
  # Wolfram-style table: transitions[state][read] = [next, write, move]
  # move can be "L"/"R"/"S" or -1/1/0
  [ [2, 1, "R"], [1, 0, "S"] ],
  [ [2, 1, "L"], [3, 1, "R"] ],
  [ [0, 0, "S"], [3, 1, "S"] ],
]
```

You can also use the explicit (state, read, write, move, next) object form:

```
transitions = [
  { state=1, read=0, write=1, move="R", next=2 },
  { state=1, read=1, write=0, move="S", next=1 },
  { state=2, read=0, write=1, move="L", next=2 },
  { state=2, read=1, write=1, move="R", next=3 },
  { state=3, read=0, write=0, move="S", next=0 },
  { state=3, read=1, write=1, move="S", next=3 },
]
```

#### Wolfram-style rule code

```
[[strategy]]
id = "tm_rule"
type = "tm"
states = 3
symbols = 2
start_state = 1
blank = 0
max_steps_per_round = 256
input_mode = "opponent_last_action"
output_map = ["C","D"]
rule_code = 600720
```

Rule decoding order (Wolfram-style one-sided TM):
- iterate `(state=states..1, read=0..symbols-1)` (state-major, descending)
- each digit is in base `symbols * states * 2`
- digit decodes as:
  - `move = digit % 2` (`0` = Left, `1` = Right)
  - `write = (digit / 2) % symbols`
  - `next = (digit / (2*symbols)) + 1` in `1..states`

Notes:
- Moves are only Left/Right (no Stay) for rule codes.
- When the head moves left of the current tape, the runtime grows a blank cell on the left.
- For long histories, runtime keeps the notebook-equivalent least-significant input window that can affect a `max_steps_per_round` run.

### Generated strategies (NDJSON)

You can reference NDJSON strategy lists in `games.toml`:

```
[[strategy]]
id = "gen"
type = "generated"
source = "generated/fsm.ndjson"
limit = 1000
```

Each NDJSON line should be a serialized `StrategySpec`. Loaded strategy ids
are prefixed with `gen::`.

## Enumeration helpers

Enumerate FSMs and emit NDJSON (the `fsm` subcommand is currently the only variant of `nit games enumerate`):

```
nit games enumerate fsm --states 2..4 --out ./generated --canonical --limit 5000 \
    --input-mode opponent_last_action
```

Flags:

- `--states <range>` — state-count range (`2..4`) or a single value
- `--out <path>` — output directory or NDJSON file
- `--canonical` — deduplicate isomorphic FSMs via canonicalization
- `--limit <N>` — cap the number of emitted strategies
- `--input-mode <name>` — `opponent_last_action` (default), `self_last_action`, `joint_last_action`

## Strategy inspection/export

Inspect a strategy (JSON or pretty text):

```
nit games inspect --config games.toml --id <strategy_id> [--format json|pretty] [--out <path>]
```

Export a strategy graph (Graphviz DOT or JSON):

```
nit games graph --config games.toml --id <strategy_id> --out <path.{dot|json}>
```

You can also load strategies from a run summary:

```
nit games graph --run runs/games/<run>/run_summary.json --id <strategy_id> --out <path.{dot|json}>
```

Notes:
- FSM edges are labeled by numeric input symbols (0..alphabet-1).
- TM edges are labeled by write symbol (ap); transitions with `next=0` target `HALT`.

Append NDJSON strategies when running a tournament:

```
nit games run --config games.toml --strategies ./generated/fsm.ndjson
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
- `match_history_preview.ndjson` plus `match_history_preview.wl` while the live TUI run is active; previews store full outcomes, while the popup display caps to 500 rounds
- `config.toml` snapshot
- `analysis/` outputs

Legacy `run__*.json` summaries under `games-runs/` or `output/` are still readable.

### Migration notes (schema v2)
- `run_summary.json` now includes `run_dir` and expanded `paths` entries
  (`definitions`, `results`, `config`, `analysis_dir`).
- `run_summary.json` also records runtime accelerator usage (`runtime.backend`,
  `runtime.metal_matches`, `runtime.cpu_matches`, `runtime.metal_fallbacks`).
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

When `engine.fast_eval = true`, the CPU kernel uses an analytical evaluator if **both**
strategies are FSMs and `noise = 0`.
It performs cycle detection and sums rounds in `O(mu + lambda)` time.

Cycle metadata (transient length, cycle length, cooperation rates) can be emitted
into `history.ndjson` by setting `history.include_cycle_metadata = true`.

On macOS, if `engine.accelerator != "cpu"`, homogeneous no-noise batch paths for
FSM, CA, and one-sided TM families may also be offloaded to Metal. The Metal path
currently requires:
- no event/history logging for that execution path
- same-kind strategies with uniform per-kind parameters
- `complexity_cost.tm_step_cost = 0.0` for TM runs
