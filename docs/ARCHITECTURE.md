# Architecture

## Overview

nit is a terminal-first editor composed of eight crates:

- `nit-core`: state, actions, text buffers, config, agent bus, and IO (no terminal dependencies).
- `nit-games`: game theory tournament engine and strategy implementations (FSM, CA, one-sided TM).
- `nit-gol`: Conway’s Game of Life engine, rule evaluation, and snapshot encoding.
- `nit-metal`: Metal GPU acceleration for macOS (optional compute offload for games engine).
- `nit-syntax`: syntax highlighting engine and language registry (tree-sitter + fallback).
- `nit-tui`: rendering, layout, event loop, key mapping, agent runners (Codex + Claude), swarm orchestration, using ratatui + crossterm.
- `nit-utils`: shared filesystem, hashing, and path utilities.
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
- Buffers (main editor + scratchpad) stored in rope-backed `Buffer`
- Mode (Insert/Normal/Visual)
- Focused pane (Editor, Notes, JobOutput, Visualizer, GateMonitor, AgentOps, AgentConsole, FileTree, SearchPopup)
- Logs ring buffer and job progress/paused flag
- Visualizer state (seed, rule, mode, pause, wrap, generation, period, leaderboard)
- App kind (GoL or Games) plus app-specific runtime state
- Metrics: last render time, frame count, last action
- Optional prompt (e.g., confirm quit)

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

Agent Ops provides nine tabs: **Roster**, **Missions**, **DAG**, **MCP**, **Alerts**, **Patch**, **Evidence**, **Diagnostics**, **Scratchpad**.

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
Each `AgentLane` has an `id`, `kind`, `role`, `status`, `queue_len`, and optional `current_mission`.

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
    Controlled by `--codex-max-parallel-turns` (default `2`).

### Claude runtime

The Claude backend is implemented in `nit-tui` as a background `ClaudeRunner` thread that emits
`AgentBusEvent` updates into the main TUI loop.

- Spawns `claude -p --verbose --output-format stream-json` per turn.
- Additional flags: `--model <slug>`, `--effort <level>`, `--add-dir <cwd>`, `--max-turns 50`.
- Session resumption: `--resume <session_id>` reuses a prior session.
- Default allowed tools: `Read,Edit,Write,Bash,Glob,Grep,WebSearch,WebFetch`.
- Optional `--permission-mode` pass-through.
- Parses NDJSON stream on stdout for stage updates, token counts, and results.
- Session ids are tracked per agent (ad-hoc) and per mission+agent (swarm), mirroring the Codex
  thread-id pattern via `claude_session_ids` / `claude_mission_session_ids`.

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
  their configured `max_parallel_turns` (default `2`) concurrently **across different agent ids**.

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

### API wiring (CLI → TUI → runner)

The wiring for Codex runtime configuration is intentionally explicit:

- `crates/nit` (CLI) parses `--codex-runtime`, plus optional `--codex-sandbox`,
  `--codex-approval-policy`, and `--codex-max-parallel-turns`, into a
  `nit_tui::codex_runner::CodexRunnerConfig`.
- `crates/nit/src/main.rs` passes that config into `nit_tui::run(state, theme, log_rx, codex_runtime, codex_config)`.
- `crates/nit-tui/src/app.rs` forwards the config into `run_loop(...)` and spawns the runner via
  `CodexRunner::spawn(codex_runtime, codex_config)`.
- `crates/nit-tui/src/codex_runner.rs` applies the config:
  - Exec runtime: adds `-a <policy>` and `-s <sandbox>` to `codex exec ...`.
  - MCP runtime: forwards `approval-policy` and `sandbox` only when starting new sessions via the
    `codex` tool (continuations via `codex-reply` resume the existing session settings).

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

- Strategy implementations live in `crates/nit-games/src/strategy.rs`:
  FSM (Moore machine), CA (cellular automaton), and one‑sided TM (Turing machine).
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
  `crates/nit-games/src/fsm_enum.rs`.

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
- Language detection is centralized in a registry (extension, filename, shebang).
- Queries live in `crates/nit-syntax/queries` and can be swapped without touching TUI code.

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
