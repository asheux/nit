# Architecture

## Overview

nit is a terminal-first editor + agent station organized as **nine workspace
crates across six layers**. Each crate owns a single concern; the layers
describe how they fit together.

### Foundation

- **`nit-core`** — application state, actions, text buffers, config, agent bus,
  and substrate primitives (signals, claims, assumptions, mood, generation
  counter, mission memory, observers, arbiters, metabolism, genome reports +
  on-disk cache, seed encoders). Pure logic; no terminal dependencies. Most
  former single-file modules (`agent_bus.rs`, `substrate.rs`, `seed.rs`,
  `state.rs`, `genome_report.rs`, `genome_storage.rs`) are now directories
  with the same module path.
- **`nit-utils`** — shared filesystem helpers (atomic writes), BLAKE3 hashing
  (`stable_hash_bytes`, `SplitMix64`), and workspace path utilities used
  across every other module.

### Interface

- **`nit-tui`** — rendering, layout, event loop, key/mouse dispatch, agent
  runners (Codex + Claude + warm Claude pool), swarm orchestration, multipane
  grid, shadow + intake support agents. Built on ratatui + crossterm. This
  is the visible layer.
- **`nit-syntax`** — tree-sitter syntax highlighting engine and language
  registry, with a fallback path for unsupported languages.

### Lab engines

- **`nit-gol`** — Conway's Game of Life engine: rule evaluation, grid
  evolution, attractor detection, snapshot encoding.
- **`nit-games`** — game theory tournament engine and strategy implementations
  (FSM Moore machines, cellular automata, one-sided Turing machines), with an
  analytical fast evaluator for deterministic FSMs.

### Agent integration

- **`nit-mcp`** — MCP stdio JSON-RPC server (`nit-mcp-server` binary) that
  exposes substrate tools (`emit_signal`, `assert_claim`, `assert_assumption`)
  to the spawned `codex` process over a Unix-domain back-channel.

### Acceleration

- **`nit-metal`** — Apple Metal GPU compute shaders for macOS, an optional
  offload path for the games engine. No-op stubs on other platforms so the
  workspace builds unconditionally.

### Entry point

- **`nit`** — the CLI binary that wires arguments, tracing, lab dispatch, and
  TUI bootstrap, including the headless `games` subcommand tree and multipane
  launch wiring.

## Data Flow

```data-flow
crossterm events -> keymap -> Action -> nit-core::apply_action(state, action)
                               |                     |
                               +---- effect (save, reseed, etc.)
state -> render -> ratatui widgets -> terminal diff
```

## State Model (nit-core)

`AppState` is the single source of truth for the editor. It lives in memory
and is mutated only through `apply_action`. The fields group as follows:

- **Workspace** — workspace root, gitignore-derived exclusions, and the
  file-tree picker state.
- **Editor** — rope-backed buffers (main editor + scratchpad notes), mode
  (Insert / Normal / Visual), yank register, vim-style search (`/`, `*`,
  `#`), and the `:` command line.
- **UI / focus** — focused pane (Editor, Agent Chat, Agent Ops,
  Visualizer, Code Structural Quality), modal prompt, fuzzy-file
  picker, help and logs scroll positions, status line, and overlay
  state for the structural-quality view and substrate inspector.
- **Lab** — app kind (GoL or Games), visualizer state (seed, rule, mode,
  generation, period, leaderboard), the games tournament state, rule
  catalog, rule / protocol pickers, and persisted rule selection.
- **Agents** — `AgentsState` carrying lanes, missions, swarm / shadow
  runtime state, Agent Ops tabs, and the chat console; plus multipane
  mode state for parallel chat panes.
- **Substrate** — the persistent stigmergic layer (signals, claims,
  assumptions, mood) along with pending claim retries and pending
  arbiter interventions.
- **Genome** — cached reports per file, pre-turn baselines, per-turn and
  per-mission modified-file sets, retry counters, in-flight evaluation
  batches, and the last quality delta surfaced to agent retries.
- **Runtime** — log ring buffer, job progress / paused flag, render
  metrics, user settings, and tree-sitter syntax status.

## Text Encoding (Editor + Scratchpad)

Both the editor and scratchpad buffers are **UTF-8 only**:

- Files are loaded with `read_to_string` (UTF-8 decode) and stored in `String`/`ropey::Rope`.
- Saves write `String` bytes back out as UTF-8.

**Why:** the terminal, `ropey`, and our cursor/selection logic all operate on Unicode text
with UTF-8 indexing. Supporting multiple encodings would add detection/normalization
complexity, ambiguity, and error cases without clear benefit for this UI. UTF‑8 keeps
rendering and text‑measurement consistent, and avoids lossy conversions.

## Layout (nit-tui)

- Top bar with title, path, mode, encoding, ln/col.
- Main grid: left (Agent Chat + Agent Ops), center (Editor), right (Visualizer + Gate Monitor).
- Bottom bar with key hints; overlay for help and prompts.

## Agent Station (Codex + Claude)

nit includes an Agent Station UI (Agent Ops + Agent Chat) with support for multiple backends:
Codex (MCP or exec runtime), Claude (subprocess per turn), and a local mock lane.

### Agent Ops tabs

Agent Ops exposes **eight** tabs in the UI (defined in `crates/nit-core/src/state/` (`AgentOpsTab`) and rendered in `crates/nit-tui/src/widgets/agent_ops_view.rs`). The user-visible labels are:

