# Swarm Orchestration (`@swarm`)

This doc explains how to use **Swarm** (multi-agent orchestration) inside nit, how the three
templates behave (`lab`, `parallel`, `bulk`), how to assign roles (especially for bulk), and how to
debug common MCP/runtime issues.

If you’re looking for implementation details, see `docs/ARCHITECTURE.md` (Swarm section).
For a practical checklist, see `docs/SMOKE_TEST.md`. For shortcuts, see `docs/KEYBINDINGS.md`.

> Looking for **single-agent** augmentation instead of a multi-agent DAG? See `docs/SHADOWS.md`
> (`@shadow` + auto-shadow). Shadows run a fixed propose/judge/review pipeline behind one agent and
> are **suppressed inside a swarm mission**, so the two features do not stack.

---

## Quickstart

### 1) Pick a template (recommended)

In **Agent Ops → Roster**, use the template buttons above the models table:

- `Swarm template: [lab] [parallel] [bulk]`
- Shortcuts: `1` = `lab`, `2` = `parallel`, `3` = `bulk`

The selected template is also shown in **Agent Chat** as a small badge (e.g. `t=bulk`).

### 2) Launch

You can launch Swarm in two ways:

1) **Explicit command** (always works):

```text
@swarm [all|N] [template=lab|parallel|bulk] [mission=general|research|computational-research] <prompt>
```

Notes:

- `template=` can also be written as `t=`.
- `mission=` can also be written as `m=`.
- In **Agent Ops → Roster**, you can pin a default template and a default mission preset.
  `mission=...` or `Mission: ...` overrides the roster preset; otherwise the roster preset applies,
  and `auto` falls back to prompt-based mission detection.
- Accepted mission aliases:
  - `general` (aka `default`, `code`, `coding`)
  - `research`
  - `computational-research` (aka `computational`, `computational research`, `comp-research`)
- Accepted template aliases:
  - `parallel` (aka `v1`)
  - `lab` (aka `default`, `v2`)
  - `bulk` (aka `bo`)

Examples:

```text
@swarm template=bulk do a quick repo health check and suggest next steps
@swarm 5 template=parallel triage this UI regression and propose a fix
@swarm all template=lab audit the repo for security footguns
@swarm template=lab mission=research read papers and rank the best strategies for this topic
@swarm 4 t=parallel m=computational-research model competing approaches and compare them
```

2) **Implicit swarm launch** (no `@swarm` needed):

- If your prompt includes a `Template: ...` line, Swarm auto-launches.
  - Examples: `Template: bulk`, `Template: "parallel"`, `- Template: \`lab\``
- If your prompt includes a `Mission: ...` line, Swarm uses it as the mission focus.
  - Examples: `Mission: research`, `Mission: computational-research`
- If your prompt contains “SWARM PLANNER” or “SWARM SYNTHESIZER”, Swarm auto-launches.
- If the roster-selected template is `bulk` or `parallel` and there are at least two Codex agents,
  a plain prompt auto-launches Swarm.
- Without an explicit `mission=...` or `Mission: ...`, nit infers mission focus from the operator
  request. It only enables research roles when the request actually asks for research work (papers,
  web/resources, source survey, modeling/experiments, etc.), not just because the word “research”
  appears in a code-change prompt.

Guardrail: prompts starting with `@` (e.g. `@all ...`) are never auto-converted to Swarm.

Tip: if you temporarily don’t want implicit swarm launches, switch the roster template back to
`lab`.

---

## How Swarm Works (high level)

Swarm is a mission-scoped orchestration loop:

1) **Planning**: a planner agent creates a plan (JSON DAG).
2) **Validation + repair** (default): the parsed plan runs through a deterministic
   validator (`crates/nit-tui/src/swarm/validator.rs`) before dispatch. `MustFix`
   defects trigger a bounded LLM repair loop (`swarm/repair.rs`, capped at
   `REPAIR_RETRY_LIMIT = 2` rounds) that only proceeds while the planner is
   making concrete progress (strict improvement or proper subset). The
   validator + repair pair can be disabled with `NIT_PLANNER_LEGACY=1` for a
   one-release rollback escape hatch — once set, the planner LLM call runs
   once and the parsed plan goes straight to `finalize_plan`.
