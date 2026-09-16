# Architecture

This doc is the contributor map of nit: the workspace crates, the state
model, the agent bus, and the agent runtimes. It also summarises the lab
engines, rendering, saving, syntax highlighting, and the visualizer. Topics
that have their own doc get a short summary and a pointer here.

## Overview

nit is a terminal editor and agent station built as ten workspace crates
across six layers. Each crate owns one concern.

### Foundation

- `nit-core`: application state, actions, text buffers, config, agent bus,
  and substrate primitives (signals, claims, assumptions, mood, generation
  counter, mission memory, observers, arbiters, metabolism, genome reports
  with an on-disk cache, seed encoders). Pure logic with no terminal
  dependencies.
- `nit-utils`: atomic file writes, BLAKE3 hashing (`stable_hash_bytes`), the
  `SplitMix64` PRNG, and workspace path helpers used by every other crate.

### Interface

- `nit-tui`: rendering, layout, event loop, key and mouse dispatch, agent
  runners (Codex, Claude, and the warm Claude pool), swarm orchestration,
  the multipane grid, the multiway runtime, and the shadow and intake
  support agents. Built on ratatui and crossterm.
- `nit-syntax`: tree-sitter highlighting engine and language registry, with
  a plain-text fallback for unsupported languages.

### Lab engines

- `nit-gol`: Game of Life engine: rule evaluation, grid evolution, attractor
  detection, snapshot encoding.
- `nit-games`: game theory tournament engine and strategies (FSM Moore
  machines, cellular automata, one-sided Turing machines), with an
  analytical fast evaluator for deterministic FSMs.

### Agent integration

- `nit-mcp`: MCP stdio JSON-RPC server (the `nit-mcp-server` binary). It
  exposes the substrate tools `emit_signal`, `assert_claim`, and
  `assert_assumption` to the spawned `codex` process over a Unix-domain
  socket back-channel. nit sets `NIT_MCP_BACKCHANNEL_SOCKET` and
  `NIT_MCP_AGENT_ID` on the server, and `NIT_MCP_BACKCHANNEL_PORT` on hosts
  without Unix sockets.
- `nit-multiway`: a pure best-first search engine over git-committed world
  states. It spawns no processes and has no TUI. The opt-in multiway mode
  uses it. See `docs/MULTIWAY.md`.

### Acceleration

- `nit-metal`: Apple Metal compute shaders for macOS, an optional offload
  path for the games engine. No-op stubs on other platforms, so the
  workspace builds everywhere.

### Entry point

- `nit`: the CLI binary. It parses arguments, sets up tracing, dispatches
  labs, and boots the TUI. It also hosts the headless `games` subcommands,
  the multipane launcher, and `nit update` (alias `nit upgrade`).

## Data Flow

```data-flow
crossterm events -> keymap -> Action -> nit-core::apply_action(state, action)
                               |                     |
                               +---- effect (save, reseed, etc.)
state -> render -> ratatui widgets -> terminal diff
```

## State Model (nit-core)

`AppState` is the single source of truth. It lives in memory and changes
only through `apply_action`. Its fields group as follows:

- Workspace: workspace root, gitignore-derived exclusions, and the file-tree
  picker.
- Editor: rope-backed buffers (main editor and scratchpad notes), mode
  (Insert / Normal / Visual), yank register, vim-style search (`/`, `*`,
  `#`), and the `:` command line.
- UI and focus: focused pane (Editor, Agent Chat, Agent Ops, Visualizer,
  Code Structural Quality), modal prompt, fuzzy file picker, help and log
  scroll positions, status line, and overlay state for the structural
  quality view and the substrate inspector.
- Lab: app kind (GoL or Games), visualizer state (seed, rule, mode,
  generation, period, leaderboard), games tournament state, rule catalog,
  rule and protocol pickers, and the persisted rule selection.
- Agents: `AgentsState` with lanes, missions, swarm and shadow runtime
  state, Agent Ops tabs, and the chat console; plus multipane state.