| Tab label     | Enum variant  | Purpose                                                       |
|---------------|---------------|---------------------------------------------------------------|
| `ROSTER`      | `Roster`      | Agent lanes grouped by backend; swarm template + priority pins |
| `MISSIONS`    | `Missions`    | Mission history and phase (`PLAN` / `EXECUTE` / `VERIFY` / `REPORT`) |
| `DAG`         | `Dag`         | Swarm DAG view (task cards, deps, gate report)                 |
| `ARTIFACTS`   | `Evidence`    | Agent output bodies, task artifacts, verify summary            |
| `MCP`         | `Mcp`         | Codex MCP connection status and controls                       |
| `ALERTS`      | `Alerts`      | Operator-visible warnings / errors                             |
| `DIAG`        | `Diagnostics` | Ops timeline (`TurnStarted`/`TurnHeartbeat`/…)                 |
| `SCRATCHPAD`  | `Scratchpad`  | Genome feedback + mission-local notes                          |

(There is also a non-visible `Patch` variant in the enum; next/prev navigation routes around it — it is internal state only.)

### Roster seeding

- `nit --agents codex` loads model metadata from `~/.codex/models_cache.json` (used to populate the
  roster and reasoning-effort picker).
- `nit --agents claude` seeds Claude lanes when `claude` is available on `PATH`. At startup, nit
  probes `claude models --json` (with fallbacks) to discover available models.
- `nit --agents local` (alias `mock`) seeds a built-in local lane.
- `nit --agents all` (or default `nit`) includes all available lanes (Codex, Claude, and Gemini
  models are probed at startup via their respective CLIs).

### Agent lane kinds

`AgentLaneKind` in `nit-core` distinguishes backends: `Unknown`, `Mock`, `Codex`, `Claude`, `Gemini`.
Each `AgentLane` has an `id`, `kind`, `role`, `status`, `queue_len`, optional `current_mission`, and a `shadow: bool` flag that hides support agents from the roster and chat UI (see Shadow Agents below).

### AgentBusEvent protocol

Runners emit `AgentBusEvent` (see `crates/nit-core/src/agent_bus/`) which the TUI applies to `AppState`. Variants:

| Variant              | Purpose                                                    |
|----------------------|------------------------------------------------------------|
| `AgentUpsert`        | Register or update a lane                                  |
| `MissionUpsert`      | Create or update a mission record                          |
| `MessageAppend`      | Append a message to the console                            |
| `AlertAppend`        | Operator alert (Info / Warn / Error)                       |
| `DiagnosticAppend`   | Ops-timeline entry                                         |
| `McpStatus`          | Codex MCP connection state                                 |
| `TurnStarted`        | Turn began (carries optional `resume_thread_id`)           |
| `TurnHeartbeat`      | Keep-alive (used to detect idle timeouts)                  |
| `TurnStage`          | Stage label (`"context"`, `"tool:edit"`, etc.)             |
| `TurnLog`            | Free-form log line                                         |
| `FileWrite`          | File attribution (agent → path) for genome tracking        |
| `TokenCount`         | Live token / context budget update                         |
| `TurnCompleted`      | Final result + `threadId` / `session_id` for resumption    |
| `TurnFailed`         | Failure (includes last known thread/session id)            |

### Runtime modes (Exec vs MCP)

The Codex backend is implemented in `nit-tui` as a background `CodexRunner` thread that emits
`nit-core::AgentBusEvent` updates into the main TUI loop.

- **Exec runtime** (`--codex-runtime exec`):
  - Spawns `codex exec` per turn.
  - Parses the JSONL stdout stream for stage updates and token counts.
- **MCP runtime** (`--codex-runtime mcp`, default):
  - Spawns a persistent `codex mcp-server` child process.
  - Communicates over stdio using JSON-RPC 2.0 / MCP protocol `2024-11-05`.
  - Startup handshake:
    1. `initialize` (clientInfo `nit/<version>`)
    2. `initialized` (notify)
    3. `tools/list` (must include tools `codex` and `codex-reply`)
  - Per turn:
    - `tools/call` with tool **`codex`** for a new session (`{prompt, model, cwd, config.model_reasoning_effort}`)
    - `tools/call` with tool **`codex-reply`** to continue an existing session (`{threadId, prompt}`)
  - While waiting for the final response, the runner consumes `codex/event` notifications to surface
    compact progress “stages” in the UI.
  - Parallel turns: the runner can keep multiple turns in-flight across different agents by
    multiplexing JSON-RPC request ids (and routing `codex/event` updates via `_meta.requestId`).
    Controlled by `--codex-max-parallel-turns` (alias `--codex-parallel`; default `8`, range
    `1..=16`). The same cap is shared with the Claude runner.

### Claude runtime

The Claude backend is implemented in `nit-tui` as a background `ClaudeRunner` thread that emits
`AgentBusEvent` updates into the main TUI loop.

- Spawns `claude -p --verbose --output-format stream-json` per turn (cold-spawn path).
- Additional flags: `--model <slug>`, `--effort <level>`, `--add-dir <cwd>`, `--max-turns 50`
  (integrator turns lift this to `INTEGRATOR_MAX_TURNS=500`).
- Session resumption: `--resume <session_id>` reuses a prior session.
- Default allowed tools: `Read,Edit,Write,Bash,Glob,Grep,WebSearch,WebFetch`. Read-only turns
  (intake, shadow proposers/judge/review, read-only swarm tasks) drop down to
  `Read,Glob,Grep` only.
- Optional `--permission-mode` pass-through.
- Parses NDJSON stream on stdout for stage updates, token counts, and results.
- Session ids are tracked per agent (ad-hoc) and per mission+agent (swarm), mirroring the Codex
  thread-id pattern via `claude_session_ids` / `claude_mission_session_ids`.
