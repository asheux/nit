# Environment Variables

This is the canonical reference for nit's runtime environment variables and the
tuning knobs they expose. Every variable is read from the process environment at
startup (or, where noted, on each dispatch), so you set them the usual way:

```bash
NIT_CLAUDE_POOL=1 NIT_TUI_FPS=30 nit --agents claude
```

Most variables are optional escape hatches or performance knobs; nit runs with
sensible defaults when none are set. Variables are grouped by the subsystem they
affect. Subsystem docs (`docs/SWARM.md`, `docs/MULTIPANE.md`, `docs/INTAKE.md`,
`docs/TERMINAL.md`, `docs/PERF.md`) keep their own contextual mentions; this page
is the full list.

> Contributor note: `CLAUDE.md` carries a condensed copy of the same table as an
> always-loaded quick reference. This page is the public, synced-to-website
> canonical version — keep the two in sync when adding a variable.

## TUI rendering

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_TUI_FPS` | `60` (16 ms) | Redraw cap for both the single-pane and multipane event loops. Clamped to `15..=120`; out-of-range values fall back to the default. The cap gates `terminal.draw` so a high-volume agent-bus burst can't repaint faster than the terminal compositor (input handling and bus-event apply remain unthrottled). Resolved once at run start, not in the hot loop. See `docs/PERF.md`. |
| `NIT_ASCII_FALLBACK` | unset | Use ASCII glyphs instead of Unicode in the agent ops UI. |

## Roster / swarm display

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_ROSTER_NO_TRUNCATE` | unset | Disable per-backend / per-mission / chat-pane breather row truncation. Set to `1`/`true` to inspect every clone in large swarms. See `docs/SWARM.md` "UI truncation for large swarms". |

## Claude runner + warm pool

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_CLAUDE_POOL` | `0` (cold-spawn) | Opt into the warm pool of long-lived `claude -p --input-format stream-json` workers (`crates/nit-tui/src/claude_pool.rs`). Set to `1` / `true` / `yes` / `on` to enable. When enabled, "vanilla" turns (no resume, default `--max-turns`, no custom `--effort`, `persist_session=true`) check a worker out of the pool, write a stream-json envelope to its long-lived stdin, and check the slot back in after the `result` event. Specialised turns (integrators with `INTEGRATOR_MAX_TURNS=500`, resumed sessions, custom `--effort`) bypass the pool and take the cold-spawn path. Unhealthy outcomes (BrokenPipe, stream-json `error`, non-zero exit, operator cancel, idle timeout, hourly GC age) replace the slot rather than returning it. The `=0` branch is byte-identical to the pre-pool runner and stays in code as the rollback path. |
| `NIT_CLAUDE_POOL_SIZE` | `default_claude_pool_size()` | Override the warm pool's worker cap (only meaningful when `NIT_CLAUDE_POOL=1`). Default is `clamp(effective_max_swarm_size / 4, 2..=8)` — macOS default `ulimit -n 256` lands on 8; tight `ulimit -n 64` drops to 2. Each parked slot permanently holds the same 4-fd footprint as an in-flight cold-spawn turn, so the effective swarm ceiling is reduced by the pool size. Multipane operators running N panes should set this to at least N. |
| `NIT_CLAUDE_TURN_IDLE_TIMEOUT_SECS` | `900` (15 min) | Idle-output reaper for Claude turns. Kills the subprocess when no stream-json line has been read for N seconds and tries to recover the final message from buffered stream-json so the swarm can still proceed. **Only fires on read-only / verifier-style turns** — any turn that invokes a write-capable tool (Write/Edit/MultiEdit/NotebookEdit) is exempted, on the assumption that writers are productive. Set to `0` to disable. The runner also exits early as soon as a stream-json `result` event is observed (regardless of writer status), even before this timeout fires. Applies to both the cold-spawn and warm-pool paths; on the pool path an idle-fired reap triggers `pool.recycle(IdleTimeout)` so the slot is replaced rather than parked with potentially-poisoned state. |

## Swarm planner / gates / prompt budgets

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_PLANNER_LEGACY` | unset | Disable the deterministic plan validator + repair loop (`crates/nit-tui/src/swarm/validator.rs` and `repair.rs`). Truthy values (`1` / `true` / `yes` / `on`, case-insensitive) revert the planner stage to its pre-validator behaviour: the planner LLM call runs once, the parsed plan goes straight to `finalize_plan`, no repair re-dispatch. Resolved once at `SwarmRuntime` construction and cached on `runtime.legacy_planner`, so a mid-mission env change can't flip behaviour halfway through a planning round. One-release rollback escape hatch. See `docs/SWARM.md` "How Swarm Works". |
| `NIT_PROMPT_TIERS` | enabled | Role-specific prompt budget tiers (`crates/nit-tui/src/swarm/budgets.rs`). When enabled, every dispatch runs through a three-stage truncation pass after `wrap_task_prompt` assembles the prompt and before it ships. Setting `0` / `false` / `no` / `off` (case-insensitive) short-circuits the pass to a no-op — byte-identical to the pre-tiers dispatch path. Resolved once at `SwarmRuntime` construction and snapshotted onto every `SwarmRun` at `start()`; mid-mission env flips cannot change behaviour between turns. Per-mission override: `@swarm budget=ROLE:N`. See `docs/SWARM.md` "Prompt budget tiers" for the role ceilings and truncation order. |
| `NIT_PROMPT_BUDGET_<ROLE>` | per-role default | Per-role byte ceiling override for the prompt budget tier — `<ROLE>` is one of `INTEGRATE`, `JUDGE`, `PROPOSE`, `REVIEW`, `TEST`, `RESEARCH`, `DEFAULT`. Decimal byte count only (no `k`/`K` suffix in env values; that suffix is only honoured by the per-mission `budget=ROLE:N` command token). Only meaningful when `NIT_PROMPT_TIERS` is enabled. Example: `NIT_PROMPT_BUDGET_INTEGRATE=600000` lifts the integrate ceiling to 600K bytes for the rest of the runtime's lifetime. |
| `NIT_SCOPE_WALK_TIMEOUT_MS` | `200` | Foreground deadline (ms) waited on the background scope walk before chat dispatch proceeds with empty `scope_files`. The walk extracts directory tokens from the operator prompt and lists source files for the planner; running it inline used to freeze the UI on big trees. The walker thread keeps running after timeout and is bounded by its own caps (depth 12, 100 files, no symlink follow, skips `target` / `node_modules` / `.*`). Set to `0` to skip the walk entirely (always returns empty). |
| `NIT_STRICT_CHECKLIST` | unset | Enforce strict file-checklist matching on swarm integrate turns. By default the structural-compliance check is **advisory** — when an integrator skips a checklist file nit logs an Info diag but does not re-dispatch. Setting it (`1`) restores the Warning substrate signal and the missing-files re-dispatch path (`crates/nit-tui/src/swarm/runtime_events.rs`). Stub files and incomplete directory splits re-dispatch regardless of this flag. |
| `NIT_NO_COMPILE_GATE` | unset | Disable the post-edit compile gate — the `cargo check` nit spawns for each Rust crate an integrator touched on `TurnCompleted` (`crates/nit-tui/src/app/compile_gate.rs`). Already a no-op when the workspace isn't a Cargo workspace or no `crates/*/` files were touched; set this to skip it unconditionally. |