3) **Execution**: tasks run in parallel when dependencies are satisfied.
4) **Verification** (optional): a verifier runs a detected gate bundle (e.g. `rust-ci`).
5) **Synthesis**: the planner produces a final cohesive report.

Where to watch it:

- **Agent Chat**: shows the classic compact “Working/Queued” table and Swarm metadata.
- **Agent Ops → DAG**: shows the full Swarm DAG (readable card rows, wraps instead of `...`).

---

## Templates

### `template=lab` (default)

Use this for “research lab” workflows where you want:

- multiple read-only proposal/review tasks feeding
- a **single-writer integrator** who is the only one allowed to edit the workspace (`writes=true`).

Key properties:

- Tasks form a dependency DAG (`deps`).
- Multiple tasks may target the same agent id; they run sequentially.
- Only the integrator may have `writes=true` (enforced; non-integrator `writes=true` is forced off).

Typical shape:

- `propose`/`review` tasks for codebase work, or `research`/`computational-research` tasks when
  the mission is external topic/literature/web research
- `integrate` task (single writer, depends on upstream investigation outputs)
- optional review/verification follow-ups

Mission-aware fallback shapes:

- `general`: repo recon -> design options -> integrate/implement -> review
- `research`: source survey -> evidence comparison / ranked strategies -> synthesis -> review
- `computational-research`: source survey -> modeling / experiments / analysis -> synthesis -> review

### `template=parallel`

Use this when tasks are naturally independent:

- one task per agent id (prefer)
- minimal or no dependencies
- maximum parallelism

This is closest to the original “split the work and run in parallel” model.

### `template=bulk` (“bulk orchestration”)

Use this when you want to explore multiple solution candidates and then converge.

Bulk is explicitly designed as:

1) **proposers** (parallel, read-only): multiple independent solution candidates
2) **judge** (read-only): compares proposals and selects the best approach + acceptance criteria
3) **integrator** (single-writer): implements the chosen approach and validates

Bulk plan conventions:

- proposer task ids: `propose-01`, `propose-02`, …
- a `judge` task that depends on **all** proposers
- an `integrate` task assigned to the integrator with `writes=true` depending on `judge`

If the planner returns an invalid bulk plan, nit falls back to a built-in bulk workflow with
proposer “lenses” (minimal diff, correctness, UX, perf, testing, docs, security, …).

---

## Role Assignment (especially for bulk)

Roles exist at two layers:

1) **Planner output**: each task has optional `role` (`propose|judge|research|computational-research|integrate|review|test`).
2) **Roster role hints** (recommended for `parallel`/`bulk`): in **Agent Ops → Roster**, expand a
   model and use the `Role` branch to pick a preferred role (or `All`).
3) **Roster mission preset**: in **Agent Ops → Roster**, set the global swarm mission preset to
   `auto`, `general`, `research`, or `computational-research`.

Notes:

- The roster role hint is passed to the planner as a constraint/preference. It does not by itself
  grant write access; `writes=true` still controls workspace edits.
- `research` means topic exploration: papers, docs, web resources, related ideas, and strategy
  discovery.
- `computational-research` means tool-assisted evidence gathering: targeted searches, calculations,
  experiments, measurements, and comparative analysis.
- `computational-research` also covers broader research-computing work such as simulation,
  modeling, numerical methods, optimization, data/model fitting, pattern or network analysis, and
  reproducible computational workflows across technical domains.
- Mission focus is role-aware:
  - `general` blocks `research` and `computational-research`
  - `research` allows `research`
  - `computational-research` allows both `research` and `computational-research`
- nit only keeps research-role assignments when the operator request is actually research-oriented
  or explicitly asks for those mission-specific roles.
- For `research` and `computational-research` tasks, expect outputs to include sources, methods,
  assumptions, and ranked strategy recommendations.
- Mission-scoped clones do not automatically inherit singleton roles like `integrate` or `judge`
  as actual task roles when the planner omits them; those hints stay planning preferences, not
  implicit assignments.
- `All` means “no role constraint”. It does not spawn extra agents or role-specific worker lanes.
- In `bulk`, if you set an agent’s roster role to `integrate`, nit uses it as the single-writer
  integrator and locks it (planner overrides are ignored).