- Optional warm worker pool (`crates/nit-tui/src/claude_pool.rs`), gated by
  `NIT_CLAUDE_POOL=1`. When enabled, "vanilla" turns (no resume, default
  `--max-turns`, no custom `--effort`) check out a long-lived `claude -p
  --input-format stream-json` worker, write one stream-json envelope to its
  stdin, and check the slot back in after the `result` event. Specialised
  turns (integrators, resumed sessions, custom `--effort`) always take the
  cold-spawn path. The cold-spawn branch stays byte-identical to the
  pre-pool runner and is kept as the rollback path.
- Idle-output reaper (`NIT_CLAUDE_TURN_IDLE_TIMEOUT_SECS`, default `900`s)
  applies on both paths; it fires only when the in-flight turn has not used
  any write-capable tool, so productive writer turns never time out.

### Gemini (detection only)

nit probes for `gemini` CLI availability at startup and lists discovered models in the roster,
but there is no `GeminiRunner` — Gemini lanes are currently display-only.

### Parallel turns (multi-agent workflows)

nit treats each roster entry (`AgentLane.id`) as an **agent**. For Codex lanes, that id is the
Codex model slug (e.g. `gpt-5.2`, `gpt-5.3-codex`); for Claude lanes, it is the Claude model
slug (e.g. `claude-sonnet-4-6`).

Parallelism exists at two layers:

- **UI queueing (per agent)**: `AppState.agents.queued_codex_turns` and
  `AppState.agents.queued_claude_turns` store prompts the operator submits while that same agent
  already has an active turn.
- **Runner parallelism (across agents)**: `CodexRunner` and `ClaudeRunner` each execute up to
  their configured `max_parallel_turns` (default `8`, range `1..=16`; shared across both runners)
  concurrently **across different agent ids**.

Key rules:

- **Per-agent single-flight:** at most one in-flight turn per `agent_id`. This avoids out-of-order
  session usage (especially `codex-reply`) and keeps `threadId` bookkeeping deterministic.
- **Global cap:** total in-flight turns across all agents is capped by
  `--codex-max-parallel-turns` (minimum `1`).
- **Dispatch fairness:** both runtimes skip queued turns whose agent already has an in-flight turn
  so other agents can make progress (simple round-robin over the queue).

#### Exec runtime (`codex exec`) parallelism

- Each in-flight turn spawns a `codex exec ...` child process (one process per turn).
- The runner loop keeps a list of active workers and starts more until the cap is reached.
- Stage and token counts come from parsing Codex JSONL on stdout and are forwarded as
  `AgentBusEvent::TurnStage` and `AgentBusEvent::TokenCount`.

#### MCP runtime (`codex mcp-server`) parallelism

The MCP runtime is a single persistent `codex mcp-server` process that can have multiple
in-flight JSON-RPC requests.

- Each turn sends exactly one JSON-RPC `tools/call` request with a unique request id (`id`).
- nit stores an `InFlightMcpTurn` record keyed by request id so it can:
  - match the final JSON-RPC response by `id`, and
  - route `codex/event` notifications to the correct agent by `_meta.requestId`.
- The final `tools/call` result yields a `threadId`. nit stores it:
  - per agent id for ad-hoc chat (`AgentsState.codex_thread_ids`), or
  - per mission id and agent id for missions (`AgentsState.codex_mission_thread_ids`).

Cancellation/timeouts in MCP mode:

- MCP Stop/Reconnect stops the server process, which cancels **all** in-flight requests (they
  share the same transport). nit emits `TurnFailed` for each in-flight turn and clears the in-flight
  maps before reconnecting.
- MCP reconnect preserves saved Codex `threadId` mappings for continuations; if Codex later reports
  “Session not found for thread_id …”, nit drops the stored thread id for that agent so the next turn
  starts a fresh thread (avoids broken “resume” loops).
- Turns have an optional total timeout via `NIT_MCP_TURN_TIMEOUT_SECS` (default disabled; set to
  `600` to enable; set to `0` to disable). If any in-flight turn exceeds the timeout, nit restarts
  the MCP server and fails all in-flight turns.
- Turns can have an idle timeout via `NIT_MCP_TURN_IDLE_TIMEOUT_SECS` (default disabled; set to
  `600` to enable; set to `0` to disable). If any in-flight turn stops producing `codex/event`
  notifications for longer than the idle timeout, nit restarts the MCP server and fails all
  in-flight turns.

#### UI visibility + interaction model

- `AgentsState.active_turns` tracks per-agent telemetry (started time, last heartbeat, last output,
  and last stage string).
- Agent Chat renders a small status table listing all in-flight turns (`agent`, `stage`, `elapsed`,
  heartbeat age, output age). When viewing a Swarm mission, the table also includes assigned
  agents that are pending/queued (for clearer multi-agent visibility).
- To inspect a specific agent’s transcript, select it in Agent Ops → Roster and press `Enter`.
- `@all <prompt>` broadcasts to multiple agents (Codex and Claude):
  - in mission context: targets the mission’s assigned agents,
  - otherwise: targets all available lanes.

#### Swarm orchestration (`@swarm`)

![Swarm DAG orchestrator: planner fans out into parallel proposers, converges through judge and review, integrates, then verifies — with a retry loop from verify back to integrate on gate failure or genome degradation](/nit-hero-swarm-dag.svg)

Operator guide: `docs/SWARM.md`.

nit supports two “multi-agent” modes:

- `@all <prompt>`: **fan-out** (every targeted agent gets the same prompt).
- `@swarm [all|N] [template=lab|parallel|bulk] [mission=general|research|computational-research] <prompt>`: **orchestrated workflows** (agents get different prompts and/or coordinated roles).

