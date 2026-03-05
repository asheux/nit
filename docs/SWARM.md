# Swarm Orchestration (`@swarm`)

This doc explains how to use **Swarm** (multi-agent orchestration) inside nit, how the three
templates behave (`lab`, `parallel`, `bulk`), how to assign roles (especially for bulk), and how to
debug common MCP/runtime issues.

If you’re looking for implementation details, see `docs/ARCHITECTURE.md` (Swarm section).
For a practical checklist, see `docs/SMOKE_TEST.md`. For shortcuts, see `docs/KEYBINDINGS.md`.

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
@swarm [all|N] [template=lab|parallel|bulk] <prompt>
```

Notes:

- `template=` can also be written as `t=`.
- Accepted template aliases:
  - `parallel` (aka `v1`)
  - `lab` (aka `default`, `v2`)
  - `bulk` (aka `bo`)

Examples:

```text
@swarm template=bulk do a quick repo health check and suggest next steps
@swarm 5 template=parallel triage this UI regression and propose a fix
@swarm all template=lab audit the repo for security footguns
```

2) **Implicit swarm launch** (no `@swarm` needed):

- If your prompt includes a `Template: ...` line, Swarm auto-launches.
  - Examples: `Template: bulk`, `Template: "parallel"`, `- Template: \`lab\``
- If your prompt contains “SWARM PLANNER” or “SWARM SYNTHESIZER”, Swarm auto-launches.
- If the roster-selected template is `bulk` or `parallel` and there are at least two Codex agents,
  a plain prompt auto-launches Swarm.

Guardrail: prompts starting with `@` (e.g. `@all ...`) are never auto-converted to Swarm.

Tip: if you temporarily don’t want implicit swarm launches, switch the roster template back to
`lab`.

---

## How Swarm Works (high level)

Swarm is a mission-scoped orchestration loop:

1) **Planning**: a planner agent creates a plan (JSON DAG).
2) **Execution**: tasks run in parallel when dependencies are satisfied.
3) **Verification** (optional): a verifier runs a detected gate bundle (e.g. `rust-ci`).
4) **Synthesis**: the planner produces a final cohesive report.

Where to watch it:

- **Agent Chat**: shows the classic compact “Working/Queued” table and Swarm metadata.
- **Agent Ops → DAG**: shows the full Swarm DAG (readable card rows, wraps instead of `...`).

---

## Templates

### `template=lab` (default)

Use this for “research lab” workflows where you want:

- multiple read-only researchers/reviewers feeding
- a **single-writer integrator** who is the only one allowed to edit the workspace (`writes=true`).

Key properties:

- Tasks form a dependency DAG (`deps`).
- Multiple tasks may target the same agent id; they run sequentially.
- Only the integrator may have `writes=true` (enforced; non-integrator `writes=true` is forced off).

Typical shape:

- `research`/`review` tasks (read-only, parallel)
- `integrate` task (single writer, depends on research outputs)
- optional review/verification follow-ups

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

Notes:

- The roster role hint is passed to the planner as a constraint/preference. It does not by itself
  grant write access; `writes=true` still controls workspace edits.
- In `bulk`, if you set an agent’s roster role to `integrate`, nit uses it as the single-writer
  integrator and locks it (planner overrides are ignored).
- You can also mark agents as **priority** in **Agent Ops → Roster** (`[x]` on the model row). For
  `parallel`/`bulk`, priority agents are passed to the planner as a hint and are preferred when
  selecting a limited swarm size (so they’re more likely to be included).

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

- `@swarm <prompt>` defaults to 4 agents (planner + 3), capped at 16.
- `@swarm N <prompt>` uses N agents total (1–16).
- `@swarm all <prompt>` uses all available Codex agents (cap 16).

Implicit launches:

- Default is still 4 agents.
- If `--codex-max-parallel-turns` is set to a non-default value, nit uses it as a *size hint* for
  implicit launches (so “bulk without @swarm” scales to your configured parallelism).
- You can always override by typing `@swarm 3 ...` / `@swarm 5 ...`.

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

Swarm can optionally run a “gate bundle” after tasks finish (e.g. `rust-ci`).

Gate selection:

- auto-detected from the workspace (e.g. `Cargo.toml` → Rust), or
- overridden via `.nit/config.toml`:

```toml
[swarm.gates]
default = "auto"   # or "none", "rust-ci", "node-ci", "python-ci", "go-ci"
```

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
    "files": [{ "path": "crates/nit-tui/src/app.rs", "notes": "…" }],
    "diffs": [{ "path": "crates/nit-tui/src/app.rs", "summary": "…" }],
    "commands": [{ "cmd": "cargo test -p nit-tui", "purpose": "…" }],
    "risks": [{ "level": "med", "item": "…", "mitigation": "…" }],
    "notes": ["…"]
  }
}
```

Persistence:

- Swarm data is persisted under `.nit/swarm/<mission-id>/…`
- Each task’s parsed artifacts are written under:
  - `.nit/swarm/<mission-id>/tasks/<task-id>/artifacts.json`

If a task declares artifacts but no parseable JSON block is found, nit emits a mission message like:

- `Swarm artifacts: task 'integrate' declared artifacts but no parseable swarm_artifacts JSON block was found.`

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