- Substrate: the persistent stigmergic layer (signals, claims, assumptions,
  mood), pending claim retries, and pending arbiter interventions.
- Genome: cached reports per file, pre-turn baselines, per-turn and
  per-mission modified-file sets, retry counters, in-flight evaluation
  batches, and the last quality delta shown to agent retries.
- Runtime: log ring buffer, job progress and paused flag, render metrics,
  user settings, and tree-sitter status.

## Text Encoding (Editor + Scratchpad)

Both buffers are UTF-8 only. Files load with `read_to_string` into a
`String` or `ropey::Rope`, and saves write UTF-8 bytes back out. The
terminal, `ropey`, and the cursor and selection logic all index Unicode
text as UTF-8. One encoding keeps rendering and text measurement consistent
and avoids lossy conversions.

## Layout (nit-tui)

- Top bar: title, path, mode, encoding, line and column.
- Main grid: left (Agent Chat and Agent Ops), center (Editor), right
  (Visualizer and Gate Monitor).
- Bottom bar: key hints. Overlays for help and prompts.

## Agent Station (Codex + Claude)

The Agent Station is Agent Ops plus Agent Chat. It supports Codex (MCP or
exec runtime), Claude (one subprocess per turn), and a local mock lane.
Multiway mode (`NIT_MULTIWAY=1`, then `@multiway <task>`) searches over git
worktrees with the `nit-multiway` crate and `crates/nit-tui/src/multiway/`;
see `docs/MULTIWAY.md`.

### Agent Ops tabs

Agent Ops has eight visible tabs. `AgentOpsTab` lives in
`crates/nit-core/src/state/agent_types.rs` and renders in
`crates/nit-tui/src/widgets/agent_ops_view.rs`.

| Tab label | Enum variant | Purpose |
|---|---|---|
| `ROSTER` | `Roster` | Agent lanes grouped by backend; swarm template and priority pins |
| `MISSIONS` | `Missions` | Mission history and phase (`PLAN` / `EXECUTE` / `VERIFY` / `REPORT`) |
| `DAG` | `Dag` | Swarm DAG view (task cards, deps, gate report) |
| `ARTIFACTS` | `Evidence` | Agent output bodies, task artifacts, verify summary |
| `MCP` | `Mcp` | Codex MCP connection status and controls |
| `ALERTS` | `Alerts` | Warnings and errors |
| `DIAG` | `Diagnostics` | Ops timeline (`TurnStarted`, `TurnHeartbeat`, and so on) |
| `SCRATCHPAD` | `Scratchpad` | Genome feedback and mission-local notes |

The enum also has a hidden `Patch` variant. Next and previous navigation
skips it.

### Roster seeding

- `nit --agents codex` loads model metadata from `~/.codex/models_cache.json`
  for the roster and the reasoning-effort picker.
- `nit --agents claude` seeds Claude lanes when `claude` is on `PATH`. At
  startup nit probes `claude models --json`, with fallbacks.
- `nit --agents local` (alias `mock`) seeds a built-in local lane.
- `nit --agents all`, or plain `nit`, includes every available lane. Codex,
  Claude, and Gemini models are probed through their CLIs at startup.

### Agent lane kinds

`AgentLaneKind` in `nit-core` is one of `Unknown`, `Mock`, `Codex`, `Claude`,
or `Gemini`. Each `AgentLane` has an `id`, `kind`, `role`, `status`,
`queue_len`, an optional `current_mission`, and a `shadow` flag. The flag
hides support agents from the roster and chat (see Shadow agents below).

### AgentBusEvent protocol

Runners emit `AgentBusEvent` (`crates/nit-core/src/agent_bus/`), and the
TUI applies each event to `AppState`.