- You can also mark agents as **priority** in **Agent Ops → Roster** (`[x]` on the model row). For
  `parallel`/`bulk`, priority agents act as an explicit **selection pool**: Swarm will only use
  the priority-marked models for worker lanes. If you request more agents than you selected, nit
  spawns mission-scoped **clones** of the selected models to reach the swarm size. If you select
  *no* priority models, nit clones the currently selected model for the worker lanes.

### Role-based ordering (producer/consumer)

Sometimes roles are **producer/consumer** pairs (e.g. `research` or `computational-research` → `judge`): the consumer task is only
useful *after* producer tasks finish.

Swarm is fundamentally a **DAG scheduler** (`deps`), so nit can express this as dependencies:

- If the plan omits `deps` but tasks/agents have recognizable roles, nit will automatically add
  missing deps so consumer roles run after their producers.
- Default role deps (built-in):
  - `judge` depends on `research` + `computational-research` + `propose`
  - `integrate` depends on `judge` + `research` + `computational-research` + `propose`
  - `review` + `test` depend on `integrate`
- Cycle safety: if adding a role-based dep would introduce a cycle, nit skips that dep and logs a
  `PLAN warning`.

You can override role deps per workspace via `.nit/config.toml`:

```toml
[swarm.role_deps]
judge = ["research", "computational-research", "propose"]
integrate = ["judge", "research", "computational-research", "propose"]
review = ["integrate"]
test = ["integrate"]
```

### DAG validation (cycles / unknown deps)

nit preflights the planner’s task DAG before dispatching.

Default behavior (`strict`):

- Unknown deps (deps that reference missing task ids) cause the swarm run to abort.
- Cycles cause the swarm run to abort.
- You’ll see a `PLAN error` explaining the issue.

Opt-in “best effort” auto-repair:

```toml
[swarm]
dag_validation = "repair"
```

In `repair` mode, nit drops unknown deps and removes deps that would cause cycles, emitting `PLAN warning`s.

### Choosing the planner

The **currently selected Codex lane** becomes the planner/synthesizer.

Practical workflow:

- In Agent Ops → Roster, select the model you want as planner.
- Press `Enter` to focus Agent Chat in that context.
- Send your `@swarm ...` (or implicit) prompt.

### Steering the integrator and judge

For `lab` and `bulk`, nit prefers a single-writer integrator. You can guide the planner by saying:

- “Make `<agent-id>` the integrator (only writer).”
- “Make `<agent-id>` the judge.”

For `bulk`, you can also pick the integrator explicitly via **Agent Ops → Roster → Role → integrate**
(this locks the integrator).

### Best-practice bulk prompt skeleton

```text
Use bulk orchestration.

Assign proposer roles with distinct lenses:
- propose-01: minimal diff / safest change
- propose-02: correctness & edge cases
- propose-03: UX/TUI clarity
- propose-04: testing & verification

Create a judge task that depends on all proposers and outputs:
- decision + rationale
- step-by-step integration plan
- acceptance criteria
- exact verification commands

Integrator must be the only writer (writes=true) and must implement + run the commands.
```

---

## Swarm Size (agent count)

Explicit:

- `@swarm <prompt>` defaults to 4 agents (planner + 3).
- `@swarm N <prompt>` uses N agents total (1–256, FD-bound — see below).
- `@swarm all <prompt>` uses all available Codex/Claude agents in the roster, clamped to the FD ceiling.
- For `parallel`/`bulk`, if the selected pool is smaller than `N`, nit fills the remainder with
  mission-scoped clones of the selected models (or clones of the planner model if none are
  priority-marked).

Implicit launches:

- Default is still 4 agents.
- If `--codex-max-parallel-turns` is set to a non-default value, nit uses it as a *size hint* for
  implicit launches (so “bulk without @swarm” scales to your configured parallelism).
- You can always override by typing `@swarm 3 ...` / `@swarm 5 ...`.

### Static and effective ceilings

