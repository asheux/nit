# nit — Neural Interface Terminal

A terminal-first, multi-pane TUI editor inspired by _Devs_. Built in Rust with a secure-by-default posture and responsive, event-driven rendering.

## Quick start

```bash
cd nit
cargo run -- path/to/file
cargo run -- games
```

- `nit <file>` opens the file in the editor.
- `nit <dir>` sets the workspace root (opens an untitled buffer).
- `nit` defaults to the current directory and an untitled buffer.
- `nit gol [path]` explicitly launches GoL mode.
- `nit games [path]` launches Games mode (games between programs).



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

- Visualizer pane defaults to GENOME (raw encoding); PLATE shows the sim seed grid.
- Run Petri Dish popup: `Ctrl+Enter`
- Show hidden Petri Dish: `Ctrl+^`
- Command prompt (Normal mode): `:gol run` / `:run gol` / `:life run`
- Seed controls (Visualizer focus):
  - Cycle encoder: `Ctrl+E`
  - Toggle view (GENOME ↔ PLATE): `Ctrl+V`
  - Cycle seed view (genome/plate/map/stats): `Ctrl+R`
  - Cycle plate render (solid/half/braille/tissue/heat): `Ctrl+M`
  - Cycle seed overlays: `Ctrl+Shift+V`
  - Toggle seed source (Editor/Notes): `Ctrl+Y`
  - Toggle seed search: `Ctrl+G`
  - Apply seed proposal: `Ctrl+A`
  - Snapshot seed: `Ctrl+N`
- Petri Dish popup controls:
  - `Esc` close, `Space` pause, `Enter` step
  - `+/-` speed, `S` snapshot sim, `Ctrl+R` reseed from current code, `H` hide popup
  - `T` wrap mode, `O` auto-stop, `G` rule search, `A` apply best rule
  - `F2` rule picker (built-ins + custom)
  - Command prompt: `:gol hide` / `:gol show` to toggle visibility while sim runs
- GoL rule selection:
  - Command: `:gol rule conway` or `:gol rule B36/S23`
  - Built-ins: curated catalog (see `crates/nit-gol/assets/rules.toml`) with classics, maze, no-death, texture, and literature rules
  - Custom rules: use any B/S string (e.g. `B2/S` or `B3678/S34678`)
  - User overlay: `~/.config/nit/rules.toml` (add new rules or override tags/aliases/description)
  - Config: `~/.config/nit/config.toml` → `[gol.rule] default = "conway"`, `workspace_override = true`
- Snapshots land in `gol-snapshots/`:
  - Seed snapshots: `seed__<timestamp>__enc-<id>__seedhash-<hash>.json` (+ `.rle`)
  - Sim snapshots: `sim__<timestamp>__rule-B3S23__gen-00145__hash-<hash>.rle` (+ `.json`)
  - `rules.ndjson` append-only best-rule log
- Snapshotting is async, bounded, and deduped to avoid repeat storms.
- Search intensity and limits are controlled in settings (defaults in `crates/nit-core/src/config.rs`).

## Games quick notes

- Launch: `nit games [path]` (opens `games.toml` by default).
- Run tournament: `Ctrl+Enter` or `:games run`.
- Hide/show: `H` in popup to hide, `Ctrl+^` to show.
- Outputs: summaries, event logs, and optional history logs land in `games-runs/` under the workspace root.

### Games config (payoff)

You can define payoffs either with `R/S/T/P` or a full matrix. Matrix form is the
source of truth if provided, and `R/S/T/P` must match it.

```toml
[payoff]
R = 3
S = 0
T = 5
P = 1
matrix = [
  [[3,3],[0,5]],
  [[5,0],[1,1]],
]
```

Matrix layout (rows = player A, cols = player B):
- `matrix[0][0] = [R,R]` (C,C)
- `matrix[0][1] = [S,T]` (C,D)
- `matrix[1][0] = [T,S]` (D,C)
- `matrix[1][1] = [P,P]` (D,D)

### Games config (history)

Enable per-match outcome history logging (NDJSON) for later graphing:

```toml
[history]
enabled = true
```

Each history line encodes the match outcomes as digits from player A’s perspective:
`0=CC`, `1=CD`, `2=DC`, `3=DD`.

## Known limitations (MVP)

- Horizontal scrolling uses character columns; tabs before the viewport can shift alignment.
- Syntax highlighting falls back to plain text for unsupported or very large files.

## License

MIT © 2026 nit contributors