| Variant | Purpose |
|---|---|
| `AgentUpsert` | Register or update a lane |
| `MissionUpsert` | Create or update a mission record |
| `MessageAppend` | Append a message to the console |
| `AlertAppend` | Alert (Info / Warn / Error) |
| `DiagnosticAppend` | Ops-timeline entry |
| `McpStatus` | Codex MCP connection state |
| `TurnStarted` | Turn began (carries an optional `resume_thread_id`) |
| `TurnHeartbeat` | Keep-alive, used to detect idle timeouts |
| `TurnStage` | Stage label (`"context"`, `"tool:edit"`, and so on) |
| `TurnLog` | Free-form log line |
| `FileWrite` | File attribution (agent to path) for genome tracking |
| `TokenCount` | Live token and context budget update |
| `TurnCompleted` | Final result plus `threadId` / `session_id` for resumption |
| `TurnFailed` | Failure, with the last known thread or session id |
| `EmitSignal` | Add a ready-made signal to the substrate |
| `AssertClaim` | Assert a claim; conflicts emit `ClaimViolation` signals |
| `AssertAssumption` | Assert an assumption; never fails |
| `EmitSignalRequest` | Signal from the MCP back-channel; the substrate mints the id on apply |
| `AssertClaimRequest` | Claim from the back-channel; id minted on apply |
| `AssertAssumptionRequest` | Assumption from the back-channel; id minted on apply |
| `SetMood` | Set the system mood and lock auto-transitions for `MOOD_OVERRIDE_LOCK_GENS` generations |
| `BackendModelsLoaded` | Async model-probe result; fills the roster's model lists and metadata |

### Codex runtime modes (exec vs MCP)

The Codex backend is a background `CodexRunner` thread in `nit-tui` that
emits `AgentBusEvent` updates into the main loop.

- Exec runtime (`--codex-runtime exec`): spawns `codex exec` per turn and
  parses its JSONL stdout for stage updates and token counts.
- MCP runtime (`--codex-runtime mcp`, the default): spawns one persistent
  `codex mcp-server` child and speaks JSON-RPC 2.0 over stdio with MCP
  protocol `2024-11-05`.

MCP startup handshake:

1. `initialize` (clientInfo `nit/<version>`)
2. `initialized` (notification)
3. `tools/list` (must include the tools `codex` and `codex-reply`)

MCP per-turn calls:

- `tools/call` with tool `codex` starts a new session
  (`{prompt, model, cwd, config.model_reasoning_effort}`).
- `tools/call` with tool `codex-reply` continues a session
  (`{threadId, prompt}`).
- While waiting for the final response, the runner turns `codex/event`
  notifications into compact progress stages in the UI.

### Claude runtime

The Claude backend is a background `ClaudeRunner` thread in `nit-tui`.

- Cold-spawn path: `claude -p --verbose --output-format stream-json` per
  turn, plus `--model <slug>`, `--effort <level>`, `--add-dir <cwd>`, and
  `--max-turns 50` (`INTEGRATOR_MAX_TURNS = 500` for integrator turns).
  `--resume <session_id>` reuses a session, and `--permission-mode` passes
  through when set.
- Allowed tools: `Read,Edit,Write,Bash,Glob,Grep,WebSearch,WebFetch`.
  Read-only turns (intake, shadow proposers, judge, review, read-only swarm
  tasks) get `Read,Glob,Grep` only.
- The runner parses the NDJSON stream on stdout for stages, token counts,
  and results. Session ids are tracked per agent for chat and per mission
  and agent for swarms (`claude_session_ids`, `claude_mission_session_ids`).
- Warm pool (`NIT_CLAUDE_POOL=1`, `crates/nit-tui/src/claude_pool/`):
  vanilla turns (no resume, default `--max-turns`, no custom `--effort`)
  check out a long-lived `claude -p --input-format stream-json` worker,
  write one stream-json envelope to its stdin, and return the slot after
  the `result` event. Integrators, resumed sessions, and custom `--effort`
  turns always cold-spawn.
- Idle-output reaper (`NIT_CLAUDE_TURN_IDLE_TIMEOUT_SECS`, default `900`
  seconds): applies on both paths. It fires only when the turn has not used
  a write-capable tool, so productive writer turns never time out.