- **Static cap**: `MAX_SWARM_SIZE = 256`. Hard upper bound regardless of host.
- **Effective cap**: read at runtime from `RLIMIT_NOFILE` and clamped to the static cap.
  Each in-flight Codex/Claude exec turn opens **4 fds** (stdin + stdout + stderr + tmp out_file),
  plus a baseline reserved for nit (terminal, log, MCP backchannel, etc.). The math:

  `effective = clamp((fd_limit − 32) / 4, 1, 256)`

  | `ulimit -n` | Effective ceiling | Soft warning fires at |
  |---|---|---|
  | **256** (macOS default) | 56 agents | 42 agents (75% of ceiling) |
  | **1024** (Linux default) | 248 agents | 64 agents (`LARGE_SWARM_WARN_THRESHOLD`) |
  | **4096** (recommended) | 256 agents (saturated) | 64 agents |
  | **65536** | 256 agents | 64 agents |

- To lift the ceiling on macOS: `ulimit -n 4096` then restart nit. The soft limit is per-process,
  inherited at fork; bumping it after nit started has no effect on the running process.

### Soft advisories

When you request a swarm, nit pushes context-aware system messages to the mission console — they
inform but never block:

| Trigger | Message shape |
|---|---|
| `@swarm N` where the request was clamped by the FD ceiling | `Requested N agents, started M (effective ceiling M; ulimit -n is …). Bump …` |
| `@swarm N` where `N` exceeds the available roster (no FD clamp) | `Requested N agents, started M (only M eligible agents in the roster).` |
| `@swarm bulk N` with `N > BULK_PRACTICAL_MAX (12)` | Bulk template auto-clamps to 12 with `Bulk template capped at 12 proposers (requested N, started 12). The judge's per-dep budget …` |
| Lightweight planner (haiku / mini / nano / flash) with `N > 20` | `Planner '<id>' is a lightweight model — coherently planning N task assignments may exceed its reasoning depth. Consider a sonnet/opus-tier planner.` |
| Final size ≥ warn threshold and not clamped | `Large swarm (N agents). Each agent spawns a Codex/Claude subprocess (~4 fds, ~50–200 MB each). Verify the host has spare RAM/CPU before continuing.` |

The planner advisory is **independent** — it can fire alongside any of the others.

### DAG view annotation

For tasks that use the full-output dependency budget (`role=judge`, `role=integrate`, or
`writes=true`), the DAG dashboard appends a per-dep budget hint when the per-dep cap drops below
`SWARM_DEP_OUTPUT_MAX_CHARS_FULL` (48 KB) — i.e. when fan-in compresses each dep's payload:

```
↳ budget: ~20KB/dep
↳ budget: ~4KB/dep — shallow (proposer reasoning truncated)
```

The "shallow" warning fires below 8 KB/dep, where each proposer effectively contributes headers
rather than reasoning. This hint is what motivates the bulk-template hard cap at 12.

### UI truncation for large swarms

Three views truncate the displayed agent list when the swarm is large:

- **Roster (Agent Ops)**: per backend group, max 12 visible. Header shows `(visible of total)` —
  e.g. `Codex (12 of 58)`. The currently-selected agent is auto-promoted into the visible window
  so keyboard navigation never lands on a hidden lane. Running agents (`active_turns`) sort first,
  followed by queued, idle, error.
- **Missions tab**: max 8 agent rows per mission, then a `(+N more)` overflow row.
- **Chat-pane breather table**: max 6 visible. Sorted running-first.

`NIT_ROSTER_NO_TRUNCATE=1` disables all three caps when you need to inspect every clone.

---

## Aborting a swarm

When a swarm goes off the rails — wrong direction, runaway tool calls,
hung MCP server, or just "I changed my mind" — five triggers cancel
in-flight work:

| Trigger | Where you press it | Scope |
|---|---|---|
| `/abort` (or `@abort`) | Chat input + Enter | Current mission |
| `/abort all` | Chat input + Enter | Every active swarm + clears both runner queues |
| `/abort <agent-id>` | Chat input + Enter | One agent (surgical strike) |
| **Ctrl+C** | Chat input (must be empty) | Current mission |
| **Esc Esc** (within ~500 ms) | Chat pane focused | Current mission |
| **`x`** | Missions tab, with a mission highlighted | That mission specifically |

### What "abort" actually does

Hard cancel. The swarm runtime moves the mission to `completed_runs`
with `report_status = "ABORTED"`, drains queued turns from the runner
queues, and pushes a system message to the chat:

> ↳ [swarm] Mission aborted by operator. In-flight turns are being
> killed; queued turns dropped.

