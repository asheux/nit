# CLAUDE.md

## Build & test

```bash
just ci          # fmt-check + clippy + test + cargo deny
just test        # cargo test --all
just clippy      # cargo clippy --all-targets --all-features -- -D warnings
just run -- <args>  # cargo run -- <args>
```

CI uses `--locked` — do not update `Cargo.lock` unless intentional.
MSRV: Rust 1.88.0 (pinned in `rust-toolchain.toml`).

## Workspace layout

| Crate | Purpose |
|-------|---------|
| `nit` | CLI binary entry point |
| `nit-core` | State (`AppState`), agent bus, config, buffer |
| `nit-tui` | TUI app loop, widgets, swarm orchestration, Claude/Codex runners, games UI |
| `nit-games` | Game theory tournament engine |
| `nit-gol` | Game of Life simulation |
| `nit-metal` | Metal GPU acceleration (macOS) |
| `nit-syntax` | Syntax highlighting |
| `nit-utils` | Shared filesystem/hashing/path utilities |

## Key source files

- `crates/nit-tui/src/app/mod.rs` — main event loop, input handling, keybinding dispatch
- `crates/nit-tui/src/app/dispatch.rs` — agent prompt dispatch (Codex and Claude routing, queue management)
- `crates/nit-tui/src/app/chat_input.rs` — chat input command parsing (`@all`, `@swarm`, `@new`, `@queue`)
- `crates/nit-core/src/agent_bus.rs` — `AgentBusEvent` enum and state application
- `crates/nit-core/src/state.rs` — `AppState`, `AgentsState`, `AgentLane`, `MissionRecord`, queue types
- `crates/nit-tui/src/swarm.rs` — swarm orchestrator (DAG planning/execution, gate bundles)
- `crates/nit-tui/src/codex_runner.rs` — Codex CLI integration (MCP server + exec runtime)
- `crates/nit-tui/src/claude_runner.rs` — Claude CLI subprocess integration (`claude -p`)
- `crates/nit-tui/src/widgets/` — all TUI widgets (agent_console_view, agent_ops_view, artifacts_popup, gate_monitor_view, etc.)

## Conventions

- No network calls from `nit` itself; external CLIs (`codex`, `claude`, `git`) are spawned directly (no shell).
- `time` crate is vendored at `vendor/time`.
- Clippy must pass with zero warnings (`-D warnings`).
- Tests: `cargo test --all` — ~526 tests across the workspace.
- Agent dispatch: Codex uses MCP or exec runtime; Claude spawns `claude -p` subprocesses.
- Queue management: `queue_len` on `AgentLane` tracks UI-visible queue depth; increment on enqueue, decrement on `TurnCompleted`/`TurnFailed`.

## Environment variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_LOG_PATH` | `<state_dir>/logs/<hash>.log` | Override the log file path |
| `NIT_ASCII_FALLBACK` | unset | Use ASCII glyphs instead of Unicode in the agent ops UI |
| `NIT_SNAPSHOT_QUEUE` | `64` | Snapshot writer channel capacity |
| `NIT_SNAPSHOT_DEBUG` | unset | Enable verbose snapshot debug logging to stderr |
| `NIT_SNAPSHOT_CYCLE` | unset | Force a snapshot when an attractor cycle is detected |
| `NIT_GOL_STACK_MB` | `256` | Stack size (MB) for Game of Life worker threads |
| `NIT_GOL_IO_STACK_MB` | `256` | Stack size (MB) for snapshot-stress I/O threads (falls back to `NIT_GOL_STACK_MB`) |
| `NIT_MCP_TURN_TIMEOUT_SECS` | none | Hard timeout for an MCP turn (0 or unset = no limit) |
| `NIT_MCP_TURN_IDLE_TIMEOUT_SECS` | disabled | Idle timeout for an MCP turn (set to enable, e.g. `600`; 0 or unset = disabled) |

## Agent commands (in Agent Chat)

- `@all <prompt>` — fan-out to multiple agents (Codex and Claude)
- `@swarm [all|N] [template=lab|parallel|bulk] [mission=general|research|computational-research] <prompt>` — orchestrated multi-agent workflow
- `@new <prompt>` — spawn fresh-context clone when agent is busy
- `@queue` / `@q` — legacy alias for queued prompt (now same as default queueing)
