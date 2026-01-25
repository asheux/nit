# Architecture

## Overview

nit is a terminal-first editor composed of three crates:

- `nit-core`: state, actions, text buffers, and IO (no terminal dependencies).
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
- Visualizer seed + variant
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