The runner-side `CancelTurn` then sets a per-turn `AtomicBool`. The
worker thread sees it within ~50 ms (`try_wait` poll interval) and calls
`child.kill()`. The subprocess receives SIGTERM and exits.

### Resolving "the current mission"

`/abort`, Ctrl+C, and Esc-Esc all target whatever the chat is showing —
`state.agents.selected_mission`. There's a fallback: if the selected
mission has already terminated (e.g. you aborted once, then started
another swarm without re-selecting it), the orchestrator falls back to
the most recently started **active** mission. So a second `/abort` after
starting a new swarm always hits the live work, not the stale aborted
one.

### What you'll see after abort

- **Roster status**: agents flip to `IDLE` (not `ERROR`). Operator
  cancellation isn't an error — the bus handler routes the
  `OPERATOR_CANCEL_TURN_MESSAGE` sentinel down a soft path: no alert
  panel, no LAB→WARN promotion, no "Codex failed: …" status banner. The
  Diag tab gets one Info-level entry.
- **Mission status**: `ABORTED` in the Missions tab.
- **Chat-pane breather**: shows `Aborted` (instead of `Done`).
- **DAG view**: non-terminal tasks marked `Skipped`.

### Soft cancel? No.

There is no graceful drain. Abort kills the subprocess immediately so
half-written files may exist on disk if the agent was mid-write. The
substrate's claim lattice will surface inconsistencies on the next
swarm.

### Esc-Esc edge cases

The Esc-Esc detector lives in a thread-local timestamp (chat input
specific). A single Esc still does its existing job (drop selection,
exit insert mode); only a second Esc within 500 ms of the first
triggers abort. The window resets after every abort and naturally
times out, so a stale half-press from yesterday can't fire today.

### Hint strip

Above the chat input, an italic dimmed line surfaces the relevant
triggers:

- When a swarm is mid-flight: `↳ /abort · Ctrl+C · Esc Esc · x in Missions tab`
- When idle: `↳ @swarm <N> t=lab|parallel|bulk <prompt>  ·  /abort to cancel`

The hint shows only when the chat pane has ≥4 rows of headroom above
the input box, and ellipsizes when the terminal is too narrow.

---

## DAG View (Agent Ops → DAG)

The DAG tab is the canonical “Swarm dashboard”.

Goals:

- readable, row-by-row task cards (not a cramped table)
- wraps long titles/fields onto more lines (no right-edge `...` truncation)
- scrollable, with clear separation between tasks and gates

Notes:

- During planning, it shows `Planning: waiting for planner output`.
- Bulk launches auto-switch Agent Ops to the DAG tab.
- Task cards are multi-line for clarity:
  - line 1: `id / state / title` (title wraps)
  - detail lines: agent/role, deps/blocked-on (wraps; no `...` truncation)

---

## Verification Gates

After tasks finish, swarm can optionally dispatch a **verifier agent** that runs
a list of gate commands against the workspace and produces a JSON report. Gates
are how you tell nit *what "done" means* for your project — formatters, linters,
type-checkers, tests, benchmarks, whatever matters.

### Selecting a bundle

By default, nit auto-detects a built-in **gate bundle** from the workspace root:

| Marker file          | Bundle       | Default commands                                                                                                          |
|----------------------|--------------|---------------------------------------------------------------------------------------------------------------------------|
| `Cargo.toml`         | `rust-ci`    | `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features` |
| `package.json`       | `node-ci`    | `npm run lint --if-present`, `npm run build --if-present`, `npm test -- --watch=false --passWithNoTests`                  |
| `pyproject.toml` / `requirements.txt` / `setup.py` / `setup.cfg` | `python-ci`  | `python -m ruff check .`, `python -m mypy .`, `python -m pytest -q` |
| `go.mod`             | `go-ci`      | `gofmt -l .`, `go vet ./...`, `go test ./...`                                                                             |

You can override the auto-detected bundle in `.nit/config.toml`:

```toml
[swarm.gates]
default = "auto"
# Values: "auto" (default), "none", "rust-ci", "node-ci", "python-ci", "go-ci"
```

Set `default = "none"` to skip verification entirely — the swarm will jump
straight from Executing → Synthesizing with no verifier agent.

### Scope-aware Rust commands

