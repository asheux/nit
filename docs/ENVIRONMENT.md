# Environment Variables

The full list of environment variables nit reads, with their defaults. Most
are optional escape hatches or performance knobs; nit runs fine with none set.
nit reads each variable once at startup unless the table says it reads it on
each dispatch. Set them the usual way:

```bash
NIT_CLAUDE_POOL=1 NIT_TUI_FPS=30 nit --agents claude
```

Subsystem docs (`docs/SWARM.md`, `docs/MULTIPANE.md`, `docs/INTAKE.md`,
`docs/TERMINAL.md`, `docs/PERF.md`) mention their own variables in context.
This page is the complete list.

Contributor note: `CLAUDE.md` carries a condensed copy of this table as an
always-loaded quick reference. Update both when you add or change a variable.

## TUI rendering

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_TUI_FPS` | `60` (16 ms) | Redraw cap for the single-pane and multipane event loops. Clamped to `15..=120`; out-of-range values use the default. See `docs/PERF.md`. |
| `NIT_ASCII_FALLBACK` | unset | Use ASCII glyphs instead of Unicode in the Agent Ops UI. |

The cap only limits drawing, so an agent-bus burst cannot repaint faster than
the terminal. Input handling and bus-event apply are not throttled.

## Roster / swarm display

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_ROSTER_NO_TRUNCATE` | unset | Set to `1` or `true` to show every clone row in large swarms instead of truncating per backend, per mission, and in the chat-pane breather. See `docs/SWARM.md` "UI truncation for large swarms". |

## Claude runner + warm pool

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_CLAUDE_POOL` | `0` (off) | Set to `1`, `true`, `yes`, or `on` to keep a warm pool of long-lived `claude -p --input-format stream-json` workers. Only plain turns use the pool; see the note below. |
| `NIT_CLAUDE_POOL_SIZE` | `clamp(effective_max_swarm_size / 4, 2..=8)` | Worker cap for the warm pool. Only used when `NIT_CLAUDE_POOL=1`. With N multipane panes, set it to at least N. |
| `NIT_CLAUDE_TURN_IDLE_TIMEOUT_SECS` | `900` (15 min) | Kill a Claude turn when no stream-json line arrives for this many seconds, then recover the final message from the buffered output. Only fires on read-only turns. `0` disables it. |

- Pool: a plain turn has no resume, the default `--max-turns`, no custom `--effort`, and `persist_session=true`. Integrator turns (`INTEGRATOR_MAX_TURNS=500`), resumed sessions, and custom `--effort` turns skip the pool and cold-spawn. A worker that fails (broken pipe, stream-json `error`, non-zero exit, cancel, idle timeout, or hourly GC age) is replaced, not returned. With the pool off, nit cold-spawns every turn.
- Pool size: the default is 8 with the macOS default `ulimit -n 256` and 2 with `ulimit -n 64`. Each parked worker holds the same 4 file descriptors as an in-flight turn, so the pool size lowers the effective swarm ceiling.
- Idle timeout: a turn that calls a write-capable tool (Write, Edit, MultiEdit, NotebookEdit) is exempt. The runner also stops as soon as it sees a stream-json `result` event, whatever the turn did. Applies to cold-spawn and pool turns; a pool worker that idles out is replaced.

## Swarm planner / gates / prompt budgets

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_PLANNER_LEGACY` | unset | Truthy values (`1`, `true`, `yes`, `on`, any case) disable the plan validator and repair loop: the planner runs once and its plan is used as is. Read once at startup. See `docs/SWARM.md` "How Swarm Works". |
| `NIT_PROMPT_TIERS` | enabled | Role-specific prompt budget tiers. `0`, `false`, `no`, or `off` (any case) skips the truncation pass. Read once at startup. Per-mission override: `@swarm budget=ROLE:N`. See `docs/SWARM.md` "Prompt budget tiers". |
| `NIT_PROMPT_BUDGET_<ROLE>` | per-role default | Byte ceiling for one role's prompt budget. `<ROLE>` is `INTEGRATE`, `JUDGE`, `PROPOSE`, `REVIEW`, `TEST`, `RESEARCH`, or `DEFAULT`. Decimal bytes only. Needs `NIT_PROMPT_TIERS` enabled. |
| `NIT_SCOPE_WALK_TIMEOUT_MS` | `200` | How long chat dispatch waits for the background scope walk before it continues with empty `scope_files`. `0` skips the walk. |
| `NIT_STRICT_CHECKLIST` | unset | Set to `1` to enforce the file checklist on swarm integrate turns: a skipped file raises a Warning signal and re-dispatches. By default nit only logs an Info diag. |
| `NIT_NO_COMPILE_GATE` | unset | Skip the post-edit compile gate, the `cargo check` nit runs for each Rust crate an integrator touched. |