### Gemini (detection only)

nit probes for the `gemini` CLI at startup and lists its models in the
roster. There is no `GeminiRunner`, so Gemini lanes are display only.

### Parallel turns

Each roster entry (`AgentLane.id`) is an agent. For Codex lanes the id is
the model slug (for example `gpt-5.2` or `gpt-5.3-codex`). For Claude lanes
it is the Claude model slug (for example `claude-sonnet-4-6`).

Parallelism has two layers:

- UI queueing per agent: `AppState.agents.queued_codex_turns` and
  `queued_claude_turns` hold prompts you submit while that agent already
  has an active turn.
- Runner parallelism across agents: `CodexRunner` and `ClaudeRunner` each
  run up to `max_parallel_turns` turns at once across different agent ids.
  `--codex-max-parallel-turns` (alias `--codex-parallel`) sets the cap:
  default `8`, range `1..=16`, shared by both runners.

Rules:

- Per-agent single flight: at most one in-flight turn per `agent_id`. This
  keeps session use in order (especially `codex-reply`) and thread ids
  deterministic.
- Global cap: in-flight turns across all agents never exceed the cap
  (minimum `1`).
- Dispatch fairness: both runtimes skip queued turns whose agent is already
  busy, so other agents make progress (round-robin over the queue).

Exec runtime: each in-flight turn is one `codex exec` child process. The
runner starts workers until the cap is reached and forwards JSONL stages
and token counts as `TurnStage` and `TokenCount` events.

MCP runtime: one persistent `codex mcp-server` process carries many
in-flight JSON-RPC requests. Each turn sends one `tools/call` request with
a unique `id`. nit keeps an `InFlightMcpTurn` record per request id to
match the final response by `id` and to route `codex/event` notifications
by `_meta.requestId`. The final result yields a `threadId`, stored in
`codex_thread_ids` for chat or `codex_mission_thread_ids` for missions.

Cancellation and timeouts in MCP mode:

- MCP Stop or Reconnect stops the server process, which cancels every
  in-flight request. nit emits `TurnFailed` for each and clears the
  in-flight maps before reconnecting.
- Reconnect keeps saved `threadId` mappings. If Codex later reports
  "Session not found for thread_id ...", nit drops that agent's thread id
  so the next turn starts a fresh thread.
- `NIT_MCP_TURN_TIMEOUT_SECS` sets an optional total timeout and
  `NIT_MCP_TURN_IDLE_TIMEOUT_SECS` an optional idle timeout. Both are off by
  default; `600` is a typical value and `0` disables. When either fires,
  nit restarts the MCP server and fails all in-flight turns. Details:
  `docs/ENVIRONMENT.md`.
- If `codex mcp-server` exits unexpectedly, the runner drops the dead handle
  and retries with a short backoff. You can also press `r` in the MCP tab.

### Turn visibility

`AgentsState.active_turns` tracks each in-flight turn's start time, last
heartbeat, last output, and last stage. Agent Chat shows them in a status
table (`agent`, `stage`, `elapsed`, heartbeat age, output age), plus
pending or queued assigned agents during a swarm mission. To read one
agent's transcript, select it in Agent Ops → Roster and press `Enter`.
`@all <prompt>` targets the mission's agents inside a mission, otherwise
every available lane.

### Swarm orchestration (`@swarm`)