## Codex MCP turn timeouts

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_MCP_TURN_TIMEOUT_SECS` | none | Hard total timeout for an MCP turn (0 or unset = no limit). If any in-flight turn exceeds it, nit restarts the MCP server and fails all in-flight turns. |
| `NIT_MCP_TURN_IDLE_TIMEOUT_SECS` | disabled | Idle timeout for an MCP turn (set to enable, e.g. `600`; 0 or unset = disabled). If an in-flight turn stops producing `codex/event` notifications for longer than this, nit restarts the MCP server and fails the in-flight turns. Disabled by default because cancelling hung turns can force a new session and affect continuity. See `docs/SWARM.md` "Optional safety valve: idle timeouts". |

## Intake preprocessor

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_INTAKE_DISABLED` | unset | Runtime kill switch for the hidden Claude-class intake agent. `1` disables intake for the rest of the session (read on every dispatch, so it flips without a restart). Equivalent to `intake_enabled = false` in `config.toml`. On disable, prompts dispatch as-is with no file-checklist augmentation. See `docs/INTAKE.md`. |

## Snapshots (Game of Life)

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_SNAPSHOT_QUEUE` | `64` | Snapshot writer channel capacity. |
| `NIT_SNAPSHOT_DEBUG` | unset | Enable verbose snapshot debug logging to stderr. |
| `NIT_SNAPSHOT_CYCLE` | unset | Force a snapshot when an attractor cycle is detected. |

## Game of Life worker stacks

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_GOL_STACK_MB` | `256` | Stack size (MB) for Game of Life worker threads. |
| `NIT_GOL_IO_STACK_MB` | `256` | Stack size (MB) for snapshot-stress I/O threads (falls back to `NIT_GOL_STACK_MB`). |

## Games engine

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_GAMES_DISABLE_METAL` | unset | Force the games tournament kernel onto the CPU fallback path, skipping all Metal GPU work (`crates/nit-games/src/tournament/kernel.rs`). Escape hatch for hosts where Metal devices initialise but reject compute submissions (e.g. GitHub Actions macOS VMs). |

## Logging / version check

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_LOG_PATH` | `<state_dir>/logs/<hash>.log` | Override the log file path. |
| `NIT_NO_VERSION_CHECK` | unset | Silence the launch-time "newer release available" prompt (`1` to mute). |

## Related constants

These are compile-time constants, not environment variables, but they bound the
behaviour the variables above tune:

| Constant | Value | Location | Role |
|----------|-------|----------|------|
| `MAX_SWARM_SIZE` | `256` | `crates/nit-tui/src/swarm/constants.rs` | Hard upper bound on swarm agent count, before the FD clamp. See `docs/SWARM.md` "Static and effective ceilings". |
| `BULK_PRACTICAL_MAX` | `12` | `crates/nit-tui/src/swarm/` | Bulk-template proposer cap (per-dep budget collapses past it). |
| `INTEGRATOR_MAX_TURNS` | `500` | `crates/nit-tui/src/claude_runner.rs` | `--max-turns` lifted for single-writer integrator turns (default turns use `50`). |
