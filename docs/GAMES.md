# Games Engine

The games engine runs iterated-game tournaments between strategy programs:
finite state machines (FSMs), one-sided Turing machines (TMs), and cellular
automata (CAs). This doc covers the engine modes, seeding, config keys,
strategy formats, the headless CLI, the run layout, and history analysis. It
is for people who write `games.toml` configs or run tournaments from the
command line. TUI keys and `:games` commands are in `docs/KEYBINDINGS.md`.

## Engine modes

- Interactive stepper (`TournamentRunner`): steps round by round so you can inspect it. The TUI runs it in a background worker thread.
- Batch kernel (`TournamentKernel`): a tight loop for headless runs. The CLI uses it.

## Deterministic seeding

All randomness comes from a single run seed. Per-match RNGs derive from it, so
results do not depend on match order or thread scheduling.

```text
base_strategy_seed = hash(run_seed, role, strategy_id)
match_strategy_seed = hash(base_strategy_seed, match_id, repetition)
noise_seed = hash(hash(run_seed, "noise"), match_id, repetition)
```

This applies to noise flips (`noise` in the config).

## Parallel logging

With parallel execution, channel-backed writers write the events and history,
so NDJSON line order is not deterministic. Each record carries `match_id`
(0-based stable id) and `match_index` (1-based display index). Use them to
rebuild the order.

## Config keys

```text
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

- `save_data = false` turns off all persisted run artifacts for normal tournament runs.
- `score_aggregation` picks the per-strategy score that is shown and aggregated. `mean` (default) is the average payoff per round. `total` is the cumulative payoff across all matches.
- `accelerator` controls optional GPU offload on macOS. `auto` (default) uses Metal when a compatible homogeneous batch path allows it. `cpu` stays on CPU and Rayon. `metal` prefers Metal on those paths and falls back to the CPU when the run shape is unsupported.

Tournament scheduling matches the notebook: ordered pairs play in both
directions (`A vs B` and `B vs A`), `self_play` defaults to `true`, and in
`mean` mode the table shows `AggPayoff`, which follows Code-02 and sums
matchup means rather than raw cumulative payoff.

## Strategies (programs)

### FSM (Moore machine)

An FSM is a deterministic Moore machine. Each state has a fixed output, and
transitions depend on an input symbol derived from the last round.

```toml
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

Input mode: `opponent_last_action` only (notebook semantics).

Validation rules:

- `outputs.len == states`
- `transitions.len == states`
- each transition row has either `alphabet` entries or `alphabet+1` entries (a leading state index)
- next states must be valid for the indexing base: `input_index_base = 0` means `0..(num_states-1)`, and `input_index_base = 1` means `1..num_states`

### One-sided Turing machine

One-sided TMs are deterministic, bounded-step programs with notebook-compatible
history input.

Each round:

- an empty history returns `C` without running the TM
- `input = FromDigits[Flatten[history], 2]`, in global A,B order with no player swap
- the head starts on the least-significant base-`symbols` digit of that input
- nit runs `OneSidedTuringMachineFunction(tm, input, max_steps_per_round)`
- no output in time: action `D` (`halted=false`)
- output symbol `0`: action `C`; any non-zero output symbol: action `D` (`halted=true`)
- every round starts from the same TM definition; there is no persistent streaming tape

#### Explicit transition table

```toml
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

```toml
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

```toml
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

- iterate `(state=states..1, read=0..symbols-1)`, state-major and descending
- each digit is in base `symbols * states * 2`
- a digit decodes as `move = digit % 2` (`0` = Left, `1` = Right), `write = (digit / 2) % symbols`, and `next = (digit / (2*symbols)) + 1` in `1..states`

Notes:

- Rule codes only move Left or Right, never Stay.
- When the head moves left of the tape, the runtime grows a blank cell on the left.
- For long histories, the runtime keeps only the least-significant input window that can affect a `max_steps_per_round` run. This matches the notebook.

### Generated strategies (NDJSON)

Reference an NDJSON strategy list from `games.toml`:

```toml
[[strategy]]
id = "gen"
type = "generated"
source = "generated/fsm.ndjson"
limit = 1000
```

Each NDJSON line is a serialized `StrategySpec`. Loaded strategy ids get the
prefix `gen::`.

## Enumeration helpers

Enumerate FSMs and emit NDJSON. `fsm` is the only `nit games enumerate`
variant.

```bash
nit games enumerate fsm --states 2..4 --out ./generated --canonical --limit 5000 \
    --input-mode opponent_last_action
