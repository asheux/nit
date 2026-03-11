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

- Rust 1.88.0 (pinned via `rust-toolchain.toml`; CI also tests `stable`)
- ratatui + crossterm for UI/input
- ropey, unicode-segmentation, unicode-width for text correctness

### Reproducibility

- `Cargo.lock` is checked in; CI uses `--locked`.
- `time` is patched to a vendored copy at `vendor/time` (see `Cargo.toml`).

## Security Notes

- No plugins.
- No network calls from `nit` itself.
- No arbitrary command execution; `nit` may invoke `git` and the local `codex` CLI directly (no shell).
- Atomic file writes.
- Terminal restored on exit and panic.
For details see `SECURITY.md`.

## Agent Station

nit includes an Agent Ops / Agent Chat UI.

- Default: seeds all available lanes (`local`, Codex cache models, and `claude` when detected).
- `nit --agents local` (alias: `mock`) — force local lane only.
- `nit --agents codex` — force Codex only (loads a model roster from `~/.codex/models_cache.json`).
- `nit --agents claude` — force Claude lane only.
- `nit --agents all` — include all available lanes.
  - Default runtime: `--codex-runtime mcp` (runs a persistent `codex mcp-server`).
  - Exec runtime: `--codex-runtime exec` (spawns `codex exec` per turn).
  - Parallelism: `--codex-max-parallel-turns <N>` (default `2`).
  - Optional safety knobs:
    - `--codex-sandbox <read-only|workspace-write|danger-full-access>` (default: Codex config)
    - `--codex-approval-policy <untrusted|on-failure|on-request|never>` (default: `never`)
  - In Agent Chat:
    - `@all <prompt>` broadcasts to multiple Codex agents (fan-out).
    - `@swarm [all|N] [template=lab|parallel|bulk] <prompt>` runs an orchestrated multi-agent workflow (plan → DAG tasks → verify → synthesis). (`lab` is the default.)

Examples:

```bash
# Load all available lanes (default)
nit

# Force Codex agent station
nit --agents codex

# Force Claude-only agent station
nit --agents claude

# Force Codex agent station, per-turn `codex exec`
nit --agents codex --codex-runtime exec

# Force local-only agent station
nit --agents local

# From source
cargo run -p nit -- --agents codex
```

## Documentation

- `docs/ARCHITECTURE.md` — state model, rendering pipeline.
- `docs/KEYBINDINGS.md` — full keymap.
- `docs/SMOKE_TEST.md` — feature tour + quick manual test checklist.

## Command prompt (`:`)

Open the command prompt with `:` in Normal mode (or press `F1`/`?` for the full help overlay). Commands are routed to the active lab; start with `--lab gol|games` to switch.

- `:q` — quit (confirm if dirty)
- `:help` / `:commands` — open the help overlay
- `:run` — run the active app (GoL Petri Dish or Games tournament)

GoL (aka `:life`):
- `:gol run` / `:gol hide` / `:gol show` / `:gol stop`
- `:petri hide` / `:petri show` (aliases for `:gol hide|show`)
- `:gol rule [id|B/S]` — show/set rule (logs built-ins); example: `:gol rule conway` / `:gol rule B36/S23`
- `:gol rules` — list rules (logs)
- `:gol seed` / `:gol encoder` (aliases: `:seed view` / `:seed encoder`) — cycle seed view / encoder

Games:
- `:games run` / `:games hide` / `:games show` / `:games stop`
- `:games status` — show tournament status
- `:games export` — re-emit last run summary (if present)
- `:games runs` — browse saved runs (aliases: `:games browse` / `:games browser`)
- `:games replay` — open match replay selector (requires loaded run)
- `:games strategy [run]` — open strategy inspector for loaded run
- `:games strategies all|config` — open strategy inspector from config
- `:games inspect <strategy_id>` — introspect a strategy by id
- `:games inspect <strategy_id> {rule,states,symbols}` — inspect a one-sided TM rule tuple (override)
- `:games inspect {rule,states,symbols}` — inspect a one-sided TM rule tuple (no config/run)
- `:games tm [run|config] <input> [steps] [strategy_id]` — simulate one-sided TM
- `:games tm {rule,states,symbols} <input> [steps]` — simulate a rule-code TM without config
- `:games ca [run|config] <input> [steps] [strategy_id]` — simulate shrinking CA
- `:games ca {n,k,r} <input> [steps]` — simulate a CA rule tuple (uses default `t=10`)
- `:games analyze|analyse [path] [tail=N] [samples=N]` — analyze last/specified history log (accepts `tail_rounds=`/`trajectory_samples=` and `path=...`)

## Visualizer quick notes

- Visualizer pane defaults to GENOME (raw encoding); PLATE shows the sim seed grid.
- Run Petri Dish popup: `Ctrl+Enter`
- Show hidden Petri Dish: `Ctrl+^` (or `Ctrl+6`)
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
- Hide/show: `H` in popup to hide, `Ctrl+^` (or `Ctrl+6`) to show.
- Inspector: `Tab` toggles tournament vs match inspector; `←/→` changes the window size.
- Outputs: summaries, event logs, and optional history logs land in `games-runs/` under the workspace root.
  Summary JSON includes `run_id`, `config_text`, and `paths` for summary/events/history.

### Games config (payoff)

You can define payoffs either with `R/S/T/P` or a full matrix. Matrix form is the
source of truth if provided. `R/S/T/P` are validated when the matrix is symmetric.

```toml
[payoff]
R = -1
S = -3
T = 0
P = -2
matrix = [
  [[-1,-1],[-3,0]],
  [[0,-3],[-2,-2]],
]
```

Matrix layout (rows = player A, cols = player B):
- `matrix[0][0] = [A,B]` (C,C)
- `matrix[0][1] = [A,B]` (C,D)
- `matrix[1][0] = [A,B]` (D,C)
- `matrix[1][1] = [A,B]` (D,D)

### Games config (history)

Enable per-match outcome history logging (NDJSON) for later graphing:

```toml
save_data = true

[history]
enabled = true
```

Each history line encodes the match outcomes as digits from player A’s perspective:
`0=CC`, `1=CD`, `2=DC`, `3=DD`.

Set `save_data = false` to keep the run in-memory only and skip writing the run directory,
summary, results, config snapshot, and logs.

### Games config (scoring)

Choose how leaderboard scores are aggregated:

```toml
[engine]
accelerator = "auto"      # auto|cpu|metal
score_aggregation = "mean" # Code-02 semantics: per-round average score; TotalPayoff sums matchup means
```

## Known limitations (MVP)

- Horizontal scrolling uses character columns; tabs before the viewport can shift alignment.
- Syntax highlighting falls back to plain text for unsupported or very large files.

## License

MIT © 2026 nit contributors