The built-in `rust-ci` bundle is **scope-aware**. When the operator's prompt
mentions one or more `crates/<pkg>/` paths, nit derives the set of touched
cargo packages from those paths and substitutes them into the gate commands
using the `{cargo_packages}` placeholder. The verifier then runs targeted
commands instead of the full workspace suite:

```text
Operator prompt: "refactor crates/nit-utils/src/ for clarity"
  ↓ derive_cargo_packages → ["nit-utils"]
  ↓ substituted into the rust-ci templates
Verifier runs:
  cargo fmt -p nit-utils -- --check
  cargo clippy -p nit-utils --all-targets --all-features -- -D warnings
  cargo test -p nit-utils --all-features
```

If the scope spans multiple packages, the templates expand into multiple
`-p` flags (`cargo test -p nit-utils -p nit-core --all-features`).

**Fallback to full workspace** happens when:
- No scope files were declared in the prompt, OR
- Any scope file sits outside `crates/<pkg>/...` (e.g. a workspace-root
  `Cargo.toml` edit, a file under `scripts/`, or `docs/`) — in that case,
  nit can't cleanly map the scope to packages and runs `--workspace` /
  `--all` to stay correct.

The `node-ci`, `python-ci`, and `go-ci` bundles do **not** currently ship
scoped templates — they always run their full-workspace commands. If you
want scoped behavior on those stacks, define custom gates (see below).

### Custom gates — `[[swarm.gates.custom]]`

When the built-in bundles don't match your project's toolchain, define an
explicit gate list in `.nit/config.toml`. Custom gates **fully override** the
auto-detected bundle — the swarm will run exactly what you list, in order.

```toml
[[swarm.gates.custom]]
name = "fmt"
command = "just fmt-check"
scoped_command = "just fmt-check-crates {cargo_packages}"  # optional

[[swarm.gates.custom]]
name = "lint"
command = "just clippy"
scoped_command = "just clippy-crates {cargo_packages}"

[[swarm.gates.custom]]
name = "test"
command = "cargo nextest run --workspace"
scoped_command = "cargo nextest run {cargo_packages}"

[[swarm.gates.custom]]
name = "bench-smoke"
command = "cargo bench --bench smoke -- --quick"
# No scoped_command → this gate always runs the full command, even when
# scope is known.
```

**Fields:**

| Field            | Required | Description                                                                                                                           |
|------------------|----------|---------------------------------------------------------------------------------------------------------------------------------------|
| `name`           | yes      | Short label shown in the gate dashboard and in `report.json` (e.g. `"fmt"`, `"test"`, `"genome"`).                                    |
| `command`        | yes      | Full, workspace-wide command. Used as the fallback when scope cannot be derived cleanly.                                              |
| `scoped_command` | no       | Template run when nit successfully derives cargo packages from the prompt scope. Supports `{cargo_packages}` / `{packages}` substitution (see below). Omit to always run `command`. |

**Placeholders in `scoped_command`:**

| Placeholder        | Expands to                                                                 |
|--------------------|----------------------------------------------------------------------------|
| `{cargo_packages}` | Space-joined `-p <pkg>` flags, e.g. `-p nit-tui -p nit-core`. Best for cargo. |
| `{packages}`       | Plain space-joined package names, e.g. `nit-tui nit-core`. Best for `just`, `make`, scripts. |

If you use a language other than Rust, the simplest pattern is to wrap your
project's scoped operations in scripts or justfile recipes, then reference
them from `scoped_command`. Example for a pnpm workspace:

```toml
[[swarm.gates.custom]]
name = "lint"
command = "pnpm -r lint"
scoped_command = "pnpm --filter {packages} lint"

[[swarm.gates.custom]]
name = "test"
command = "pnpm -r test"
scoped_command = "pnpm --filter {packages} test"
```

> **Note:** `{cargo_packages}` and `{packages}` are only substituted when the
> operator's prompt scope maps cleanly onto the `crates/<pkg>/` layout. For
> non-Rust projects, the current scope derivation won't populate packages,
> so `scoped_command` will never fire unless you also extend scope detection
> (see `derive_cargo_packages` in `crates/nit-tui/src/swarm/dashboard.rs` — open to
> contributions for language-agnostic scope mapping).

### Config resolution order

1. **`[[swarm.gates.custom]]` entries exist** → use them verbatim. The
   detected language bundle is ignored.