- Budgets: `NIT_PROMPT_BUDGET_INTEGRATE=600000` lifts the integrate ceiling to 600K bytes for the rest of the session. The `k` / `K` suffix only works in the `budget=ROLE:N` command token, not in env values.
- Scope walk: it pulls directory tokens from your prompt and lists source files for the planner. It keeps running after the timeout, stops at depth 12 or 100 files, does not follow symlinks, and skips `target`, `node_modules`, and dot-directories.
- Checklist: stub files and incomplete directory splits re-dispatch whether or not `NIT_STRICT_CHECKLIST` is set.
- Compile gate: already a no-op outside a Cargo workspace or when no `crates/*/` files were touched.

## Multiway search engine

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_MULTIWAY` | `0` (off) | Set to `1`, `true`, `yes`, or `on` to enable the multiway best-first search engine and the `@multiway <task>` chat command. Read once at startup. See `docs/MULTIWAY.md`. |
| `NIT_MULTIWAY_GATES` | unset (auto-detect) | Override the multiway value-gate commands as a `;`-separated list. Unset auto-detects the gate bundle per worktree; empty means no gates, genome-only valuation. Needs `NIT_MULTIWAY=1`. Read once at startup. |

- `@multiway` accepts `mood=` (`explore`, `balanced`, `exploit`) and `k=N` fork-width flags. The default `k` is 3, clamped by the effective swarm ceiling. A mission runs as a best-first DAG search over isolated git worktrees, persisted to `<state_dir>/multiway/<mission>.json`. `/abort` removes the worktrees and prunes `refs/nit-multiway/<mission>/*`. With the flag off, `@multiway ...` goes to normal chat as a literal prompt.
- The graph view (`@multiway-graph`, `Ctrl+Shift+M`) and the `mode=multiway` modifier on `@shadow`, `@swarm`, and `@all` use this same flag. There is no separate variable. v1 is Claude-only and single-pane.
- Gate commands are split on whitespace and run directly in the node's worktree with no shell. All must pass for a node to be viable. Auto-detect picks `rust-ci`, `node-ci`, `python-ci`, or `go-ci`. Point the gate at a runner that resolves deps in a clean tree, for example `NIT_MULTIWAY_GATES="uv run pytest -q"`. A missing gate program fails the node instead of aborting the search.

## Codex MCP turn timeouts

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_MCP_TURN_TIMEOUT_SECS` | none | Hard total timeout for an MCP turn. `0` or unset means no limit. When any in-flight turn exceeds it, nit restarts the MCP server and fails all in-flight turns. |
| `NIT_MCP_TURN_IDLE_TIMEOUT_SECS` | disabled | Idle timeout for an MCP turn, for example `600`. `0` or unset disables it. When a turn stops producing `codex/event` notifications for this long, nit restarts the MCP server and fails the in-flight turns. See `docs/SWARM.md` "Optional safety valve: idle timeouts". |

The idle timeout is off by default because cancelling a hung turn can force a
new session and break continuity.

## Intake preprocessor

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_INTAKE_DISABLED` | unset | `1` turns off the hidden Claude-class intake agent for the rest of the session. Read on every dispatch, so it works without a restart. Same as `intake_enabled = false` in `config.toml`. See `docs/INTAKE.md`. |

With intake off, prompts dispatch as-is with no file-checklist augmentation.

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
| `NIT_GOL_IO_STACK_MB` | `256` | Stack size (MB) for snapshot-stress I/O threads. Falls back to `NIT_GOL_STACK_MB`. |

## Games engine

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_GAMES_DISABLE_METAL` | unset | Force the games tournament kernel onto the CPU path and skip all Metal GPU work. For hosts where Metal devices initialise but reject compute submissions, such as GitHub Actions macOS VMs. See `docs/GAMES.md`. |

## Logging / version check

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_LOG_PATH` | `<state_dir>/logs/<hash>.log` | Override the log file path. |
| `NIT_NO_VERSION_CHECK` | unset | `1` silences the launch-time "newer release available" prompt. |

## Set by nit, not by you

nit sets these on the `nit-mcp-server` process it spawns. They are not user
knobs.

| Variable | Purpose |
|----------|---------|
| `NIT_MCP_BACKCHANNEL_SOCKET` | Path of the Unix socket the server uses to talk back to nit. |
| `NIT_MCP_AGENT_ID` | The id of the agent the server belongs to. |
| `NIT_MCP_BACKCHANNEL_PORT` | TCP port used instead of the socket on hosts without Unix sockets. |

## Related constants

These are compile-time constants, not environment variables, but they bound
what the variables above tune:

| Constant | Value | Location | Role |
|----------|-------|----------|------|
| `MAX_SWARM_SIZE` | `256` | `crates/nit-tui/src/swarm/constants.rs` | Hard upper bound on swarm agent count, before the FD clamp. See `docs/SWARM.md` "Static and effective ceilings". |
| `BULK_PRACTICAL_MAX` | `12` | `crates/nit-tui/src/swarm/` | Bulk-template proposer cap. The per-dep budget collapses past it. |
| `INTEGRATOR_MAX_TURNS` | `500` | `crates/nit-tui/src/claude_runner.rs` | `--max-turns` for single-writer integrator turns. Normal turns use `50`. |
