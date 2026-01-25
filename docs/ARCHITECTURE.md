# Architecture

## Overview

nit is a terminal-first editor composed of five crates:

- `nit-core`: state, actions, text buffers, and IO (no terminal dependencies).
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
- Metrics: last render time, frame count, last action
- Optional prompt (e.g., confirm quit)

## Layout (nit-tui)

- Top bar with title, path, mode, encoding, ln/col.
- Main grid: left (Notes + Job Output), center (Editor), right (Visualizer + Gate Monitor).
- Bottom bar with key hints; overlay for help and prompts.

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
- Visualizer state (rule, generation, alive count, period, mode) is rendered in the pane
  and summarized in Gate Monitor.

**Snapshots**
- Stored under `gol-snapshots/` in the workspace root as RLE + JSON metadata.
- Deduped by grid hash and pruned by max file count.
