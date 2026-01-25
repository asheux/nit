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

- Toggle search mode: `Ctrl+G`
- Cycle auto-stop policy: `Ctrl+O` (Off → Fixed → Repeat)
- Toggle seed source (Editor/Notes): `Ctrl+Y`
- Force snapshot: `Ctrl+N`
- Snapshots land in `gol-snapshots/`:
  - `<timestamp>__rule-B3S23__gen-00042__hash-abcdef.rle`
  - matching `.json` with metadata
  - `rules.ndjson` append-only best-rule log
- Search intensity and limits are controlled in settings (defaults in `crates/nit-core/src/config.rs`).

## Known limitations (MVP)

- Horizontal scrolling uses character columns; tabs before the viewport can shift alignment.
- Syntax highlighting falls back to plain text for unsupported or very large files.

## License

MIT © 2026 nit contributors