`@swarm` is implemented in `nit-tui` as a small orchestrator state machine (`SwarmRuntime`) that
creates a mission, asks a planner agent for a machine-readable plan, dispatches distinct tasks to
other agents, optionally runs a verification gate bundle, then asks the planner to synthesize a
final report.

Agent selection rules:

- The currently selected Codex or Claude model becomes the **planner/synthesizer**.
- Swarm size:
  - `@swarm <prompt>` defaults to 4 agents total (planner + 3), capped at 16.
  - `@swarm N <prompt>` uses `N` agents total (1–16).
  - `@swarm all <prompt>` uses all available Codex agents (still capped at 16).
- Agent selection:
  - `lab`: selects additional Codex/Claude agents from the roster (priority agents are preferred).
  - `parallel`/`bulk`: if any roster models are marked **priority**, Swarm restricts worker lanes
    to that selected pool; if you request more agents than selected, nit spawns mission-scoped
    clones of the selected models to reach the swarm size.
  - `parallel`/`bulk` fallback: if *no* priority models are selected, nit clones the planner model
    for worker lanes (so Swarm can still run even with a single model configured).
- If Swarm ends up with fewer than 2 agents (e.g. `@swarm 1 ...`), it falls back to a normal
  single-agent send.

Templates:

  - `lab` (default): DAG-style workflow optimized for “research lab” collaboration:
  - read-only proposal/review tasks feed a single-writer integrator task,
  - `research` / `computational-research` are reserved for external topic, paper, resource, or
    evidence-gathering work,
  - the roster stores both a default swarm template and a default swarm mission preset
    (`auto|general|research|computational-research`),
  - nit classifies each swarm mission as `general`, `research`, or `computational-research`,
  - mission focus can be explicit (`mission=...` / `Mission: ...`) or inferred from the operator
    request, with explicit prompt mission taking precedence over the roster preset and `auto`
    falling back to inference,
  - `general` blocks research roles, `research` allows `research`, and
    `computational-research` allows both `research` and `computational-research`,
  - `computational-research` additionally covers simulation, modeling, numerical methods,
    optimization, data/model fitting, pattern/network analysis, and reproducible research-computing
    workflows,
  - research-focused lab fallbacks shift from repo recon toward source survey, evidence comparison,
    and synthesis,
  - research-role outputs are expected to include sources, methods, assumptions, and ranked
    strategy recommendations,
  - tasks can have dependencies (`deps`) and multiple tasks can target the same agent id
    (they run sequentially),
  - only `writes=true` tasks are allowed to touch the workspace (enforced to the integrator agent),
  - scheduler dispatches tasks when their deps have finished (DONE/FAILED/SKIPPED).
- `parallel`: v1-style parallel split:
  - prefer one task per agent id, no deps, and maximum parallelism.
- `bulk`: “bulk orchestration” (ensemble + converge):
  - run multiple proposer tasks in parallel (different “lenses” on the same operator request),
  - run a judge task that depends on all proposers and selects the best approach,
  - run a single-writer integrator task (`writes=true`) that implements the selected approach.

Planner contract:

- nit sends the planner the operator request plus the list of available agent ids.
- The planner returns:
  1) a brief human-readable summary, and
  2) a JSON plan inside a ` ```json ` code block.

Plan validation + repair:

- Before dispatch, parsed plans run through a deterministic validator
  (`crates/nit-tui/src/swarm/validator.rs`) that classifies structural defects
  as `MustFix` or `Advisory`. `MustFix` violations trigger a bounded LLM
  repair loop (`swarm/repair.rs`, capped at `REPAIR_RETRY_LIMIT = 2`) that
  only continues while the planner is making concrete progress (strict
  improvement or proper subset of the prior violation set — same-set
  ping-pong stops the loop).
- `NIT_PLANNER_LEGACY=1` (truthy values `1` / `true` / `yes` / `on`,
  case-insensitive) disables the validator + repair flow entirely and reverts
  the planner stage to the pre-validator behaviour. Resolved once at
  `SwarmRuntime` construction and cached on `runtime.legacy_planner` so a
  mid-mission env flip cannot change behaviour halfway through a planning
  round.

Plan schema (v2):

```json
{
  "version": 2,
  "template": "lab",
  "integrator_agent_id": "gpt-5.2",
  "tasks": [
    {
      "id": "recon",
      "agent_id": "gpt-5.2",
      "role": "research",
      "title": "Topic scan",
      "prompt": "...",
      "deps": [],
      "writes": false,
      "artifacts": ["sources", "notes", "risks"],
      "done_when": "..."
    }
  ],
  "synthesis_prompt": "(optional extra guidance for the final synthesis step)"
}
```

Execution rules:

- Tasks are dispatched as agent turns (Codex or Claude) in the new mission when they become runnable:
  - tasks with `deps=[]` start immediately,
  - a task becomes runnable when all its deps have reached a terminal state.
- If tasks have recognizable roles (from the plan or roster hints), nit may add missing deps based on
  role-based producer/consumer ordering (configurable via `.nit/config.toml` `[swarm.role_deps]`).
- DAG validation is preflighted before dispatch:
  - default `strict`: aborts execution on cycles or unknown deps,
  - opt-in `repair`: drops unknown deps and removes deps that create cycles (`.nit/config.toml` `[swarm] dag_validation = "repair"`).
- Multiple tasks may target the same agent id; they will run sequentially (queued).
- Tasks run in parallel subject to `--codex-max-parallel-turns` (the runner may queue excess turns).
- Single-writer enforcement:
  - tasks may set `writes=true`, but only for the selected integrator agent id,
  - the scheduler dispatches at most one `writes=true` task at a time.
- When all task turns have completed/failed:
  - If a built-in gate bundle is detected, nit enters phase `VERIFY` and dispatches a verifier turn
    (first non-planner agent) to run the gates and emit a JSON report.
  - nit then dispatches a final **synthesis** turn to the planner containing the original prompt,
    each agent’s full output, and the verification report.

Gate bundles:

- `rust-ci` (auto-detected when a `Cargo.toml` exists in the workspace root or ancestors):
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test --workspace --all-features`
- `node-ci`, `python-ci`, `go-ci` — additional gate bundles available via config override.
- Gate bundle can be overridden in `.nit/config.toml`:
  ```toml
  [swarm.gates]
  default = "auto"  # or "none", "rust-ci", "node-ci", "python-ci", "go-ci"
  ```