2. **`[swarm.gates] default` is set to a specific bundle** (e.g. `"rust-ci"`)
   → use that bundle's built-in commands.
3. **`[swarm.gates] default = "none"`** → skip verification entirely.
4. **Default (`"auto"` or no config)** → auto-detect from workspace marker
   files and use the matching built-in bundle.

Malformed custom-gate entries surface as a `config-error:…` segment in the
mission's "gates:" system message, and nit falls back to the detected bundle
so the swarm still makes progress.

### Genome quality gate

The `genome-quality` gate is independent of the bundle/custom selection: it
runs automatically as a background task when `state.settings.genome.genome_gate_enabled`
is true, evaluating the structural quality of files the integrator touched.
Its results are injected into the verifier's prompt so the verifier can
include a `genome-quality` entry in the report alongside the bundle gates.

### Output artifacts

Every verify pass writes:

- `.nit/swarm/<mission-id>/gates/report.json` — structured `GateReport` with
  per-gate `ok`/`status`/`notes` plus `overall_ok`.
- `.nit/swarm/<mission-id>/gates/output.txt` — the verifier agent's raw
  command output (truncated to `SWARM_VERIFY_MAX_CHARS`).
- `.nit/swarm/<mission-id>/gates/verify.md` — a readable summary combining
  the two above.

---

## Structured Task Artifacts (`swarm_artifacts`)

Tasks may declare expected artifacts in the plan (`artifacts: ["files","diffs","commands",...]`).
If they do, the agent output should include a **JSON code block** describing artifacts.

Supported shape (recommended):

```json
{
  "type": "swarm_artifacts",
  "version": 1,
  "task_id": "integrate",
  "summary": "What changed / why",
  "artifacts": {
    "files": [{ "path": "crates/nit-tui/src/app/mod.rs", "notes": "…" }],
    "diffs": [{ "path": "crates/nit-tui/src/app/mod.rs", "summary": "…" }],
    "commands": [{ "cmd": "cargo test -p nit-tui", "purpose": "…" }],
    "risks": [{ "level": "med", "item": "…", "mitigation": "…" }],
    "notes": ["…"]
  }
}
```

Persistence:

- Swarm data is persisted under `.nit/swarm/<mission-id>/…`
- `Agent Ops → Artifacts` surfaces the parsed task artifacts and verification summary for the
  selected mission
- Each task’s parsed artifacts are written under:
  - `.nit/swarm/<mission-id>/tasks/<task-id>/artifacts.json`
- Task outputs are written under:
  - `.nit/swarm/<mission-id>/tasks/<task-id>/output.md`
