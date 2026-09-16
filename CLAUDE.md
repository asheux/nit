# CLAUDE.md

## Build and test

```bash
just ci             # fmt-check + clippy + test + cargo deny
just test           # cargo test --all
just clippy         # cargo clippy --all-targets --all-features -- -D warnings
just run -- <args>  # cargo run -- <args>
```

- CI uses `--locked`. Do not update `Cargo.lock` unless the change is intentional.
- MSRV: Rust 1.88.0, pinned in `rust-toolchain.toml`.
- Clippy must pass with zero warnings.
- Run `cargo test --all` for the live test count.

## Workspace layout

| Crate | Purpose |
|-------|---------|
| `nit` | CLI binary entry point |
| `nit-core` | State (`AppState`), agent bus, config, buffers, substrate, genome reports, seed encoders |
| `nit-tui` | TUI loop, widgets, swarm, shadows, intake, multipane, terminal, Codex and Claude runners, games UI |
| `nit-multiway` | Best-first search engine over git worktrees (opt-in, `NIT_MULTIWAY=1`) |
| `nit-mcp` | MCP stdio JSON-RPC server (`nit-mcp-server`) that exposes substrate tools to spawned `codex` |
| `nit-games` | Game theory tournament engine |
| `nit-gol` | Game of Life engine |
| `nit-metal` | Metal GPU acceleration (macOS) |
| `nit-syntax` | Tree-sitter highlighting (29 grammars; the language table is `nit-core::languages::LANGUAGES`) |
| `nit-utils` | Shared filesystem, hashing, and path helpers |

## Key source files

- `crates/nit-tui/src/app/mod.rs`: main event loop, input handling, key dispatch.
- `crates/nit-tui/src/app/genome_retry.rs`: genome retry prompt and `GENOME_RETRY_LIMIT = 3`. There is no file-size gate on retries; the encoder's under-20-significant-lines auto-pass and the parsimony detector cover trivial files.
- `crates/nit-tui/src/app/dispatch.rs`: agent prompt dispatch (Codex and Claude routing, queues).
- `crates/nit-tui/src/app/chat_input.rs`: chat command parsing (`@all`, `@swarm`, `@shadow`, `@multiway`, `@new`, `@queue`, `/abort`).
- `crates/nit-core/src/agent_bus/`: `AgentBusEvent` and how events apply to state.
- `crates/nit-core/src/state/`: `AppState`, `AgentsState`, `AgentLane`, `MissionRecord`, queue types, `AgentOpsTab` (8 UI tabs plus one internal `Patch` tab).
- `crates/nit-core/src/genome_report/`: genome tier scoring, parsimony detector, soft-bottleneck lift.
- `crates/nit-core/src/languages.rs`: the `LANGUAGES` table (extensions, filenames, shebangs, injection aliases, `is_code`). Used by `nit-syntax` detection, the file watcher, the swarm scope walk, and the seed encoders. To add a language: append a `LanguageInfo` here, add the grammar dep and arm in `nit-syntax/src/language/grammars.rs`, and add a `SeedLanguage::ts_language` arm in `nit-core/src/seed/encoders/lang.rs` if the encoders should score it.
- `crates/nit-tui/src/swarm/`: swarm orchestrator (DAG planning and execution, gate bundles `rust-ci` / `node-ci` / `python-ci` / `go-ci`, custom gates, validator, repair, budgets).
- `crates/nit-tui/src/shadow.rs`: shadow pipeline (`propose-a` / `propose-b`, then `judge`, then `review`, then the main agent).
- `crates/nit-tui/src/intake.rs`: hidden intake classifier (Claude lanes only).
- `crates/nit-tui/src/multiway/` and `crates/nit-multiway/`: multiway runtime and engine.
- `crates/nit-tui/src/codex_runner/`: Codex integration (MCP server and exec runtime).
- `crates/nit-tui/src/claude_runner.rs` and `claude_pool.rs`: Claude subprocess runner and warm pool.
- `crates/nit-tui/src/widgets/`: all TUI widgets.

## Conventions

- nit makes no network calls. It spawns `codex`, `claude`, and `git` directly, never through a shell.
- The `time` crate is vendored at `vendor/time`.
- Codex turns use the MCP or exec runtime; Claude turns spawn `claude -p`.
- `queue_len` on `AgentLane` is the UI-visible queue depth. Increment on enqueue, decrement on `TurnCompleted` or `TurnFailed`.
- Document every new environment variable in both this file and `docs/ENVIRONMENT.md`.

## Environment variables

`docs/ENVIRONMENT.md` is the full public reference (synced to the website). This table is
the quick version; keep the two in sync.