- Gates run inside an agent turn (nit does not execute arbitrary shell commands itself), so outcomes
  respect the agent's sandbox + approval policy settings.

Fallback behavior:

- If the planner output has no parseable JSON plan, nit falls back to built-in prompts (recon,
  implementation plan, tests/verification, review) and proceeds with execution + synthesis.
- If the planner outputs the legacy v1 schema, nit will still run it (interpreting tasks as
  independent, read-only tasks with no deps).

Safety note:

- `@swarm` is orchestration and aggregation; it does not automatically merge code changes.
  The planner prompt encourages using a single “integrator” for file edits to reduce conflicts.

#### Shadow agents (`@shadow`, auto-shadow)

Shadows are a complementary pattern to `@swarm`: instead of planning a DAG across the roster, they run a fixed **propose-a / propose-b → judge → review → main** pipeline behind a single selected agent and prepend the four outputs as advisory context. Operator guide: `docs/SHADOWS.md`.

- Implemented in `crates/nit-tui/src/shadow.rs` (`ShadowRuntime`, stage enum, prompt builders, lane id helpers).
- Activated either explicitly via `@shadow <prompt>` or automatically when `should_auto_enable_shadows(prompt)` returns true (prompt > 500 chars or contains `refactor` / `migrate` / `rewrite` / `implement` / `overhaul` / `restructure`).
- Suppressed inside an active swarm mission and for `@all` / `@swarm` / `@new` / `@queue` prefixes; suppression is decided in `chat_input.rs` before dispatch.
- Shadow lanes are created with `AgentLane { shadow: true, ... }` and the id format `<base_id>#shadow-<run_id>-<role>` (parse with `shadow::parse_shadow_lane_id`). Roster, chat, and Ops views filter them out so only the main agent's turn is visible.
- Shadow turns run `read_only = true`, which in the Claude runner restricts `--allowedTools` to `Read,Glob,Grep` and in the Codex runner forwards the read-only sandbox flag.
- Only one shadow run per main agent can be in flight at once; concurrent prompts to the same agent queue normally.

#### Intake agent (hidden Claude-class preprocessor)

Before every Claude-class chat dispatch (and only Claude-class — codex/gemini lanes are skipped with an `intake.skipped` diag), nit runs a hidden one-step intake turn that classifies the operator's intent and, on `write` / `mixed` classifications, appends a `## FILE CHECKLIST (non-negotiable)` block to the raw prompt. Operator guide: `docs/INTAKE.md`.

- Implemented in `crates/nit-tui/src/intake.rs` — lifecycle mirrors shadows
  (synthetic `<base>#intake-<run_id>` lane, `read_only = true`, single-stage
  pipeline, stash-then-resume).
- Settings + kill switch: `Settings::intake_enabled` (default `true`) and
  `NIT_INTAKE_DISABLED=1` (read on every dispatch — flip without restarting).
- Failures (timeout, JSON parse, prefix violation, runner exit, dispatch
  enqueue failure) all fall back to **passthrough**: the operator's raw
  prompt dispatches as-is. The chat never shows an error banner; diags land
  in Agent Ops → DIAG.
- Operator `/abort` (and siblings) cancels any pending intake turn via
  `intake::cancel_pending_intake` and drops the deferred resume.

#### Multipane mode (`nit multipane`)

`nit multipane [--backend <model>] [--panes N] [--cwd PATH]` opens a grid of N independent chat panes (default 8, range `1..=32`), each anchored at its own working directory. State lives in `MultipaneState` on `AppState` (`crates/nit-core/src/state/multipane.rs`); when `state.multipane` is `Some`, the TUI dispatches to `multipane::run_loop` instead of the standard event loop. Pane sessions persist to `<state_dir>/multipane/session-<workspace-hash>.json`.

- Per-pane agent ids use the form `<base>#mp-pane-NN` so they coexist with
  `#swarm-`, `#chat-clone-`, `#shadow-`, and `#intake-` conventions.
- Editor / agent ops / visualizer / file tree are unavailable; only chat
  dispatch + per-pane dir search are wired. Disallowed keys are silently
  swallowed by `multipane::runtime::handle_key`.
- Every pane runs the canonical `app::chat_input::submit_chat_input_and_dispatch`
  via a Lens-B alias-and-restore wrapper (`multipane::dispatch::with_pane_aliased`),
  so `@swarm` / `@shadow` / `@all` / `@new` / `@queue` / `/abort` / queueing /
  swarm-followup re-activation all work per pane.

Spec + keymap: `docs/MULTIPANE.md`.

### API wiring (CLI → TUI → runner)

The wiring for Codex runtime configuration is intentionally explicit:

- `crates/nit` (CLI) parses `--codex-runtime`, plus optional `--codex-sandbox`,
  `--codex-approval-policy`, and `--codex-max-parallel-turns`, into a
  `nit_tui::codex_runner::CodexRunnerConfig` (assembled in `crates/nit/src/bootstrap.rs::build_runner_configs`).
- `crates/nit/src/main.rs` passes that config — together with the parallel
  `ClaudeRunnerConfig` — into `nit_tui::run(state, theme, log_rx,
  codex_runtime, codex_config, claude_config)`.
- `crates/nit-tui/src/app/runner.rs` forwards the config into `run_loop(...)`
  and spawns the runners via `CodexRunner::spawn(...)` and
  `ClaudeRunner::spawn(...)`.
- `crates/nit-tui/src/codex_runner/` applies the config:
  - Exec runtime: adds `-a <policy>` and `-s <sandbox>` to `codex exec ...`.
  - MCP runtime: forwards `approval-policy` and `sandbox` only when starting new sessions via the
    `codex` tool (continuations via `codex-reply` resume the existing session settings).

### Genome feedback + auto-retry

When an agent edits files (tracked via `AgentBusEvent::FileWrite`), nit re-runs the genome/parsimony analyzer on the changed files and compares tiers against a baseline captured before the turn. If quality degraded OR the parsimony detector flags bloat, `build_genome_retry_prompt` (in `crates/nit-tui/src/app/genome_retry.rs`) re-dispatches a follow-up prompt to the writer. Constants live in the same file: `GENOME_RETRY_LIMIT = 3` and `GENOME_RETRY_MIN_LINES = 120` (files shorter than that are skipped to avoid over-engineering trivial modules). See `docs/SEEDS.md` for the parsimony rule and tier system.

### Thread + mission context

- For ad-hoc chat (no mission), the last known session id is tracked per model so future
  prompts can resume the session:
  - Codex: `codex_thread_ids` (maps agent_id → threadId)
  - Claude: `claude_session_ids` (maps agent_id → session_id)
- For missions, session ids are tracked per mission *and* per model so each agent can continue its
  own mission thread independently:
  - Codex: `codex_mission_thread_ids` (mission_id → agent_id → threadId)
  - Claude: `claude_mission_session_ids` (mission_id → agent_id → session_id)
- `AgentBusEvent::TurnCompleted` / `TurnFailed` store the returned session id back into the
  appropriate map.

## MCP status + notes

The MCP tab in Agent Ops reflects `AgentsState.mcp` (connection state + endpoint + last error).

Implementation notes:

- Token accounting: MCP mode consumes `codex/event` token count notifications and emits
  `AgentBusEvent::TokenCount` so the UI can keep context usage estimates fresh.
- Cancellation/timeouts:
  - MCP Stop/Reconnect cancels in-flight turns by stopping the server process.
  - Turns have an optional total timeout via `NIT_MCP_TURN_TIMEOUT_SECS` (default disabled; set to
    `600` to enable; set to `0` to disable).
  - Turns can have an idle timeout via `NIT_MCP_TURN_IDLE_TIMEOUT_SECS` (default disabled; set to
    `600` to enable; set to `0` to disable).
  - Reconnect robustness: the runner checks for unexpected `codex mcp-server` exit, drops the dead
  handle, and retries with a short backoff (operator can still use MCP tab `r`).
- Latency: `latency_ms` is best-effort; it is updated on connect and on successful turns.
- Sandbox/approval pass-through:
  - `nit --codex-sandbox <read-only|workspace-write|danger-full-access>`
  - `nit --codex-approval-policy <untrusted|on-failure|on-request|never>` (default: `never`)
  - In MCP mode these are applied when starting new sessions via the `codex` tool; `codex-reply`
    continues an existing session and does not accept these options.

## Lab Dispatch (Active Lab)

- The CLI supports `nit` (default GoL), `nit gol`, `nit games`, and `nit --lab <gol|games>`.
- `LabId`/`AppKind` in `AppState` selects the active lab and gates commands/keybindings.
- The TUI instantiates lab-specific runtimes:
  - GoL: seed runtime + GoL Petri Dish + GoL visualizer widget.
  - Games: Games Petri Dish + Games visualizer dashboard widget + run/replay tooling.
- Unnamespaced commands (`:run`, `:hide`, etc.) route to the active lab.
  Namespaced commands are accepted **only** for the active lab to avoid cross‑lab conflicts.

## Games Config (Payoff Matrix)

- Games configs support a payoff matrix under `[payoff]`.
- `matrix` is a 2×2 grid where each cell is `[A_payoff, B_payoff]`.
- When `matrix` is present, it is the source of truth; `R/S/T/P` must match it.

## Games Output Logs

- Runs are stored under `runs/games/<timestamp>__seed-<seed>/` with:
  - `run_summary.json` (schema v2) with config + results + paths
  - `definitions.json` and `results.json`
  - `events.ndjson` and `history.ndjson` when enabled
  - `config.toml` snapshot + `analysis/` outputs
- History logs are per‑match outcome strings when enabled.
  Outcomes are encoded as digits from player A’s perspective:
  `0=CC`, `1=CD`, `2=DC`, `3=DD`.
- Analysis outputs (`analysis__*.json`, `analysis_matches__*.{csv,ndjson}`,
  `analysis_strategies__*.csv`, `analysis_trajectories__*.csv`) are generated
  via `:games analyze` and summarize per‑match, steady‑state, and trajectory stats.

## Games Engine (Phase 2)

See `docs/GAMES.md` for the engine split (kernel vs stepper), deterministic seeding,
and parallel logging behavior.

