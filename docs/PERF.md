# Performance

## Benchmarks

Run the nit-games benches:

```bash
cargo bench -p nit-games
```

The benchmark suite includes:
- `single_match_200_rounds`
- `tournament_small` (16 strategies)
- `tournament_medium` (128 strategies)
- `parallel/tournament_parallel_large` (256 strategies)
- `logging_on` vs `logging_off`
- `fast_eval/deterministic_fast` vs `fast_eval/deterministic_slow`
- `fast_eval/mixed_random`
- `fsm_fast_eval/fsm_fast` vs `fsm_fast_eval/fsm_slow`
- `tm_micro_steps`
- `tm_tournament/tm` vs `tm_tournament/baseline`
- `tm_heavy/tm_steps_heavy` (max-step TM stress)
- `sweep_cell_io` (filesystem + serialization overhead)

## Flamegraphs

Install cargo-flamegraph once:

```bash
cargo install cargo-flamegraph
```

Generate a flamegraph for the nit-games bench:

```bash
cargo flamegraph -p nit-games --bench engine_bench -- benchmark=tournament_small
```

Useful flamegraph targets:

```bash
cargo flamegraph -p nit-games --bench engine_bench -- benchmark=tm_steps_heavy
cargo flamegraph -p nit-games --bench engine_bench -- benchmark=sweep_cell_io
```

## Sweep benchmarks

To benchmark sweep orchestration end-to-end:

```bash
cargo bench -p nit-games --bench engine_bench -- sweep_cell_io
```

## “Fast mode” knobs

For the fastest batch runs:
- `engine.mode = "batch"`
- `engine.parallelism = "auto"` (or `threads = N`)
- `engine.fast_eval = true` (eligible deterministic strategies, `noise = 0`)
- `event_log.enabled = false` (or `include_rounds = false`)
- `history.enabled = false`
- `engine.progress_interval_ms = 0` (no UI throttling)
