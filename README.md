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

## Features (MVP)

- Rigid multi-pane layout: Notes, Job Output, Editor, Visualizer, Gate Monitor, and bottom status bar.
- Insert/edit text with ropey-based buffers; Notes is a separate editable scratch buffer.
- Pane focus cycling via Tab / Shift+Tab with focus highlighting.
- Visualizer runs Conway’s Game of Life seeded from editor/notes text, with optional rule search.
- Visualizer snapshots saved under `gol-snapshots/` (RLE + JSON metadata).
- Job output ring buffer fed by tracing logs; clear/pause controls.
- Gate Monitor dashboard with editor metrics (dirty flag, Ln/Col, bytes, render ms, focus, seed, etc.).
- Tree-sitter syntax highlighting with background parsing and plain-text fallback.
- Safe atomic saves; confirm prompt on quit when dirty.
- Help overlay (F1) and keyboard hints in the bottom bar.

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

## Known limitations (MVP)

- Horizontal scrolling uses character columns; tabs before the viewport can shift alignment.
- Syntax highlighting falls back to plain text for unsupported or very large files.

## License

MIT © 2026 nit contributors