![Swarm DAG orchestrator: planner fans out into parallel proposers, converges through judge and review, integrates, then verifies, with a retry loop from verify back to integrate on gate failure or genome degradation](https://nit.tools/nit-hero-swarm-dag.svg)

User guide: `docs/SWARM.md`. It owns templates, roles, agent selection,
size ceilings, DAG validation and repair, prompt budgets, gates, and abort.

nit has two multi-agent modes:

- `@all <prompt>`: fan-out. Every targeted agent gets the same prompt.
- `@swarm [all|N] [template=lab|parallel|bulk] [mission=general|research|computational-research] <prompt>`:
  an orchestrated workflow where agents get different prompts and roles.

`SwarmRuntime` in `nit-tui` is a small state machine. It creates a mission,
asks a planner agent (the selected Codex or Claude model) for a
machine-readable plan, dispatches tasks to other agents, optionally runs a
verification gate bundle, then asks the planner to synthesize a report.
`@swarm` alone uses `DEFAULT_SWARM_SIZE = 4` agents; `N` may go up to
`MAX_SWARM_SIZE = 256`, clamped by the host's file-descriptor ceiling, and
fewer than 2 agents becomes a normal single-agent send. The `1..=16`
parallel-turn cap bounds concurrent turns, not swarm size.

Planner contract: nit sends the planner your request plus the available
agent ids. The planner returns a short summary and a JSON plan in a fenced
`json` block. Before dispatch, a validator
(`crates/nit-tui/src/swarm/validator.rs`) classifies defects as `MustFix`
or `Advisory`. `MustFix` defects run a repair loop capped at
`REPAIR_RETRY_LIMIT = 2`. `NIT_PLANNER_LEGACY=1` skips both steps.

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

- Single writer: only the `integrator_agent_id` may have `writes=true`, and
  the scheduler dispatches at most one `writes=true` task at a time. Tasks
  that share an agent id run one after another.
- When every task is terminal, nit runs a verifier turn if a gate bundle is
  detected (phase `VERIFY`), then a synthesis turn on the planner with the
  original prompt, every agent's output, and the verification report.
- Without a parseable plan, nit falls back to built-in prompts. A legacy v1
  plan still runs as independent read-only tasks.

Gate bundles: `rust-ci` is auto-detected when a `Cargo.toml` exists in the
workspace root or an ancestor. It runs `cargo fmt --all -- --check`,
`cargo clippy --all-targets --all-features -- -D warnings`, and
`cargo test --workspace --all-features`. `node-ci`, `python-ci`, and
`go-ci` are available through config. Gates run inside an agent turn, so
they respect the agent's sandbox and approval policy; nit never runs shell
commands itself.

```toml
[swarm.gates]
default = "auto"  # or "none", "rust-ci", "node-ci", "python-ci", "go-ci"
```

`@swarm` orchestrates and aggregates; it never merges code itself. The
planner prompt favours one integrator for file edits.

### Shadow agents (`@shadow`, auto-shadow)

Shadows run a fixed propose-a / propose-b, judge, review pipeline behind
one selected agent and prepend the four outputs as advisory context.
`ShadowRuntime` lives in `crates/nit-tui/src/shadow.rs`. Shadow lanes have
`shadow: true` and ids of the form `<base_id>#shadow-<run_id>-<role>`; they
run read-only and stay hidden from the roster, chat, and Ops views. Auto
mode triggers, suppression rules, and lane helpers: `docs/SHADOWS.md`.

### Intake agent

Before every Claude-class chat dispatch, nit runs a hidden one-step intake
turn (`crates/nit-tui/src/intake.rs`) that classifies your intent. On
`write` or `mixed` it appends a `## FILE CHECKLIST (non-negotiable)` block
to the prompt. Codex and Gemini lanes skip it. Any failure falls back to
passthrough of the raw prompt. Disable it with `intake_enabled = false` or
`NIT_INTAKE_DISABLED=1`. Details: `docs/INTAKE.md`.

### Multipane mode (`nit multipane`)

`nit multipane [--backend <model>] [--panes N] [--cwd PATH]` opens a grid of
independent chat panes (default 8, range `1..=32`), each with its own
working directory. `MultipaneState` lives in
`crates/nit-core/src/state/multipane.rs`; when `state.multipane` is `Some`,
the TUI runs `multipane::run_loop` instead of the standard loop. Pane agent
ids use `<base>#mp-pane-NN`, and every pane dispatches through the normal
chat path, so `@swarm`, `@shadow`, `@all`, and `/abort` work per pane.
Details: `docs/MULTIPANE.md`.

### API wiring (CLI to TUI to runner)

- `build_runner_configs` in `crates/nit/src/bootstrap.rs` turns
  `--codex-runtime`, `--codex-sandbox`, `--codex-approval-policy`, and
  `--codex-max-parallel-turns` into a `CodexRunnerConfig` and a
  `ClaudeRunnerConfig`.
- `crates/nit/src/main.rs` passes both into `nit_tui::run(state, theme,
  log_rx, codex_runtime, codex_config, claude_config)`.
- `crates/nit-tui/src/app/runner.rs` forwards them into `run_loop` and
  spawns `CodexRunner::spawn` and `ClaudeRunner::spawn`.
- `crates/nit-tui/src/codex_runner/` applies the config. Exec adds
  `-a <policy>` and `-s <sandbox>` to `codex exec`. MCP forwards
  `approval-policy` and `sandbox` only when the `codex` tool starts a new
  session; `codex-reply` keeps the existing session's settings.

### Genome feedback and auto-retry

When an agent writes files (`AgentBusEvent::FileWrite`), nit re-runs the
genome and parsimony analysis on them and compares tiers with a baseline
taken before the turn. If quality dropped or bloat was detected,
`build_genome_retry_prompt` (`crates/nit-tui/src/app/genome_retry.rs`)
sends a follow-up prompt to the writer, at most `GENOME_RETRY_LIMIT = 3`
times per turn. Tiers, parsimony, and retry rules: `docs/SEEDS.md`.

### Thread and mission context

- Ad-hoc chat tracks the last session id per model so later prompts can
  resume it: `codex_thread_ids` (agent_id to threadId) and
  `claude_session_ids` (agent_id to session_id).
- Missions track ids per mission and per model, so each agent continues
  its own thread: `codex_mission_thread_ids` and
  `claude_mission_session_ids` (mission_id to agent_id to id).
- `TurnCompleted` and `TurnFailed` write the returned id back into the
  matching map.

### MCP status

The MCP tab reflects `AgentsState.mcp`: connection state, endpoint, and
last error. MCP mode turns `codex/event` token notifications into
`TokenCount` events so context estimates stay fresh. `latency_ms` is best
effort, updated on connect and after successful turns. Sandbox and approval
settings pass through:

- `nit --codex-sandbox <read-only|workspace-write|danger-full-access>`
- `nit --codex-approval-policy <untrusted|on-failure|on-request|never>`
  (default `never`)

## Lab Dispatch (Active Lab)

- The CLI accepts `nit` (default GoL), `nit gol`, `nit games`, and
  `nit --lab <gol|games>`.
- `LabId` / `AppKind` in `AppState` selects the active lab and gates
  commands and key bindings.
- The TUI builds lab-specific runtimes. GoL: seed runtime, GoL Petri Dish,
  and the GoL visualizer widget. Games: Games Petri Dish, the games
  dashboard widget, and run and replay tooling.
- Unnamespaced commands (`:run`, `:hide`, and so on) go to the active lab.
  Namespaced commands are accepted only for the active lab.

## Games

Engine internals (kernel vs stepper, deterministic seeding, parallel
logging), the headless CLI, and output formats: `docs/GAMES.md`.

- Config: a `[payoff]` table may hold `matrix`, a 2x2 grid where each cell
  is `[A_payoff, B_payoff]`. When `matrix` is present it is the source of
  truth, and `R/S/T/P` must match it.
- Output: runs live under `runs/games/<timestamp>__seed-<seed>/` with
  `run_summary.json` (schema v2), `definitions.json`, `results.json`,
  `events.ndjson` and `history.ndjson` when enabled, a `config.toml`
  snapshot, and `analysis/` outputs. History logs encode each round from
  player A's view as `0=CC`, `1=CD`, `2=DC`, `3=DD`. `:games analyze`
  writes `analysis__*.json`, `analysis_matches__*.{csv,ndjson}`,
  `analysis_strategies__*.csv`, and `analysis_trajectories__*.csv`.
- Strategies live in `crates/nit-games/src/strategy/`: FSM (Moore machine),
  CA (`strategy/ca/`), and one-sided TM (`strategy/tm/`). Deterministic FSM
  and memory strategies have fast-eval models in
  `crates/nit-games/src/fast_eval.rs` (cycle detection on combined state).
  TMs are deterministic but still run through the simulator. Definitions
  serialize into `definitions.json`, and TM metrics appear in
  `run_summary.json`. Introspection (`crates/nit-games/src/introspection.rs`)
  feeds `nit games inspect`, `nit games graph`, and the `:games inspect`
  popup. FSM enumeration and canonicalization live in
  `crates/nit-games/src/fsm_enum/`.

## Rendering Discipline

- Event-driven, no busy loop. nit redraws when input or an action changes
  state, on a tick for jobs and visualizer animation, and on resize.
- ratatui diffs frames to minimise terminal writes. The cursor shows only
  in editable panes.

## Saving

`save_buffer` in `crates/nit-core/src/io.rs` calls
`nit_utils::fs::write_atomic`:

1. Write to a sibling temp file, `<path>.tmp.<pid>.<counter>`.
2. Flush and fsync.
3. Rename over the destination.

## Error Handling

- Every crate has `#![forbid(unsafe_code)]` except `nit-metal` (Metal GPU
  interop) and `nit-mcp`.
- Terminal restoration uses guard structs and panic hooks to leave raw mode
  and the alternate screen cleanly.

## Syntax Highlighting

`nit-syntax` provides incremental tree-sitter highlighting with a
plain-text fallback. The pipeline is split so semantic tokens (LSP) could
later layer on top of syntactic tokens without touching UI code.

Language coverage: 29 active grammars on tree-sitter 0.25. The single
source of truth is the `LANGUAGES` table in
`crates/nit-core/src/languages.rs` (`LanguageInfo` entries with extensions,
filenames, shebangs, injection aliases, and an `is_code` flag). Every
detection gate reads it: `detect_by_path`, `detect_by_extension`,
`detect_by_filename`, `detect_by_shebang`, `detect_by_injection_alias`,
and `is_supported_extension`, plus the file watcher, the swarm scope
walker, the markdown fenced-code resolver, and the seed encoders. The
grouping below is documentation only.

| Family | Languages |
|---|---|
| Systems | Rust, Go, C, C++, Zig |
| JVM | Java, Kotlin |
| Scripting | Python, JavaScript, TypeScript, Ruby, Lua, PHP, Bash |
| Functional / Symbolic | OCaml, Haskell, Elixir, Lean, Wolfram |
| Mobile / Apple | Swift |
| Markup / Config | Markdown, HTML, CSS, JSON, TOML, YAML, Nix |
| Data / Build | SQL, Makefile |

Dockerfile has a `LANGUAGES` entry, so `Dockerfile`, `Containerfile`,
`Dockerfile.prod`, and `prod.dockerfile` are detected. Its grammar crate is
pinned to an older ABI, so `tree_sitter_language` returns `None` for it and
it renders as plain text.

Pipeline:

- Buffer edits in `nit-core` record byte and point edits and bump the
  buffer version.
- The TUI collects edits, debounces them, and schedules background
  highlight jobs.
- `nit-syntax` parses and runs highlight queries off the UI thread.
- Results are versioned; stale highlights are dropped.
- Render layers: base style, syntax spans, selection, cursor-line
  background.

Fallbacks: if highlighting is disabled or the file exceeds
`highlight.max_file_bytes`, the engine uses a plain-text snapshot with no
spans and reports the status in Gate Monitor.

Config knobs: `highlight.enabled`, `highlight.engine`,
`highlight.debounce_ms`, `highlight.max_file_bytes`,
`highlight.max_spans_per_line`, and `editor.tab_width`.

Adding a language takes three edits:

1. Append a `LanguageInfo` entry to `LANGUAGES` in
   `crates/nit-core/src/languages.rs`. This unlocks extension, filename,
   shebang, and injection-alias matching, the file watcher, the swarm scope
   walker, and the markdown fenced-code resolver.
2. Wire the grammar: add the `tree-sitter-<lang>` crate to
   `crates/nit-syntax/Cargo.toml` (and to `crates/nit-core/Cargo.toml` if
   the seed encoders should score it), add a `LanguageId` variant in
   `crates/nit-syntax/src/language/id.rs`, and add arms in
   `crates/nit-syntax/src/language/grammars.rs` (`tree_sitter_language` and
   `highlights_query`, plus a hand-written `queries/<lang>/highlights.scm`
   if the crate exports no `HIGHLIGHTS_QUERY`). For seed scoring, also add
   a `SeedLanguage` variant and `ts_language` arm in
   `crates/nit-core/src/seed/encoders/lang.rs`.
3. Add a smoke-test row in `crates/nit-syntax/src/tests/engines.rs` with
   at least one keyword.

Queries live in `crates/nit-syntax/queries`. `LanguageRegistry` in
`nit-syntax` is a thin shim that maps table hits to `LanguageId` variants.

## Visualizer (Game of Life)

The Visualizer pane runs a Game of Life simulation seeded from the editor
or scratchpad text. The TUI drives a light tick loop; rule search and
snapshot I/O run on a background worker thread.

- Pipeline: seed text, encoder, GoL simulation (`nit-gol`).
- Rule search scores Life-like rules in the background and reports a
  leaderboard. The live grid always runs one rule at a time, default B3/S23
  (Conway's Life). `Apply` swaps in a single rule, so the step function
  stays deterministic and fast.
- The pane shows rule, generation, alive count, attractor, auto-stop
  policy, and mode; Gate Monitor summarises them. The simulation can
  auto-pause on fixed points or repeats.
- Snapshots go to `gol-snapshots/` in the workspace root as RLE plus JSON
  metadata, deduped by grid hash and pruned by a max file count.

### Seed Encoding System

The seed system turns editor text into a Game of Life genome. Encoders live
in `crates/nit-core/src/seed/` and the runtime in
`crates/nit-tui/src/seed_runtime.rs`. Encoder internals, views, hashing,
seed search, the parsimony rule, and retries are in `docs/SEEDS.md`.

```
text input → encoder → value grid → jitter → density threshold → bit grid → symmetry → target grid
```

| Encoder | Grid Size | Category | Method |
|---|---|---|---|
| `token_spectrum` | 32x32 | AST-driven (default) | One value per AST node from seven semantic role bands |
| `ast_structure` | 32x32 | AST-driven | Node kind weight, depth, and role-band spread per chunk |
| `complexity_field` | 32x32 | AST-driven | Nesting, cognitive complexity, role entropy, and role diversity per row |
| `structural` | 32x32 | Hybrid | Role diversity, depth gradient, role entropy, and role n-gram uniqueness |
| `ascii_bytes` | 32x32 | Byte-level | Text bytes with index mixing and PRNG noise |
| `hilbert_bits` | 32x32 | Byte-level | The same bytes on a Hilbert space-filling curve |
| `lifehash16` | 16x16 | Byte-level | Pure PRNG seeded by the text hash |

| Parameter | Default | Range | Description |
|---|---|---|---|
| `symmetry` | mirror-x | none, mirror-x, mirror-y, rotate-180 | Spatial symmetry; if either mirrored cell is alive, both are |
| `target_density` | 0.31 | 0.08 - 0.7 | Target share of alive cells |
| `padding` | 1 | 0+ | Border padding in cells |
| `placement` | center | center, top-left | Seed position within the grid |
| `jitter` | 0.04 | 0.0 - 0.25 | Random perturbation amplitude |