| Variable | Default | Purpose |
|----------|---------|---------|
| `NIT_LOG_PATH` | `<state_dir>/logs/<hash>.log` | Log file path. |
| `NIT_NO_VERSION_CHECK` | unset | `1` mutes the launch-time update prompt. |
| `NIT_TUI_FPS` | `60` | Redraw cap for both event loops, clamped to `15..=120`. Read once at start. |
| `NIT_ASCII_FALLBACK` | unset | ASCII glyphs instead of Unicode in the agent ops UI. |
| `NIT_ROSTER_NO_TRUNCATE` | unset | `1` shows every clone in the roster, missions, and chat breather. |
| `NIT_CLAUDE_POOL` | `0` | `1` enables the warm pool of long-lived `claude -p --input-format stream-json` workers. Only plain turns use it; integrators, resumed sessions, and custom `--effort` turns cold-spawn. The `0` path stays as the rollback. |
| `NIT_CLAUDE_POOL_SIZE` | `clamp(effective_max_swarm_size / 4, 2..=8)` | Pool worker cap. Each parked worker holds 4 fds, so the swarm ceiling drops by this much. Multipane users with N panes should set at least N. |
| `NIT_CLAUDE_TURN_IDLE_TIMEOUT_SECS` | `900` | Kills a Claude turn that has produced no stream-json for N seconds and recovers the last message. Only fires on turns that have not used a write tool. `0` disables. |
| `NIT_SCOPE_WALK_TIMEOUT_MS` | `200` | How long chat dispatch waits for the background scope walk before sending empty `scope_files`. `0` skips the walk. |
| `NIT_PLANNER_LEGACY` | unset | Truthy disables the plan validator and repair loop (`swarm/validator.rs`, `repair.rs`). Read once when `SwarmRuntime` is built. |
| `NIT_PROMPT_TIERS` | enabled | Role-based prompt byte ceilings with three-stage truncation (`swarm/budgets.rs`). `0` / `false` / `no` / `off` disables. Ceilings: integrate 480K, judge 320K, research 240K, propose 160K, review 120K, test 96K, default 96K. Per mission: `@swarm budget=ROLE:N` (`k` suffix allowed). |
| `NIT_PROMPT_BUDGET_<ROLE>` | per role | Byte ceiling override for `INTEGRATE`, `JUDGE`, `PROPOSE`, `REVIEW`, `TEST`, `RESEARCH`, or `DEFAULT`. Decimal bytes only. |
| `NIT_STRICT_CHECKLIST` | unset | `1` makes a missed checklist file on an integrate turn emit a Warning signal and re-dispatch, instead of an Info diag. |
| `NIT_NO_COMPILE_GATE` | unset | Skips the `cargo check` nit runs on each crate an integrator touched (`app/compile_gate.rs`). |
| `NIT_MULTIWAY` | `0` | `1` enables the multiway engine, `@multiway`, `mode=multiway`, the graph view, and the roster controls. Read once in `MultiwayRuntime::from_env`. Claude-only, single-pane. See `docs/MULTIWAY.md`. |
| `NIT_MULTIWAY_GATES` | unset | Unset: auto-detect the gate bundle per worktree. Set: `;`-separated commands that replace it (argv-split, no shell, all must pass). Empty: no gates, genome-only. |
| `NIT_MCP_TURN_TIMEOUT_SECS` | none | Hard timeout for a Codex MCP turn. `0` or unset means no limit. |
| `NIT_MCP_TURN_IDLE_TIMEOUT_SECS` | disabled | Idle timeout for a Codex MCP turn, for example `600`. |
| `NIT_INTAKE_DISABLED` | unset | `1` turns the intake agent off. Read on every dispatch. |
| `NIT_SNAPSHOT_QUEUE` | `64` | Snapshot writer channel capacity. |
| `NIT_SNAPSHOT_DEBUG` | unset | Verbose snapshot logging to stderr. |
| `NIT_SNAPSHOT_CYCLE` | unset | Force a snapshot when an attractor cycle is found. |
| `NIT_GOL_STACK_MB` | `256` | Stack size for Game of Life worker threads. |
| `NIT_GOL_IO_STACK_MB` | `256` | Stack size for snapshot-stress I/O threads. Falls back to `NIT_GOL_STACK_MB`. |
| `NIT_GAMES_DISABLE_METAL` | unset | Forces the games kernel onto the CPU path. |

## Swarm size limits

The user-facing ceiling table is in `docs/SWARM.md` under "Static and effective ceilings".

