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

## Flamegraphs

Install cargo-flamegraph once:

```bash
cargo install cargo-flamegraph
```

Generate a flamegraph for the nit-games bench:

```bash
cargo flamegraph -p nit-games --bench engine_bench -- benchmark=tournament_small
```

## “Fast mode” knobs

For the fastest batch runs:
- `engine.mode = "batch"`
- `engine.parallelism = "auto"` (or `threads = N`)
- `engine.fast_eval = true` (eligible deterministic strategies, `noise = 0`)
- `event_log.enabled = false` (or `include_rounds = false`)
- `history.enabled = false`
- `engine.progress_interval_ms = 0` (no UI throttling)
