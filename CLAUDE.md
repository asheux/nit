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
| `nit-mcp` | MCP stdio JSON-RPC server (`nit-mcp-server` binary) — bridges spawned `codex` back into substrate tools (signals/claims/assumptions); spawned by `codex_runner` |
| `nit-syntax` | Syntax highlighting |
| `nit-utils` | Shared filesystem/hashing/path utilities |

## Key source files

- `crates/nit-tui/src/app/mod.rs` — main event loop, input handling, keybinding dispatch, genome retry logic (`GENOME_RETRY_LIMIT = 3`, `GENOME_RETRY_MIN_LINES = 120`)
- `crates/nit-tui/src/app/dispatch.rs` — agent prompt dispatch (Codex and Claude routing, queue management)
- `crates/nit-tui/src/app/chat_input.rs` — chat input command parsing (`@all`, `@swarm`, `@shadow`, `@new`, `@queue`)
- `crates/nit-core/src/agent_bus.rs` — `AgentBusEvent` enum and state application
- `crates/nit-core/src/state.rs` — `AppState`, `AgentsState`, `AgentLane`, `MissionRecord`, queue types, `AgentOpsTab` (8 UI tabs + 1 internal `Patch`)
- `crates/nit-core/src/genome_report.rs` — code-as-genome tier scoring, parsimony detector, soft-bottleneck lift
- `crates/nit-tui/src/swarm.rs` — swarm orchestrator (DAG planning/execution, gate bundles `rust-ci`/`node-ci`/`python-ci`/`go-ci`, custom gates)
- `crates/nit-tui/src/shadow.rs` — shadow agent pipeline (`propose-a` / `propose-b` → `judge` → `review` → main)
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
| `NIT_TUI_FPS` | `60` (16 ms) | Redraw cap for both the single-pane and multipane event loops. Clamped to `15..=120`; out-of-range values fall back to the default. The cap gates `terminal.draw` so a high-volume agent-bus burst can't repaint faster than the terminal compositor (input handling and bus event apply remain unthrottled). Resolved once at run start, not in the hot loop. |
| `NIT_ASCII_FALLBACK` | unset | Use ASCII glyphs instead of Unicode in the agent ops UI |
| `NIT_ROSTER_NO_TRUNCATE` | unset | Disable per-backend / per-mission / chat-pane breather row truncation. Set to `1`/`true` to inspect every clone in large swarms. |
| `NIT_SNAPSHOT_QUEUE` | `64` | Snapshot writer channel capacity |
| `NIT_SNAPSHOT_DEBUG` | unset | Enable verbose snapshot debug logging to stderr |
| `NIT_SNAPSHOT_CYCLE` | unset | Force a snapshot when an attractor cycle is detected |
| `NIT_GOL_STACK_MB` | `256` | Stack size (MB) for Game of Life worker threads |
| `NIT_GOL_IO_STACK_MB` | `256` | Stack size (MB) for snapshot-stress I/O threads (falls back to `NIT_GOL_STACK_MB`) |
| `NIT_MCP_TURN_TIMEOUT_SECS` | none | Hard timeout for an MCP turn (0 or unset = no limit) |
| `NIT_MCP_TURN_IDLE_TIMEOUT_SECS` | disabled | Idle timeout for an MCP turn (set to enable, e.g. `600`; 0 or unset = disabled) |
| `NIT_CLAUDE_TURN_IDLE_TIMEOUT_SECS` | `900` (15 min) | Idle-output reaper for Claude turns. Kills the subprocess when no stream-json line has been read for N seconds and tries to recover the final message from buffered stream-json so the swarm can still proceed. **Only fires on read-only / verifier-style turns** — any turn that invokes a write-capable tool (Write/Edit/MultiEdit/NotebookEdit) is exempted on the assumption that writers are productive. Set to `0` to disable. The runner also exits early as soon as a stream-json `result` event is observed (regardless of writer status), even before this timeout fires. |
| `NIT_SCOPE_WALK_TIMEOUT_MS` | `200` | Foreground deadline (ms) waited on the background scope walk before chat dispatch proceeds with empty `scope_files`. The walk extracts directory tokens from the operator prompt and lists source files for the planner; running it inline used to freeze the UI on big trees. The walker thread keeps running after timeout and is bounded by its own caps (depth 12, 100 files, no symlink follow, skips `target` / `node_modules` / `.*`). Set to `0` to skip the walk entirely (always returns empty). |

## Swarm size limits

- Static cap: `MAX_SWARM_SIZE = 256` (`crates/nit-tui/src/swarm/constants.rs`).
- Effective cap: clamped at runtime by the host's `RLIMIT_NOFILE`. Each
  in-flight Codex/Claude exec turn opens 4 fds, so the formula is
  `min(MAX_SWARM_SIZE, max(1, (fd_limit - 32) / 4))` — defined in
  `crates/nit-tui/src/swarm/limits.rs::compute_effective_max_swarm_size`.
