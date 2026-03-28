# nit — Neural Interface Terminal

A terminal-first, multi-pane TUI editor built in Rust with a secure-by-default posture and responsive, event-driven rendering.

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
- No arbitrary command execution; `nit` may invoke `git`, `codex`, `claude`, and the platform URL launcher (`open`/`xdg-open`) directly (no shell). At startup, `gemini` is probed for model detection.
- `#![forbid(unsafe_code)]` across all crates except `nit-metal` (Metal GPU interop).
- Atomic file writes.
- Terminal restored on exit and panic.

For details see `docs/SECURITY.md`.

## Agent Station

nit includes an Agent Station UI (Agent Ops + Agent Chat) with multiple backends: Codex (MCP or exec), Claude (subprocess per turn), and a local mock lane. Gemini models are detected at startup but have no runtime runner yet.

- Default: seeds all available lanes (Codex, Claude, and Gemini models when detected on `PATH`).
- `nit --agents local` (alias: `mock`) — force local lane only.
- `nit --agents codex` — force Codex only (loads a model roster from `~/.codex/models_cache.json`).
- `nit --agents claude` — force Claude only (probes `claude models --json` for available models).
- `nit --agents all` — include all available lanes.
  - Default runtime: `--codex-runtime mcp` (runs a persistent `codex mcp-server`).
  - Exec runtime: `--codex-runtime exec` (spawns `codex exec` per turn).
  - Parallelism: `--codex-max-parallel-turns <N>` (default `2`).
  - Optional safety knobs:
    - `--codex-sandbox <read-only|workspace-write|danger-full-access>` (default: Codex config)
    - `--codex-approval-policy <untrusted|on-failure|on-request|never>` (default: `never`)
  - In Agent Chat:
    - `@all <prompt>` broadcasts to multiple agents — Codex and Claude (fan-out).
    - `@swarm [all|N] [template=lab|parallel|bulk] [mission=general|research|computational-research] <prompt>` runs an orchestrated multi-agent workflow (plan → DAG tasks → verify → synthesis). (`lab` is the default.)
    - `@new <prompt>` spawns a fresh-context clone when the agent is busy (queued turns continue on the original).
    - Prompt queuing: if an agent is busy, prompts are automatically queued and dispatched when the agent becomes idle.

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

- `docs/ARCHITECTURE.md` — state model, rendering pipeline, agent system, swarm orchestration.
- `docs/KEYBINDINGS.md` — full keymap.
- `docs/SMOKE_TEST.md` — feature tour + quick manual test checklist.
- `docs/SWARM.md` — swarm orchestration operator guide (templates, roles, DAG, gates).
- `docs/GAMES.md` — games engine details (strategies, config, headless CLI, analysis).
- `docs/PERF.md` — benchmarks and flamegraphs.
- `docs/RULES.md` — Game of Life rule catalog and contribution guide.
- `docs/SECURITY.md` — security policy, protections, and hardening backlog.

## Command prompt (`:`)

Open the command prompt with `:` in Normal mode (or press `F1`/`?` for the full help overlay). Commands are routed to the active lab; start with `--lab gol|games` to switch.

- `:q` — quit (confirm if dirty)
- `:help` / `:commands` — open the help overlay
- `:run` — run the active app (GoL Petri Dish or Games tournament)
- `:gol run|hide|show|stop|rule|rules` — GoL controls (aliases: `:petri`, `:life`)
- `:games run|hide|show|stop|status|runs|replay|inspect|tm|ca|analyze` — Games controls

Full command and keybinding reference: `docs/KEYBINDINGS.md`.

## GoL (Game of Life)

- Run Petri Dish: `Ctrl+Enter`; show hidden: `Ctrl+^`
- Petri Dish popup: `Space` pause, `Enter` step, `+/-` speed, `H` hide, `S` snapshot, `F2` rule picker, `P` protocol picker, `G` rule search, `A` apply best rule
- Visualizer seed controls: `Ctrl+E` encoder, `Ctrl+V` view, `Ctrl+R` cycle seed view, `Ctrl+M` plate render, `Ctrl+Y` seed source, `Ctrl+G` search, `Ctrl+A` apply, `Ctrl+N` snapshot
- Rule selection: built-in catalog (`crates/nit-gol/assets/rules.toml`), custom B/S input, user overlay (`~/.config/nit/rules.toml`). See `docs/RULES.md`.
- Snapshots land in `gol-snapshots/` (async, bounded, deduped).

## Games

- Launch: `nit games [path]` (opens `games.toml` by default).
- Run tournament: `Ctrl+Enter` or `:games run`; hide/show: `H` / `Ctrl+^`.
- Outputs land in `runs/games/` under the workspace root.

For strategy types (FSM, CA, one-sided TM), config format (payoff, history, scoring, engine), headless CLI, and analysis: see `docs/GAMES.md`.

## Known limitations (MVP)

- Horizontal scrolling uses character columns; tabs before the viewport can shift alignment.
- Syntax highlighting falls back to plain text for unsupported or very large files.

## License

MIT © 2026 nit contributors
