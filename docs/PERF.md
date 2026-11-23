# Performance

This doc covers two areas: the TUI render loop (frame budget, redraw cap,
and how output-heavy subsystems stay off the redraw path) and the
`nit-games` engine benchmarks (criterion benches and flamegraphs). The
environment variables named here are listed in `docs/ENVIRONMENT.md`.

## TUI render performance

### Interactive editor measurements

The following measurements compare debug builds of `69657d0` and `ab9ab11` on the same Mac. Each build ran in its own 160×48 tmux session. The station opened a tracked 68,011-byte UTF-8 file with 2,001 lines of text. Input latency covers sending keys through tmux until the updated line first appeared in a pane capture. Each median uses 12 edits. The benchmark saved the file and checked its contents.

| Measurement | Before | After |
| --- | ---: | ---: |
| One key to visible edit | 23.6 ms | 13.2 ms |
| Three keys to visible edit | 39.4 ms | 13.2 ms |
| Idle station, percent of one CPU core | 8.2% | 1.8% |
| Idle eight-pane local session, percent of one CPU core | 14.7% | 0.3% |

CPU figures use process CPU time over eight seconds for the station and six seconds for multipane, after allowing startup activity to settle. Terminal size, hardware, repository size and active agent output can change these results.

The station batches up to 16 ready input events per frame. In Changes, with the simulation idle, it refreshes periodic information four times per second. Input and terminal activity can request an earlier frame. Multipane also repaints for input, PTY output and active agent turns.

### Frame cap (`NIT_TUI_FPS`)

The single-pane and multipane event loops gate `terminal.draw` behind a
redraw cap, default 60 fps (16 ms). nit reads the cap once at startup and
clamps it to `15..=120`; out-of-range values fall back to the default. The
cap applies to the draw call only. Input handling and agent-bus event
application stay unthrottled, so a burst of bus events can update state on
every event but cannot repaint faster than the terminal compositor keeps
up. Lower it (`NIT_TUI_FPS=30`) on slow or remote terminals; raise it for
smoother scrolling on a fast local terminal.

### PTY and terminal render decoupling

The embedded terminal renders independently of shell output, so a chatty
process cannot storm the redraw loop. The PTY reader thread keeps the
`vt100` grid current on its own thread, and nit samples it on the normal
frame cadence. See "Notes" in `docs/TERMINAL.md` for the full model.

### Multipane render budget

Multipane runs the same per-pane render code once per ratatui frame. The
latency targets (initial 16-pane render under 50 ms, dir-search keystroke
to result update under 16 ms) are in the "Performance budget" section of
`docs/MULTIPANE.md`.

## Games benchmarks

```bash
cargo bench -p nit-games
```

The suite includes:

- `single_match_200_rounds`
- `tournament_small` (16 strategies, 200 rounds)
- `tournament_medium` (128 strategies, 50 rounds)
- `parallel/tournament_parallel_auto` (64 strategies, 50 rounds)
- `parallel/tournament_parallel_large` (256 strategies, 50 rounds)
- `logging_on` vs `logging_off`
- `fast_eval/deterministic_fast` vs `fast_eval/deterministic_slow` (5000 rounds)
- `fast_eval/mixed_fsm_ca_tm` (mixed strategy families, 500 rounds)
- `fsm_fast_eval/fsm_fast` vs `fsm_fast_eval/fsm_slow` (32 FSM strategies, 3000 rounds)
- `tm_micro_steps` (256 steps per iteration)
- `tm_tournament/tm` vs `tm_tournament/baseline` (12 strategies, 200 rounds)
- `tm_heavy/tm_steps_heavy` (8 TM strategies, 150 rounds, 512 max steps)
- `tm_family_halting/tm_1x2_rounds200_steps1000` (TM family halting filter)
- `sweep_cell_io` (filesystem and serialization overhead)

## Flamegraphs

Install cargo-flamegraph once:

```bash
cargo install cargo-flamegraph
```

Generate a flamegraph for the nit-games bench:

```bash
cargo flamegraph -p nit-games --bench engine_bench -- benchmark=tournament_small
```

Other useful targets:

```bash
cargo flamegraph -p nit-games --bench engine_bench -- benchmark=tm_steps_heavy
cargo flamegraph -p nit-games --bench engine_bench -- benchmark=sweep_cell_io
```

## Sweep benchmarks

To benchmark sweep orchestration end to end:

```bash
cargo bench -p nit-games --bench engine_bench -- sweep_cell_io
```

## Fast-mode knobs

For the fastest batch runs:

- `engine.mode = "batch"`
- `engine.parallelism = "auto"` (or `threads = N`)
- `engine.fast_eval = true` (eligible deterministic strategies, `noise = 0`)
- `event_log.enabled = false` (or `include_rounds = false`)
- `history.enabled = false`
- `engine.progress_interval_ms = 0` (no UI throttling)