- Gate verification outputs live under `.nit/swarm/<mission-id>/gates/` — see
  [Verification Gates → Output artifacts](#output-artifacts) for the file list.

If a task declares artifacts but no parseable JSON block is found, nit emits a mission message like:

- `Swarm artifacts: task 'integrate' declared artifacts but no parseable swarm_artifacts JSON block was found.`

### Serialization format

Swarm artifacts and on-disk task state use **pretty-printed JSON** (`serde_json::to_vec_pretty`).
The format was evaluated against MessagePack, CBOR, custom-binary, and sqlite-json; JSON won on a
2× weighting of debuggability and migration cost over disk size and parse cost.

Reasons the weighting favors JSON:

- **Operator inspection.** `.nit/swarm/<mission>/` trees are routinely read with `cat`, `jq`,
  `grep -r`, and `git diff`. Pretty-print keeps diffs line-readable; binary forks break all of
  these workflows and would require a new `nit dump-artifact` CLI to restore parity.
- **LLM agent self-read.** The on-disk artifact path `.nit/swarm/<mission>/tasks/<id>/artifacts.json`
  is embedded directly in downstream agent prompts (see `crates/nit-tui/src/swarm/artifacts.rs`),
  so swarm successors read predecessor state straight off disk. The wire format is JSON
  (unchangeable — LLMs emit/consume text); forking the disk format from the wire format would
  break this property.
- **Migration cost.** Status quo is zero source edits, zero new dependencies, zero rewriting of
  existing operator `.nit/swarm/` trees.

Decisions that stay rejected under this weighting:

- **Do not switch `to_vec_pretty` → `to_vec`** at the artifact write sites in
  `crates/nit-tui/src/app/provenance.rs`. The ~30–40% byte savings are not worth losing
  line-based `git diff` readability over the swarm tree.
- **Do not rename `artifacts.json` / `run.json` / `summary.json` / `gates/report.json`**.
  These extensions are referenced in (a) the LLM prompt at
  `crates/nit-tui/src/swarm/artifacts.rs`, (b) `crates/nit-tui/src/widgets/artifacts_popup.rs`
  and `crates/nit-tui/src/widgets/agent_ops_view.rs`, and (c) `docs/SWARM.md` +
  `docs/SMOKE_TEST.md`. Any future format/extension change must update all five sites in
  lockstep.

Revisit only if a future requirement provably cannot be served by the JSON tree — e.g.
cross-mission analytical queries over 10⁵+ tasks or full-text search across artifacts. In that
case, layer a derived, rebuildable `.nit/swarm/index.db` (sqlite-json) cache **on top of** the
file tree; source of truth stays in JSON.

---

## MCP + Troubleshooting

### “Stuck in Working …”

The top “Working/Queued” breather stays active if any Codex lane is still marked as having an
in-flight turn. If a lane shows a stage like `Context: …` for a very long time, the underlying MCP
request may be hung.

Quick checks:

- Agent Ops → MCP tab: confirm `CONNECTED`, and check for `last_error`.
- Try `r` (reconnect). This cancels in-flight requests and reinitializes MCP.

### MCP reconnect and context (“Session not found for thread_id …”)

In MCP mode, reconnecting can invalidate the Codex “thread/session id” that nit uses for
continuations (`codex-reply`).

Behavior:

- **MCP reconnect (`r`) preserves** saved Codex thread ids for continuations.
- If Codex later reports `Session not found for thread_id …`, nit **drops the stored thread id for
  that agent** so the next prompt starts a fresh thread (avoids broken “resume” loops).
- **MCP stop (`x`) clears** saved thread ids (next prompt starts a new thread).

If you need more stable “resume” semantics under a flaky MCP transport, run with:

- `--codex-runtime exec`

(exec mode uses `codex exec` processes and can resume sessions without depending on a persistent
MCP server.)

### Optional safety valve: idle timeouts

If you want a “don’t pin the UI forever” safety valve for MCP hangs, you can enable an idle
timeout:

- `NIT_MCP_TURN_IDLE_TIMEOUT_SECS=600`

This is **disabled by default** because cancelling hung turns can force a new session and may
affect continuity for long-running prompts.

---

## Roadmap: toward a self-sustaining “experiments” lab

The next logical steps to make bulk orchestration feel like a durable “lab” (for experiments,
agent collaboration, and accelerating programming/research) are:

- **Runbooks / presets**: one-click (or one-keystroke) bulk/lab workflows (repo health, bug triage,
  perf investigation, refactor plan, ship-a-feature), with editable templates.
- **Explicit role assignment UI**: pick planner/integrator/judge/verifier and proposer “lenses” from
  the roster (and display them prominently in chat + DAG).
- **Persistence + replay**: store plan + outputs + artifacts + diffs + gate reports; support “rerun
  with same plan”, “re-judge”, “re-integrate”, “re-verify”, and compare runs.
- **DAG controls**: retry a single task, skip a task, re-run the judge, and re-run verify without
  restarting the whole swarm mission.
- **Acceptance criteria & scoring**: require `done_when` + verification commands for integrate;
  surface “missing artifacts”, “failed gates”, and “unmet acceptance criteria” clearly.

---

## Example Prompts

Bulk implicit (select `bulk` in roster, then send):

```text
do a quick repo health check and suggest next steps
```

Bulk explicit:

```text
@swarm template=bulk do a quick repo health check and suggest next steps
```

Bulk with roles + lenses:

```text
@swarm template=bulk
Triage this UI regression. Use proposer lenses (minimal diff, correctness, UX clarity, tests).
Judge picks one approach + acceptance criteria + exact commands. Integrator implements.
```

Parallel template line (implicit):

```text
Template: parallel
Investigate why the DAG view is slow; propose 3 fixes; include risks.
```

Explicit agent count override:

```text
@swarm 3 template=bulk scan the repo and propose a small but high-impact cleanup
```
