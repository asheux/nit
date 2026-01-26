# nit — Neural Interface Terminal

A terminal-first, multi-pane TUI editor inspired by _Devs_. Built in Rust with a secure-by-default posture and responsive, event-driven rendering.

## Quick start

```bash
cd nit
cargo run -- path/to/file
```

- `nit <file>` opens the file in the editor.
- `nit <dir>` sets the workspace root (opens an untitled buffer).
- `nit` defaults to the current directory and an untitled buffer.



## Development

```bash
just fmt
just clippy
just test
just run -- path/to/file
```

### Toolchain

- Rust stable (pinned via `rust-toolchain.toml`)
- ratatui + crossterm for UI/input
- ropey, unicode-segmentation, unicode-width for text correctness

## Security Notes

- No network or shell execution.
- Atomic file writes.
- Terminal restored on exit and panic.
For details see `SECURITY.md`.

## Documentation

- `docs/ARCHITECTURE.md` — state model, rendering pipeline.
- `docs/KEYBINDINGS.md` — full keymap.

## Visualizer quick notes

- Visualizer pane shows the encoded seed (genome) only; simulation runs in a popup.
- Run Petri Dish popup: `Ctrl+Enter`
- Show hidden Petri Dish: `Ctrl+^`
- Command prompt (Normal mode): `:gol run` / `:run gol` / `:life run`
- Seed controls (Visualizer focus):
  - Cycle encoder: `Ctrl+E`
  - Cycle seed preview: `Ctrl+R` (grid/matrix/motif)
  - Toggle seed source (Editor/Notes): `Ctrl+Y`
  - Toggle seed search: `Ctrl+G`
  - Apply seed proposal: `Ctrl+A`
  - Snapshot seed: `Ctrl+N`
- Petri Dish popup controls:
  - `Esc` close, `Space` pause, `Enter` step
  - `+/-` speed, `S` snapshot sim, `Ctrl+R` reseed from current code, `H` hide popup
  - `T` wrap mode, `O` auto-stop, `G` rule search, `A` apply best rule
  - Command prompt: `:gol hide` / `:gol show` to toggle visibility while sim runs
- Snapshots land in `gol-snapshots/`:
  - Seed snapshots: `seed__<timestamp>__enc-<id>__seedhash-<hash>.json` (+ `.rle`)
  - Sim snapshots: `sim__<timestamp>__rule-B3S23__gen-00145__hash-<hash>.rle` (+ `.json`)
  - `rules.ndjson` append-only best-rule log
- Snapshotting is async, bounded, and deduped to avoid repeat storms.
- Search intensity and limits are controlled in settings (defaults in `crates/nit-core/src/config.rs`).

## Known limitations (MVP)

- Horizontal scrolling uses character columns; tabs before the viewport can shift alignment.
- Syntax highlighting falls back to plain text for unsupported or very large files.

## License

MIT © 2026 nit contributors