## Program Strategies (Phase 3E)

- Strategy implementations live in `crates/nit-games/src/strategy/`:
  FSM (Moore machine), CA (`strategy/ca/`), and one‑sided TM (`strategy/tm/`).
- Deterministic FSM/memory strategies have fast‑eval models in
  `crates/nit-games/src/fast_eval.rs` (cycle detection on combined state).
- One‑sided TMs are deterministic but currently run through the simulator
  (not fast‑evaluated).
- Program definitions are serialized into `definitions.json`, and TM-derived
  metrics are surfaced in `run_summary.json` results.
- Strategy introspection/export lives in `crates/nit-games/src/introspection.rs`
  and feeds both CLI (`nit games inspect/graph`) and the TUI `:games inspect`
  popup for downstream visualization workflows.
- FSM enumeration + canonicalization utilities live in
  `crates/nit-games/src/fsm_enum/`.

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

- All crates forbid unsafe code (`#![forbid(unsafe_code)]`) except `nit-metal` (Metal GPU interop).
- Terminal restoration uses guard structs and panic hooks to exit raw/alt screen cleanly.

## Syntax Highlighting

nit uses a dedicated crate (`nit-syntax`) to provide fast, incremental, tree-sitter‑based
highlighting with a plain‑text fallback. The pipeline is intentionally split so future
semantic tokens (LSP) can layer on top of syntactic tokens without rewriting UI code.

**Language coverage (28 active grammars on tree-sitter 0.25):**

The canonical list lives in `crates/nit-core/src/languages.rs` as the
`LANGUAGES` table (`LanguageInfo` entries with extensions, filenames,
shebangs, injection aliases, and an `is_code` flag). Every detection
gate in the workspace — `nit-syntax`'s `detect_by_path` /
`detect_by_extension` / `detect_by_injection_alias`, the file-watcher,
the swarm scope walker, the markdown fenced-code resolver, and the
seed encoders' supported-language predicate — pulls from that table.
The grouping below is documentation only:

| Family | Languages |
|--------|-----------|
| Systems | Rust, Go, C, C++, Zig |
| JVM | Java, Kotlin |
| Scripting | Python, JavaScript, TypeScript, Ruby, Lua, PHP, Bash |
| Functional | OCaml, Haskell, Elixir, Lean |
| Mobile / Apple | Swift |
| Markup / Config | Markdown, HTML, CSS, JSON, TOML, YAML, Nix |
| Data / Build | SQL, Makefile |

Dockerfile has an entry in `LANGUAGES` (so filename detection works for
`Dockerfile`, `Containerfile`, `Dockerfile.prod`, `prod.dockerfile`),
but `nit-syntax::grammars::tree_sitter_language` returns `None` for it
— the upstream `tree-sitter-dockerfile` crate is pinned to an older
ABI, so it currently renders as plain text until upstream ships a
0.25-compatible release.

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
- Language detection is centralized in `crates/nit-core/src/languages.rs`
  (the `LANGUAGES` table). `detect_by_path`, `detect_by_extension`,
  `detect_by_filename`, `detect_by_shebang`,
  `detect_by_injection_alias`, and `is_supported_extension` all read
  from that single source — `nit-syntax`'s `LanguageRegistry` is a thin
  shim that translates table hits into `LanguageId` variants.
- Queries live in `crates/nit-syntax/queries` and can be swapped
  without touching TUI code.
- Adding a language is a two-edit workflow:
  1. Append a `LanguageInfo` entry to `LANGUAGES` in
     `crates/nit-core/src/languages.rs` (extensions, filenames,
     shebangs, injection aliases, `is_code` flag). This automatically
     unlocks extension matching, filename matching, shebang
     resolution, injection-alias resolution, the file-watcher's
     trackable-source predicate, the swarm scope walker, and the
     markdown fenced-code resolver.
  2. Wire the grammar: add the `tree-sitter-<lang>` crate dep to
     `crates/nit-syntax/Cargo.toml` (and to
     `crates/nit-core/Cargo.toml` if the seed encoders should score
     the language), add a variant to `LanguageId` in
     `crates/nit-syntax/src/language/id.rs`, then add matching arms in
     `nit-syntax/src/language/grammars.rs::tree_sitter_language` (and
     `highlights_query`, plus a hand-rolled
     `queries/<lang>/highlights.scm` if the grammar crate doesn't
     export a `HIGHLIGHTS_QUERY` constant). For seed-encoder coverage,
     also add a `SeedLanguage` variant + `ts_language` arm in
     `crates/nit-core/src/seed/encoders/lang.rs`.
  3. Add a smoke-test row in
     `crates/nit-syntax/src/tests/engines.rs` covering at least one
     keyword.
- Extension lists, filename matchers, alias dispatches, and
  `is_code` predicates no longer need per-language edits — they all
  derive from the central `LANGUAGES` table.
- For visual eyeballing, `testall/` at the repo root ships one
  minimal sample per language — open with `nit testall/` and tab
  through.

## Visualizer (Game of Life)

The Visualizer pane runs a Conway’s Game of Life simulation seeded from visible editor/scratchpad
text. The TUI drives a lightweight tick loop for simulation, while heavier work (rule search
and snapshot I/O) runs in a background worker thread.

**Pipeline**
- Seed text (viewport) → ASCII-to-grid mapping → GoL simulation (nit-gol).
- Rule search evaluates Life-like rules asynchronously and reports a leaderboard.
- Visualizer state (rule, generation, alive count, attractor, auto-stop policy, mode) is rendered
  in the pane and summarized in Gate Monitor.