- Static cap: `MAX_SWARM_SIZE = 256` (`crates/nit-tui/src/swarm/constants.rs`).
- Effective cap: `min(MAX_SWARM_SIZE, max(1, (fd_limit - 32) / 4))`, from
  `compute_effective_max_swarm_size` in `crates/nit-tui/src/swarm/limits.rs`. Each
  in-flight turn opens 4 fds. The macOS default `ulimit -n 256` gives 56 agents; run
  `ulimit -n 4096` and restart to lift it.
- `NIT_CLAUDE_POOL=1` parks `default_claude_pool_size()` workers at 4 fds each, so the
  ceiling drops by the pool size (56 to 48 on the macOS default).
- Bulk proposers cap at `BULK_PRACTICAL_MAX = 12`.

## Agent chat commands

- `@all <prompt>`: same prompt to several agents.
- `@swarm [all|N] [template=lab|parallel|bulk] [mission=general|research|computational-research] <prompt>`: orchestrated mission. See `docs/SWARM.md`.
- `@shadow <prompt>`: one agent with hidden propose, judge, and review support. Auto-enables for heavy prompts. See `docs/SHADOWS.md`.
- `@multiway [mood=explore|balanced|exploit] [k=N] <prompt>`: best-first search over git worktrees. Needs `NIT_MULTIWAY=1`; otherwise it dispatches as plain chat. See `docs/MULTIWAY.md`.
- `mode=multiway`: modifier on `@shadow`, `@swarm`, `@all`, and bare chat that routes the command into the multiway engine (`@shadow` gives `k=2` balanced; `@swarm` maps `N` and `template` to `k` and `mood`; `@all` gives one wide expansion). Without the token, or with the flag off, the command is unchanged. Also needs `NIT_MULTIWAY=1`.
- `@multiway-graph`: render the current search DAG with Graphviz and open it. `Ctrl+Shift+M` or `@multiway-popup` toggles the live popup. Both need `NIT_MULTIWAY=1`.
- `@new <prompt>`: fresh-context clone when the agent is busy.
- `@queue` / `@q`: explicit queue (same as the default queueing).
- `/abort` (or `@abort`): abort the active swarm mission. `/abort all` cancels every mission and clears both runner queues. `/abort <agent-id>` kills one agent's in-flight and queued turns. See `docs/SWARM.md` "Aborting a swarm".

## Multipane mode

`nit multipane [--backend <model>] [--panes N] [--cwd PATH] [--terminal-command CMD ...]`
opens N independent chat panes (default 8, range `1..=32`, grid `ceil(sqrt(N))` columns).
`--backend` is optional: omit it for a per-pane picker, name a family (`claude` / `codex`)
to filter the picker, or name a lane id to pre-pick every pane. `--terminal-command`
takes exactly one command per pane and starts that pane in its terminal. Per-pane agent
ids are `<base>#mp-pane-NN`. Only chat dispatch is wired; editor, agent ops, and
visualizer panes are unavailable.

Every pane runs the standard `app::chat_input::submit_chat_input_and_dispatch` through
the alias-and-restore wrapper `multipane::dispatch::with_pane_aliased`, so every chat
command works per pane. Prompts land in `state.agents.messages` tagged with the pane's
`mission_id`, and `agent_console_view::render_pane` draws the pane. Sessions persist to
`<state_dir>/multipane/session-<workspace-hash>.json` on Ctrl+Q and on focus change
(at most one write per second); `chat_input` is capped at 4 KB. Keys are in
`docs/KEYBINDINGS.md` under "Multipane mode"; the spec is `docs/MULTIPANE.md`.

## Aborting in-flight work

User-facing triggers and behaviour are in `docs/SWARM.md` under "Aborting a swarm". Wiring:

- All triggers go through `chat_input::handle_abort`. `/abort` parsing is
  `app/chat_input.rs::parse_abort_command`; Ctrl+C and `x` (Missions tab) live in
  `app/agent_station.rs`; Esc-Esc uses `chat_input::record_chat_esc_press` (thread-local).
- The swarm runtime moves the run to `completed_runs` with `report_status = "ABORTED"`,
  drains queued turns, and pushes a `SYSTEM_ALERT_KIND` message. `CancelTurn { agent_id }`
  sets the per-turn cancel flag; the worker thread sees it within about 50 ms and kills
  the child.
- Operator cancels ride the same `TurnFailed` event but carry the
  `OPERATOR_CANCEL_TURN_MESSAGE` sentinel (`nit-core::agent_bus`), which takes the soft
  path: `AgentStatus::Idle`, no alert or signal, an Info diag, no LAB to WARN promotion.