- macOS default `ulimit -n 256` → effective ceiling **56 agents**. Bump
  with `ulimit -n 4096` and restart nit to lift it.
- Bulk template caps proposers at `BULK_PRACTICAL_MAX = 12` (per-dep
  budget collapses past that; see `docs/SWARM.md` "Aborting and limits").
- Soft advisories pushed to the mission console when triggered:
  - `agents >= LARGE_SWARM_WARN_THRESHOLD (64)` or the FD-bound 75% of
    the effective ceiling, whichever is smaller.
  - Lightweight planner (haiku / mini / nano / flash) with N > 20.
  - Operator-explicit clamp ("requested X, started Y").
  - Bulk template proposer cap.

## Agent commands (in Agent Chat)

- `@all <prompt>` — fan-out to multiple agents (Codex and Claude)
- `@swarm [all|N] [template=lab|parallel|bulk] [mission=general|research|computational-research] <prompt>` — orchestrated multi-agent workflow
- `@shadow <prompt>` — single-agent dispatch with hidden propose/judge/review support agents; auto-enables for heavy prompts when no prefix is present (see `docs/SHADOWS.md`)
- `@new <prompt>` — spawn fresh-context clone when agent is busy
- `@queue` / `@q` — legacy alias for queued prompt (now same as default queueing)
- `/abort` (or `@abort`) — abort the active swarm mission. `/abort all` cancels every running swarm + clears both runner queues. `/abort <agent-id>` is a surgical strike that kills one agent's in-flight + queued turns. See `docs/SWARM.md` "Aborting a swarm".

## Multipane mode

`nit multipane [--backend <model>] [--panes N] [--cwd PATH]` opens a grid
of N independent chat panes (default 8, range 1..=32; grid is
`ceil(sqrt(N))` columns × `ceil(N / cols)` rows), each a self-contained
chat session anchored at its own working directory. `--backend` is
optional: omit it for a per-pane roster picker, name a family
(`claude` / `codex`) to filter the per-pane roster, or name a specific
lane id to pre-pick every pane. Per-pane agent ids use the form
`<base>#mp-pane-NN` so they coexist with `#swarm-` and `#chat-clone-`
conventions. Editor / agent ops / visualizer panes are unavailable;
only chat dispatch is wired.

Chat-pane parity: every pane runs the canonical
`app::chat_input::submit_chat_input_and_dispatch` via a Lens-B
alias-and-restore wrapper in `multipane::dispatch::with_pane_aliased`,
so `@swarm` / `@shadow` / `@all` / `@new` / `@queue` / `@q` /
`/abort` / queueing / broadcast / swarm-followup re-activation all
work per pane. Operator prompts land in `state.agents.messages`
tagged with the pane's `mission_id`, and `agent_console_view::render_pane`
renders inline-breather + agent-table rows scoped to the pane.

Keymap: Tab / Shift+Tab cycle focus, mouse click focuses a pane
directly, `Ctrl+Q` quits cleanly, `F1` / `?` toggles the multipane
help overlay, `/abort` / Ctrl+C empty / Esc-Esc target the focused
pane only, `Ctrl+R` reverts the focused pane to its roster picker.
Per-pane sessions persist to
`<state_dir>/multipane/session-<workspace-hash>.json` on Ctrl+Q and
on focus change (debounced ≤ 1 write/sec); `chat_input` is capped at
4 KB; a "fresh" Ctrl+Q with no prior file drops the session instead
of writing an empty layout. See `docs/MULTIPANE.md` for the full
spec.

## Aborting in-flight work

Five triggers, all routed through `chat_input::handle_abort`. When an
operator triggers an abort, the swarm runtime moves the run to
`completed_runs` with `report_status = "ABORTED"`, drains queued turns,
and pushes a `SYSTEM_ALERT_KIND` message to the chat. The runner-side
`CancelTurn { agent_id }` command then sets the per-turn cancel
`AtomicBool`; the worker thread sees it within ~50ms and calls
`child.kill()` on the subprocess.

| Trigger | Scope | Where wired |
|---|---|---|
| `/abort` (or `@abort`) typed in chat | Current mission | `app/chat_input.rs::parse_abort_command` |
| `/abort all` | Every active swarm + runner queues | same |
| `/abort <agent-id>` | One agent only | same |
| Ctrl+C with empty chat input | Current mission | `app/agent_station.rs` (KeyCode::Char('c') + CONTROL, plus `\u{3}` raw ETX) |
| Esc-Esc within ~500ms | Current mission | `chat_input::record_chat_esc_press` (thread-local) |
| `x` in Missions tab | Highlighted mission | `app/agent_station.rs` |

Operator cancels ride the same `TurnFailed` event but use the
`OPERATOR_CANCEL_TURN_MESSAGE` sentinel (in `nit-core::agent_bus`) so the
bus handler routes them to the soft path: `AgentStatus::Idle` (not
Error), no alert/signal, Info-level diag, no LAB→WARN promotion. See
`docs/SWARM.md` for the operator-facing description.