- The simulation can auto-pause on fixed points or repeats based on the auto-stop policy.

**Rule Model**
- The simulation always runs **one active rule at a time**.
- Default is **B3/S23 (Conway’s Life)** for familiarity and stable baseline behavior.
- Search mode evaluates many rules in the background, but `Apply` swaps in a single rule
  so the live grid remains deterministic and the step function stays simple and fast.

**Snapshots**
- Stored under `gol-snapshots/` in the workspace root as RLE + JSON metadata.
- Deduped by grid hash and pruned by max file count.

### Seed Encoding System

The seed encoding system converts editor text into a Game of Life genome (initial grid
pattern). The pipeline lives in `nit-core/src/seed/` (encoder modules + utils) and
`nit-tui/src/seed_runtime.rs` (runtime orchestration).

**Encoding Pipeline**

```
text input → encoder → value grid → jitter → density threshold → bit grid → symmetry → target grid
```

1. **Encoder** produces a base value grid (each cell 0-255) from the input text.
2. **Jitter** adds random perturbation (SplitMix64 PRNG, upper bits via `>> 48`) to break
   uniformity. Amplitude is `jitter * 32` intensity units.
3. **Density threshold** converts values to alive/dead: `cell >= (1 - target_density) * 255`.
4. **Symmetry** enforces spatial constraints using union semantics — if either mirrored cell
   is alive, both become alive.
5. **Target grid** scales the bit grid into the final GoL grid dimensions with placement
   and padding.

**Encoders**

| Encoder | Grid Size | Category | Method |
|---------|-----------|----------|--------|
| `token_spectrum` | 32x32 | AST-driven (default) | Tree-sitter semantic token classification into 9 value ranges |
| `ast_structure` | 32x32 | AST-driven | DFS tree walk encoding depth, child count, byte span, node kind |
| `complexity_field` | 32x32 | AST-driven | Per-line cyclomatic complexity, token entropy, nesting, identifier uniqueness |
| `structural` | 32x32 | Hybrid | Per-byte Shannon entropy, bracket depth, token signal, n-gram uniqueness |
| `ascii_bytes` | 32x32 | Byte-level | Maps text bytes with index mixing and PRNG |
| `hilbert_bits` | 32x32 | Byte-level | Hilbert space-filling curve mapping |
| `lifehash16` | 16x16 | Byte-level | Pure PRNG derived from text hash |

See `docs/SEEDS.md` for detailed encoder documentation, anti-gaming properties, and fallback behavior.

**Seed Parameters**

| Parameter | Default | Range | Description |
|-----------|---------|-------|-------------|
| `symmetry` | mirror-x | none, mirror-x, mirror-y, rotate-180 | Spatial symmetry (union: either side alive → both alive) |
| `target_density` | 0.31 | 0.08 - 0.7 | Target proportion of alive cells |
| `padding` | 1 | 0+ | Border padding in cells |
| `placement` | center | center, top-left | Seed position within the grid |
| `jitter` | 0.04 | 0.0 - 0.25 | Random perturbation amplitude |

**Change Detection**

The seed runtime (`seed_runtime.rs`) detects parameter changes by direct `PartialEq`
comparison on `SeedParams` (not fingerprint hashing), so arbitrarily small changes to
density or jitter trigger recomputation. A debounce timer (120ms) prevents thrashing
during rapid edits.

**Seed Hashing**

`hash_seed()` produces a 64-bit identity hash using BLAKE3 incremental hashing (no
intermediate allocation). Inputs: encoder id, params fingerprint, variant, grid dimensions,
and cell data. The fingerprint quantizes density and jitter to 1e-6 precision.

**PRNG**

All randomness (jitter, encoders, seed search mutations) uses `SplitMix64`
(`nit-utils/src/hashing.rs`), a full-period 2^64 PRNG with excellent bit distribution.
Jitter additionally extracts upper bits (`>> 48`) before the modulo to avoid low-bit
correlation.

**Seed Search**

Toggled via `Ctrl+G` or the **SEARCH** title button. A background worker mutates seed
parameters (symmetry, density, jitter, padding, placement) and scores candidates by:

```
score = component_count - 40 * |actual_density - target_density|
```

Best proposals are surfaced to the UI and applied with `Ctrl+A` or the **APPLY** title button.

**Title Bar Buttons**

The visualizer pane header has four clickable buttons:

| Button | Action | Keyboard |
|--------|--------|----------|
| **APPLY** | Apply the best seed search proposal (swaps in candidate params) | `Ctrl+A` |
| **SEED** | Cycle symmetry: none → mirror-x → mirror-y → rotate-180 | `Ctrl+S` |
| **SNAP** | Snapshot current seed to `gol-snapshots/` as RLE + JSON metadata | `Ctrl+N` |
| **SEARCH** | Toggle seed search background worker on/off | `Ctrl+G` |

Buttons are rendered as inverted-color spans and use column-based hit detection
(`visualizer_view::title_button_hit`).

**Key Source Files**

| File | Contents |
|------|----------|
| `nit-core/src/seed/` | Encoders, symmetry, jitter, thresholding, hashing, component counting (`encoders/`, `params.rs`, `grid_types.rs`, `utils.rs`, `view_modes.rs`) |
| `nit-tui/src/seed_runtime.rs` | Runtime loop, change detection, compute worker, search worker, snapshot dispatch |
| `nit-tui/src/widgets/visualizer_view.rs` | Visualizer rendering, title bar buttons, click hit detection |
| `nit-utils/src/hashing.rs` | SplitMix64 PRNG, BLAKE3 stable hashing |
