# Architecture

## Overview

nit is a terminal-first editor composed of six crates:

- `nit-core`: state, actions, text buffers, and IO (no terminal dependencies).
- `nit-games`: games-between-programs engine and tournament logic.
- `nit-gol`: Conway’s Game of Life engine, rule evaluation, and snapshot encoding.
- `nit-syntax`: syntax highlighting engine and language registry (tree-sitter + fallback).
- `nit-tui`: rendering, layout, event loop, and key mapping using ratatui + crossterm.
- `nit`: binary entrypoint wiring CLI args, tracing, and running the TUI.

## Data Flow

```
crossterm events -> keymap -> Action -> nit-core::apply_action(state, action)
                               |                     |
                               +---- effect (save, reseed, etc.)
state -> render -> ratatui widgets -> terminal diff
```

The app redraws only when state changes or the terminal resizes.

## State Model (nit-core)

- Workspace root (PathBuf)
- Buffers (main editor + notes) stored in rope-backed `Buffer`
- Mode (Insert/Normal)
- Focused pane
- Logs ring buffer and job progress/paused flag
- Visualizer state (seed, rule, mode, pause, wrap, generation, period, leaderboard)
- App kind (GoL or Games) plus app-specific runtime state
- Metrics: last render time, frame count, last action
- Optional prompt (e.g., confirm quit)

## Text Encoding (Editor + Notes)

Both the editor and notes buffers are **UTF-8 only**:

- Files are loaded with `read_to_string` (UTF-8 decode) and stored in `String`/`ropey::Rope`.
- Saves write `String` bytes back out as UTF-8.

**Why:** the terminal, `ropey`, and our cursor/selection logic all operate on Unicode text
with UTF-8 indexing. Supporting multiple encodings would add detection/normalization
complexity, ambiguity, and error cases without clear benefit for this UI. UTF‑8 keeps
rendering and text‑measurement consistent, and avoids lossy conversions.

## Layout (nit-tui)

- Top bar with title, path, mode, encoding, ln/col.
- Main grid: left (Notes + Job Output), center (Editor), right (Visualizer + Gate Monitor).
- Bottom bar with key hints; overlay for help and prompts.

## Lab Dispatch (Active Lab)

- The CLI supports `nit` (default GoL), `nit gol`, `nit games`, and `nit --lab <gol|games>`.
- `LabId`/`AppKind` in `AppState` selects the active lab and gates commands/keybindings.
- The TUI instantiates lab-specific runtimes:
  - GoL: seed runtime + GoL Petri Dish + GoL visualizer widget.
  - Games: Games Petri Dish + Games visualizer dashboard widget + run/replay tooling.
- Unnamespaced commands (`:run`, `:hide`, etc.) route to the active lab.
  Namespaced commands are accepted **only** for the active lab to avoid cross‑lab conflicts.

## Games Config (Payoff Matrix)

- Games configs support a payoff matrix under `[payoff]`.
- `matrix` is a 2×2 grid where each cell is `[A_payoff, B_payoff]`.
- When `matrix` is present, it is the source of truth; `R/S/T/P` must match it.

## Games Output Logs

- Runs are stored under `runs/games/<timestamp>__seed-<seed>/` with:
  - `run_summary.json` (schema v2) with config + results + paths
  - `definitions.json` and `results.json`
  - `events.ndjson` and `history.ndjson` when enabled
  - `config.toml` snapshot + `analysis/` outputs
- History logs are per‑match outcome strings when enabled.
  Outcomes are encoded as digits from player A’s perspective:
  `0=CC`, `1=CD`, `2=DC`, `3=DD`.
- Analysis outputs (`analysis__*.json`, `analysis_matches__*.{csv,ndjson}`,
  `analysis_strategies__*.csv`, `analysis_trajectories__*.csv`) are generated
  via `:games analyze` and summarize per‑match, steady‑state, and trajectory stats.

## Games Engine (Phase 2)

See `docs/GAMES.md` for the engine split (kernel vs stepper), deterministic seeding,
and parallel logging behavior.

## Rendering Discipline

- Event-driven; no busy loop. Redraw when:
  - input/action changes state
  - tick for job/progress or visualizer animations
  - terminal resize
- ratatui diff minimizes terminal updates; cursor shown only in editable panes.

## Saving

Atomic save in `io.rs`:
1. Write to `.<name>.nit.tmp` in the target directory.
2. Flush and optionally sync.
3. Rename over the destination.

## Error Handling

- All crates forbid unsafe code.
- Terminal restoration uses guard structs and panic hooks to exit raw/alt screen cleanly.

## Syntax Highlighting

nit uses a dedicated crate (`nit-syntax`) to provide fast, incremental, tree-sitter‑based
highlighting with a plain‑text fallback. The pipeline is intentionally split so future
semantic tokens (LSP) can layer on top of syntactic tokens without rewriting UI code.

**Pipeline**
- Buffer edits in `nit-core` record byte/point edits and bump the buffer version.
- The TUI collects edits, debounces updates, and schedules background highlight jobs.
- `nit-syntax` runs tree-sitter parsing and highlight queries off the UI thread.
- Results are versioned; stale highlights are discarded.
- Rendering layers: base style → syntax spans → selection → cursor-line background.

**Fallbacks**
- If highlighting is disabled or file size exceeds `highlight.max_file_bytes`, the
  engine switches to a plain-text snapshot (no spans) and reports status in Gate Monitor.

**Config knobs**
- `highlight.enabled`, `highlight.engine`, `highlight.debounce_ms`
- `highlight.max_file_bytes`, `highlight.max_spans_per_line`
- `editor.tab_width`

**Extensibility**
- Language detection is centralized in a registry (extension, filename, shebang).
- Queries live in `crates/nit-syntax/queries` and can be swapped without touching TUI code.

## Visualizer (Game of Life)

The Visualizer pane runs a Conway’s Game of Life simulation seeded from visible editor/notes
text. The TUI drives a lightweight tick loop for simulation, while heavier work (rule search
and snapshot I/O) runs in a background worker thread.

**Pipeline**
- Seed text (viewport) → ASCII-to-grid mapping → GoL simulation (nit-gol).
- Rule search evaluates Life-like rules asynchronously and reports a leaderboard.
- Visualizer state (rule, generation, alive count, attractor, auto-stop policy, mode) is rendered
  in the pane and summarized in Gate Monitor.
- The simulation can auto-pause on fixed points or repeats based on the auto-stop policy.

**Rule Model**
- The simulation always runs **one active rule at a time**.
- Default is **B3/S23 (Conway’s Life)** for familiarity and stable baseline behavior.
- Search mode evaluates many rules in the background, but `Apply` swaps in a single rule
  so the live grid remains deterministic and the step function stays simple and fast.

**Snapshots**
- Stored under `gol-snapshots/` in the workspace root as RLE + JSON metadata.
- Deduped by grid hash and pruned by max file count.