```

Flags:

- `--states <range>`: state-count range (`2..4`) or a single value
- `--out <path>`: output directory or NDJSON file
- `--canonical`: deduplicate isomorphic FSMs
- `--limit <N>`: cap the number of emitted strategies
- `--input-mode <name>`: `opponent_last_action` (default), `self_last_action`, `joint_last_action`

## Strategy inspection and export

Inspect a strategy as JSON or pretty text:

```bash
nit games inspect --config games.toml --id <strategy_id> [--format json|pretty] [--out <path>]
```

Export a strategy graph as Graphviz DOT or JSON:

```bash
nit games graph --config games.toml --id <strategy_id> --out <path.{dot|json}>
```

Or load the strategies from a run summary:

```bash
nit games graph --run runs/games/<run>/run_summary.json --id <strategy_id> --out <path.{dot|json}>
```

- FSM edges are labeled by numeric input symbol (0..alphabet-1).
- TM edges are labeled by write symbol (ap). Transitions with `next=0` target `HALT`.

Append NDJSON strategies to a tournament run:

```bash
nit games run --config games.toml --strategies ./generated/fsm.ndjson
```

## Headless CLI

Run without the TUI:

```bash
nit games run --config games.toml --out . --format pretty
```

Sweep a parameter grid:

```bash
nit games sweep --config games.toml --rounds 200,500 --noise 0.0,0.05 --repetitions 1,3
```

## Run layout

Runs live under `runs/games/<timestamp>__seed-<seed>/` and contain:

- `run_summary.json` (schema v2)
- `definitions.json` and `results.json`
- `events.ndjson` and `history.ndjson` (when enabled)
- `match_history_preview.ndjson` and `match_history_preview.wl` while a live TUI run is active. Previews store full outcomes; the popup display caps at 500 rounds.
- a `config.toml` snapshot
- `analysis/` outputs

`run_summary.json` includes `run_dir`, `paths` entries (`definitions`,
`results`, `config`, `analysis_dir`), and accelerator usage
(`runtime.backend`, `runtime.metal_matches`, `runtime.cpu_matches`,
`runtime.metal_fallbacks`). Older schema v1 summaries still load, with those
fields set to `null`. Legacy `run__*.json` summaries under `games-runs/` or
`output/` are still readable.

`history.ndjson` stores one outcome digit per round in `score_idx` (older
logs used the field name `outcomes`). Digits are from player A's view:
0=CC, 1=CD, 2=DC, 3=DD.

## History analysis

Analyze a history log (`history.ndjson`, or legacy `history__*.ndjson`) to
get:

- per-match summaries (overall and tail-window stats)
- per-strategy cooperation rates
- cooperation trajectories for random matchups

TUI command prompt:

```text
:games analyze [path] [tail=10000] [samples=50]
```

Outputs land next to the history log:

- `analysis__<stamp>__.json` (summary and strategy stats)
- `analysis_matches__<stamp>__.csv` and `analysis_matches__<stamp>__.ndjson`
- `analysis_strategies__<stamp>__.csv`
- `analysis_trajectories__<stamp>__.csv`

A matchup counts as random when a strategy id contains `rand` or `random`
(case-insensitive).

## Fast evaluator

With `engine.fast_eval = true`, the CPU kernel uses an analytical evaluator
when both strategies are FSMs and `noise = 0`. It detects the cycle and sums
the rounds in `O(mu + lambda)` time.

Set `history.include_cycle_metadata = true` to write cycle metadata (transient
length, cycle length, cooperation rates) into `history.ndjson`.

On macOS, when `engine.accelerator != "cpu"`, homogeneous no-noise batch paths
for the FSM, CA, and one-sided TM families may also run on Metal. The Metal
path requires:

- no event or history logging on that execution path
- same-kind strategies with uniform per-kind parameters
- `complexity_cost.tm_step_cost = 0.0` for TM runs

Set `NIT_GAMES_DISABLE_METAL=1` to force the CPU path. See
`docs/ENVIRONMENT.md`.
