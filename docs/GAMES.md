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
complexity_cost.enabled = true | false
complexity_cost.tm_step_cost = 0.0
complexity_cost.fsm_state_cost = 0.0
complexity_cost.memory_n_cost = 0.0

[history]
enabled = true | false
include_cycle_metadata = true | false
  # when enabled, history.ndjson also includes per-match TM metrics (if applicable)
```

## Strategies (programs)

### FSM (Moore machine)

FSMs are deterministic Moore machines with a fixed output per state and transitions
based on an input symbol derived from the last round’s observation.

```
[[strategy]]
id = "my_fsm"
type = "fsm"
num_states = 4
start_state = 0
outputs = ["C","D","D","C"]    # length = num_states
input_mode = "opponent_last_action"  # default
transitions = [
  # Each row: state index; then next_state for each input symbol
  # For opponent_last_action => alphabet size = 2
  [0, 1, 2],
  [1, 1, 3],
  [2, 0, 2],
  [3, 0, 1],
]
```

Input modes:
- `opponent_last_action` (alphabet size 2)
- `self_last_action` (alphabet size 2)
- `joint_last_action` (alphabet size 4: CC, CD, DC, DD)

Validation rules:
- `outputs.len == num_states`
- `transitions.len == num_states`
- each transition row is either `alphabet` entries or `alphabet+1` entries
  (leading state index)
- next states must be in `0..num_states`

### One-sided Turing machine

One-sided TMs are deterministic, bounded-step programs with a right-rail output.

Semantics:
- Tape indices are `0..∞` with a left boundary at `0`.
- Tape grows by appending one **history symbol** after each round.
- If a transition requests `L` while the head is at `0`, the head **clamps** to `0`
  (no left extension); the write still applies.
- At the start of each round:
  - head is positioned at the rightmost tape cell (most recent history symbol)
  - internal state resets to `start_state`
- The TM steps up to `max_steps_per_round`.
- **Rail output:** if a transition would move `R` when the head is already at the
  rightmost index, the action is produced immediately using `write` (mapped
  through `output_map`) and the round ends. The current cell is written **before**
  the rail output triggers. The tape is **not** extended to the right; the rail
  cell is conceptual.
- If no output occurs within `max_steps_per_round`, or a transition is invalid
  or halts before the rail (e.g., `next=0` without a rail move), the fallback
  action is used.
  By default this is `output_map[blank]`, but you can override it with
  `fallback_symbol`.

History symbol encoding uses `input_mode`:
- `opponent_last_action`, `self_last_action`, or `joint_last_action`

#### Explicit transition table

```
[[strategy]]
id = "tm1"
type = "one_sided_tm"
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
  [ [0, 0, "S"], [3, 1, "S"] ], # next=0 => HALT (fallback)
]
```

`next = 0` indicates HALT (fallback output).

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
type = "one_sided_tm"
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
- `next = 0` halting is only available in explicit transition tables.
- **Bounded tape window:** with `max_steps_per_round = M`, the engine retains only
  the last `M+1` tape symbols (the maximum span reachable in a round). This keeps
  TM memory usage `O(M)` rather than growing with rounds.

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

Enumerate FSMs and emit NDJSON:

```
nit games enumerate fsm --states 2..4 --out ./generated --canonical --limit 5000
```

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
- Memory-n graphs show full-history states (n rounds); initial action is used until history is filled.

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

Note: one-sided TMs are deterministic but **not** currently fast-evaluated; they
run via the standard simulator.
