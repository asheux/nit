# Swarm Orchestration (`@swarm`)

This doc is the user guide for Swarm, nit's multi-agent orchestration. It covers how to launch a swarm, how the three templates (`lab`, `parallel`, `bulk`) behave, how roles and dependencies work, size limits, prompt budgets, aborting, verification gates, output artifacts, and MCP troubleshooting. It is for people running swarms, not for contributors.

For implementation details, see `docs/ARCHITECTURE.md` (Swarm section). For a practical checklist, see `docs/SMOKE_TEST.md`. For shortcuts, see `docs/KEYBINDINGS.md`.

Want to augment a single agent instead of planning a multi-agent DAG? See `docs/SHADOWS.md` for `@shadow` and auto-shadow. Shadows never run inside a swarm mission, so the two features do not stack.

## Quickstart

### Pick a template

In Agent Ops → Roster, use the template buttons above the models table:

- `Swarm template: [lab] [parallel] [bulk]`
- Shortcuts: `1` = `lab`, `2` = `parallel`, `3` = `bulk`

Agent Chat shows the selected template as a small badge, for example `t=bulk`.

### Launch with `@swarm`

```text
@swarm [all|N] [template=lab|parallel|bulk] [mission=general|research|computational-research] [budget=ROLE:N] <prompt>
```

- `template=` can be written as `t=`, and `mission=` as `m=`.
- `budget=ROLE:N` overrides one role's prompt-budget ceiling for this mission only, for example `budget=integrate:600k`. The `k`/`K` suffix multiplies by 1024. See [Prompt budget tiers](#prompt-budget-tiers).
- In Agent Ops → Roster you can pin a default template and a default mission preset. `mission=...` or a `Mission: ...` line overrides the roster preset. Otherwise the roster preset applies, and `auto` falls back to prompt-based mission detection.

Mission aliases:

- `general` (also `default`, `code`, `coding`)
- `research`
- `computational-research` (also `computational`, `computational research`, `comp-research`)

Template aliases:

- `lab` (also `default`, `v2`)
- `parallel` (also `v1`)
- `bulk` (also `bo`)

Examples:

```text
@swarm template=bulk do a quick repo health check and suggest next steps
@swarm 5 template=parallel triage this UI regression and propose a fix
@swarm all template=lab audit the repo for security footguns
@swarm template=lab mission=research read papers and rank the best strategies for this topic
@swarm 4 t=parallel m=computational-research model competing approaches and compare them
```

### Implicit launch

A plain prompt launches a swarm without `@swarm` when:

- it contains a `Template: ...` line, for example `Template: bulk`, `Template: "parallel"`, or `- Template: \`lab\``;
- it contains `SWARM PLANNER` or `SWARM SYNTHESIZER`;
- the roster template is `bulk` or `parallel` and the roster has at least two Codex agents.

A `Mission: ...` line sets the mission focus, for example `Mission: research` or `Mission: computational-research`. Without `mission=...` or a `Mission:` line, nit infers the mission from your request. It enables research roles only when the request asks for research work such as papers, web resources, source surveys, modeling, or experiments. The word "research" in a code-change prompt is not enough.

Prompts starting with `@` (for example `@all ...`) are never converted to a swarm. To stop implicit launches for a while, switch the roster template back to `lab`.

### Research missions

In `mission=research` and `mission=computational-research`, the producers are the writers. There is no separate writer fleet, and findings always go to files, never only to chat. Each template runs this sequence.

`parallel`:

1. Survey lenses: read-only `research` or `computational-research` tasks. In a computational-research mission, `computational-research` is the default producer.
2. judge-A: dedups the survey and maps each topic to exactly one output file.
3. Writers: `research` or `computational-research` tasks with `writes=true`, each writing its own topic file.
4. judge-B: reconciles the written files and specs the master index.
5. One `integrate` task writes the master index.
6. `review`.

This is the only shape that allows two `role=judge` tasks.

`bulk`:

1. Read-only research lenses.
2. One `judge`.
3. One `integrate` task writes the consolidated output.
4. `review`.

Output format: a file extension you name is honored (`Links.nb` gives `.nb`). Otherwise the planner matches the project convention, `.nb` for a Wolfram project or `.md` for a docs repo, and falls back to Markdown when there is no clear convention.

## How Swarm Works

A swarm is a mission-scoped loop:

1. Planning: a planner agent writes a plan as a JSON DAG.
2. Validation and repair (default): a deterministic validator (`crates/nit-tui/src/swarm/validator.rs`) checks the plan before dispatch. `MustFix` defects start a bounded LLM repair loop (`swarm/repair.rs`), capped at `REPAIR_RETRY_LIMIT = 2` rounds. The loop continues only while the planner makes concrete progress: a strict improvement or a proper subset of the previous defects. `NIT_PLANNER_LEGACY=1` disables the validator and repair loop. The planner call then runs once and the parsed plan goes straight to `finalize_plan`.
3. Execution: tasks run in parallel once their dependencies are satisfied.
4. Verification (optional): a verifier runs a detected gate bundle such as `rust-ci`.
5. Synthesis: the planner writes a final report.

Where to watch it:

- Agent Chat: the compact Working/Queued table and swarm metadata.
- Agent Ops → DAG: the full swarm DAG. See [DAG View](#dag-view).

## Templates

### `lab` (default)

Use `lab` for research-lab workflows: several read-only proposal and review tasks feed one single-writer integrator, the only task allowed to edit the workspace (`writes=true`).

- Tasks form a dependency DAG through `deps`.
- Several tasks may target the same agent id. They run one after another.
- Only the integrator may have `writes=true`. nit forces it off on any other task.

Typical shape:

- `propose` and `review` tasks for codebase work, or `research` and `computational-research` tasks when the mission is topic, literature, or web research
- an `integrate` task (single writer) that depends on the upstream outputs
- optional review or verification follow-ups

Fallback shapes by mission:

- `general`: repo recon -> design options -> integrate/implement -> review
- `research`: source survey -> evidence comparison / ranked strategies -> synthesis -> review
- `computational-research`: source survey -> modeling / experiments / analysis -> synthesis -> review

### `parallel`

Use `parallel` when tasks are independent: one task per agent id, few or no dependencies, maximum parallelism. It is the plain "split the work and run it side by side" model.

### `bulk`

Use `bulk` to explore several solution candidates and then converge:

1. Proposers (parallel, read-only) draft independent solution candidates.
2. A judge (read-only) compares them and picks the best approach plus acceptance criteria.
3. The integrator (single writer) implements the chosen approach and validates it.

Plan conventions:

- proposer task ids `propose-01`, `propose-02`, ...
- a `judge` task that depends on all proposers
- an `integrate` task with `writes=true` that depends on `judge`

If the planner returns an invalid bulk plan, nit falls back to a built-in bulk workflow with proposer lenses: minimal diff, correctness, UX, perf, testing, docs, security, and so on.

## Roles

Roles live in three places:

1. Planner output: each task has an optional `role`, one of `propose`, `judge`, `research`, `computational-research`, `integrate`, `review`, `test`.
2. Roster role hints (recommended for `parallel` and `bulk`): in Agent Ops → Roster, expand a model and use its `Role` branch to pick a preferred role, or `All`.
3. Roster mission preset: in Agent Ops → Roster, set the global mission preset to `auto`, `general`, `research`, or `computational-research`.

How nit uses them:

- A roster role hint is a planner preference. It does not grant write access; `writes=true` still controls workspace edits.
- `All` means no role constraint. It does not spawn extra agents or role-specific lanes.
- `research` means topic exploration: papers, docs, web resources, related ideas, strategy discovery.
- `computational-research` means tool-assisted evidence gathering: targeted searches, calculations, experiments, measurements, comparative analysis. It also covers simulation, modeling, numerical methods, optimization, data and model fitting, pattern or network analysis, and reproducible computational workflows.
- Mission focus filters roles. `general` blocks `research` and `computational-research`. `research` allows `research`. `computational-research` allows both.
- nit keeps research-role assignments only when the request is research work or asks for those roles by name.
- `research` and `computational-research` outputs include sources, methods, assumptions, and ranked strategy recommendations.
- Mission-scoped clones do not inherit singleton roles such as `integrate` or `judge` as task roles when the planner omits them. Those hints stay planning preferences.
- In `bulk`, a roster role of `integrate` makes that agent the single-writer integrator and locks it. Planner overrides are ignored.
- Priority agents form the selection pool for `parallel` and `bulk`. Mark one with `[x]` on its model row in Agent Ops → Roster. Swarm uses only priority-marked models for worker lanes. If you request more agents than you marked, nit spawns mission-scoped clones of the marked models. If you mark none, nit clones the currently selected model.

### Role-based ordering

Some roles are producer and consumer pairs, for example `research` or `computational-research` feeding `judge`. Swarm is a DAG scheduler, so it expresses this as dependencies. If the plan omits `deps` but tasks have recognizable roles, nit adds the missing deps so consumers run after producers.

Default role deps:

- `judge` depends on `research`, `computational-research`, and `propose`
- `integrate` depends on `judge`, `research`, `computational-research`, and `propose`
- `review` and `test` depend on `integrate`

If a role-based dep would create a cycle, nit skips it and logs a `PLAN warning`.

Override the defaults per workspace in `.nit/config.toml`:

```toml
[swarm.role_deps]
judge = ["research", "computational-research", "propose"]
integrate = ["judge", "research", "computational-research", "propose"]
review = ["integrate"]
test = ["integrate"]
```

### DAG validation

nit checks the planner's task DAG before dispatch. The default mode is `strict`:

- deps that reference missing task ids abort the run
- cycles abort the run
- a `PLAN error` explains the problem

Opt in to best-effort repair:

```toml
[swarm]
dag_validation = "repair"
```

In `repair` mode, nit drops unknown deps and any dep that would cause a cycle, and logs a `PLAN warning` for each.

### Choosing the planner

The currently selected Codex or Claude lane becomes the planner and synthesizer. In Agent Ops → Roster, select the model you want, press `Enter` to focus Agent Chat in that context, and send your `@swarm` or implicit prompt.

### Steering the integrator and judge

For `lab` and `bulk`, nit prefers a single-writer integrator. Guide the planner by writing:

- "Make `<agent-id>` the integrator (only writer)."
- "Make `<agent-id>` the judge."

For `bulk`, you can also lock the integrator through Agent Ops → Roster → Role → integrate.

### Bulk prompt skeleton

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

## Swarm Size

- `@swarm <prompt>` uses 4 agents: the planner plus 3.
- `@swarm N <prompt>` uses N agents in total, from 1 to 256, subject to the fd ceiling below.
- `@swarm all <prompt>` uses every available Codex and Claude agent in the roster, clamped to the fd ceiling.
- For `parallel` and `bulk`, if the selected pool is smaller than N, nit fills the rest with mission-scoped clones of the selected models, or of the planner model when none are priority-marked.

Implicit launches also default to 4 agents. If `--codex-max-parallel-turns` is set to a non-default value, nit uses it as the size hint for implicit launches. Typing `@swarm 3 ...` or `@swarm 5 ...` always overrides it.

### Static and effective ceilings

- Static cap: `MAX_SWARM_SIZE = 256`, on any host.
- Effective cap: read at runtime from `RLIMIT_NOFILE` and clamped to the static cap. Each in-flight Codex or Claude exec turn opens 4 fds for stdin, stdout, stderr, and a temp out_file. nit reserves 32 fds for its own terminal, log, and MCP backchannel.

```text
effective = clamp((fd_limit - 32) / 4, 1, 256)
```

| `ulimit -n` | Effective ceiling | Soft warning fires at |
|---|---|---|
| 256 (macOS default) | 56 agents | 42 agents (75% of ceiling) |
| 1024 (Linux default) | 248 agents | 64 agents (`LARGE_SWARM_WARN_THRESHOLD`) |
| 4096 (recommended) | 256 agents (saturated) | 64 agents |
| 65536 | 256 agents | 64 agents |

To lift the ceiling on macOS, run `ulimit -n 4096` and then restart nit. The soft limit is per process and inherited at fork, so raising it after nit started has no effect on the running process.

### Soft advisories

When you request a swarm, nit posts system messages to the mission console. They inform but never block.

| Trigger | Message shape |
|---|---|
| `@swarm N` clamped by the fd ceiling | `Requested N agents, started M (effective ceiling M; ulimit -n is ...). Bump ...` |
| `@swarm N` larger than the eligible roster, no fd clamp | `Requested N agents, started M (only M eligible agents in the roster).` |
| `bulk` with `N > BULK_PRACTICAL_MAX (12)` | nit clamps to 12: `Bulk template capped at 12 proposers (requested N, started 12). The judge's per-dep budget ...` |
| Lightweight planner (haiku, mini, nano, flash) with `N > 20` | `Planner '<id>' is a lightweight model — coherently planning N task assignments may exceed its reasoning depth. Consider re-running with a sonnet/opus-tier planner ...` |
| Final size at or above the warn threshold, not clamped | `Large swarm (N agents). Each agent spawns a Codex/Claude subprocess (~4 fds, ~50–200 MB each). Verify the host has spare RAM/CPU before continuing.` |

On an fd-bound host, where the ceiling is below 64, the large-swarm message instead names the fd limit and the ceiling and suggests `ulimit -n 4096`. The planner advisory is independent and can fire alongside any of the others.

### DAG view budget hint

Tasks that use the full-output dependency budget (`role=judge`, `role=integrate`, or `writes=true`) get a per-dep budget hint in the DAG view when fan-in pushes the per-dep cap below `SWARM_DEP_OUTPUT_MAX_CHARS_FULL` (48 KB):

```text
↳ budget: ~20KB/dep
↳ budget: ~4KB/dep — shallow (proposer reasoning truncated)
```

The "shallow" warning fires below 8 KB per dep, where each proposer contributes headers rather than reasoning. This is why the bulk template caps at 12 proposers.

### UI truncation for large swarms

Three views cap the visible agent list when a swarm is large:

- Roster (Agent Ops): at most 12 per backend group. The header shows `(visible of total)`, for example `Codex (12 of 58)`. The selected agent is always promoted into the visible window, so keyboard navigation never lands on a hidden lane. Running agents sort first, then queued, idle, error.
- Missions tab: at most 8 agent rows per mission, then a `(+N more)` row.
- Chat-pane breather table: at most 6, running first.

`NIT_ROSTER_NO_TRUNCATE=1` disables all three caps when you need to inspect every clone.

## Prompt budget tiers

`wrap_task_prompt` assembles every swarm dispatch, then a role-aware truncation pass (`crates/nit-tui/src/swarm/budgets.rs`) trims it before it ships. This keeps a fan-in task, such as a judge or integrator reading many upstream outputs, from overflowing the model's context window. The pass is on by default; `NIT_PROMPT_TIERS=0` disables it.

### Per-role byte ceilings

| Role | Ceiling |
|------|--------:|
| `integrate` | 480K |
| `judge` | 320K |
| `research` / `computational-research` | 240K |
| `propose` | 160K |
| `review` | 120K |
| `test` | 96K |
| default | 96K |

The ceilings assume Claude's ~200K-token window and reserve about 120K tokens for system framing and tool-use accumulation.

### Three-stage truncation

When a prompt exceeds its role ceiling, nit shrinks it in this order and stops as soon as it fits:

1. Halve each per-dependency payload.
2. Drop proposer payloads, then judge payloads, leaving a one-line breadcrumb for each.
3. Shrink the `## GENOME LANDSCAPE` block.

No stage ever drops the `## FILE CHECKLIST`, the role contract, your request, or the `<SWARM_TASK_COMPLETE>` sign-off.

### Overrides

- Per mission: add `budget=ROLE:N` to the `@swarm` command, for example `@swarm budget=integrate:600k ...`. The `k`/`K` suffix multiplies by 1024. The value is stored on the run and takes precedence over the runtime defaults.
- Per process: `NIT_PROMPT_BUDGET_<ROLE>` sets a byte ceiling for the life of the process. Decimal bytes only, no `k` suffix. `<ROLE>` is one of `INTEGRATE`, `JUDGE`, `PROPOSE`, `REVIEW`, `TEST`, `RESEARCH`, `DEFAULT`.
- Off switch: `NIT_PROMPT_TIERS=0` (or `false`, `no`, `off`) turns the pass into a no-op, so every prompt ships at full size.

See `docs/ENVIRONMENT.md` for the env var details.

## Aborting a swarm

Abort a swarm when it heads the wrong way, when tool calls run away, when an MCP server hangs, or when you change your mind. These triggers cancel in-flight work:

| Trigger | Where you press it | Scope |
|---|---|---|
| `/abort` (or `@abort`) | Chat input + Enter | Current mission |
| `/abort all` | Chat input + Enter | Every active swarm; also clears both runner queues |
| `/abort <agent-id>` | Chat input + Enter | One agent |
| Ctrl+C | Chat input, which must be empty | Current mission |
| Esc Esc (within about 500 ms) | Chat pane focused | Current mission |
| `x` | Missions tab, with a mission highlighted | That mission |

### What abort does

Abort is a hard cancel. The swarm runtime moves the mission to `completed_runs` with `report_status = "ABORTED"`, drains queued turns from the runner queues, and posts a system message to the chat:

> ↳ [swarm] Mission aborted by operator. In-flight turns are being killed; queued turns dropped.

The runner-side `CancelTurn` sets a per-turn `AtomicBool`. The worker thread sees it within about 50 ms (the `try_wait` poll interval) and calls `child.kill()`. The subprocess receives SIGTERM and exits.

There is no soft cancel or graceful drain. If an agent was mid-write, half-written files may be left on disk. The substrate's claim lattice will show the inconsistency on the next swarm.

### Which mission is "current"

`/abort`, Ctrl+C, and Esc Esc target the mission the chat is showing, `state.agents.selected_mission`. If that mission has already ended, for example you aborted once and then started another swarm without re-selecting it, nit falls back to the most recently started active mission. So a second `/abort` after starting a new swarm always hits the live work, not the stale one.

### What you see after abort

- Roster status: agents flip to `IDLE`, not `ERROR`. The bus handler routes the `OPERATOR_CANCEL_TURN_MESSAGE` sentinel down a soft path: no alert panel, no promotion from LAB to WARN, no "Codex failed: ..." status banner. The Diag tab gets one Info-level entry.
- Mission status: `ABORTED` in the Missions tab.
- Chat-pane breather: `Aborted` instead of `Done`.
- DAG view: non-terminal tasks marked `Skipped`.

### Esc Esc details

The chat input keeps a thread-local timestamp of the last Esc. A single Esc still does its normal job (drop selection, exit insert mode). Only a second Esc within 500 ms aborts. The window resets after every abort and times out on its own, so a stale half-press cannot fire later.

### Hint strip

An italic dimmed line above the chat input shows the relevant triggers:

- Swarm in flight: `↳ /abort · Ctrl+C · Esc Esc · x in Missions tab`
- Idle: `↳ @swarm <N> t=lab|parallel|bulk <prompt>  ·  /abort to cancel`

The hint appears only when the chat pane has at least 4 rows of headroom above the input box, and it ellipsizes when the terminal is too narrow.

## DAG View

The DAG tab in Agent Ops is the main swarm dashboard. It shows one readable card per task, wraps long titles and fields instead of truncating with `...`, scrolls, and separates tasks from gates.

- During planning it shows `Planning: waiting for planner output`.
- Bulk launches switch Agent Ops to the DAG tab automatically.
- Each card has line 1 `id / state / title`, then detail lines for agent/role and deps/blocked-on.

## Verification Gates

After the tasks finish, swarm can dispatch a verifier agent that runs a list of gate commands against the workspace and writes a JSON report. Gates are how you tell nit what "done" means for your project: formatters, linters, type-checkers, tests, benchmarks, whatever matters.

### Selecting a bundle

By default nit detects a built-in gate bundle from marker files in the workspace root:

| Marker file | Bundle | Default commands |
|---|---|---|
| `Cargo.toml` | `rust-ci` | `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features` |
| `package.json` | `node-ci` | `npm run lint --if-present`, `npm run build --if-present`, `npm test -- --watch=false --passWithNoTests` |
| `pyproject.toml` / `requirements.txt` / `setup.py` / `setup.cfg` | `python-ci` | `python3 -m ruff check .`, `python3 -m mypy .`, `python3 -m pytest -q` |
| `go.mod` | `go-ci` | `gofmt -l .`, `go vet ./...`, `go test ./...` |

Override the detected bundle in `.nit/config.toml`:

```toml
[swarm.gates]
default = "auto"
# Values: "auto" (default), "none", "rust-ci", "node-ci", "python-ci", "go-ci"
```

`default = "none"` skips verification. The swarm then goes straight from Executing to Synthesizing with no verifier agent.

### Scope-aware Rust commands

The `rust-ci` bundle is scope-aware. When your prompt mentions one or more `crates/<pkg>/` paths, nit derives the touched cargo packages and substitutes them into the gate commands through the `{cargo_packages}` placeholder. The verifier then runs targeted commands instead of the full workspace suite:

```text
Prompt: "refactor crates/nit-utils/src/ for clarity"
  ↓ derive_cargo_packages → ["nit-utils"]
  ↓ substituted into the rust-ci templates
Verifier runs:
  cargo fmt -p nit-utils -- --check
  cargo clippy -p nit-utils --all-targets --all-features -- -D warnings
  cargo test -p nit-utils --all-features
```

A scope that spans several packages expands into several `-p` flags, for example `cargo test -p nit-utils -p nit-core --all-features`.

nit falls back to the full workspace commands when the prompt names no scope files, or when any scope file sits outside `crates/<pkg>/...`, such as the workspace-root `Cargo.toml` or a file under `scripts/` or `docs/`. In that case nit cannot map the scope to packages, so it runs `--workspace` / `--all` to stay correct.

The `node-ci`, `python-ci`, and `go-ci` bundles have no scoped templates and always run their full-workspace commands. For scoped behaviour on those stacks, define custom gates.

### Custom gates

When the built-in bundles do not match your toolchain, list your own gates as `[[swarm.gates.custom]]` entries in `.nit/config.toml`. Custom gates fully replace the detected bundle: the swarm runs exactly what you list, in order.

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
# No scoped_command: this gate always runs the full command, even when
# scope is known.
```

Fields:

| Field | Required | Description |
|---|---|---|
| `name` | yes | Short label shown in the gate dashboard and in `report.json`, for example `"fmt"`, `"test"`, `"genome"`. |
| `command` | yes | Full, workspace-wide command. Also the fallback when scope cannot be derived. |
| `scoped_command` | no | Template used when nit derives cargo packages from the prompt scope. Supports `{cargo_packages}` and `{packages}`. Omit to always run `command`. |

Placeholders in `scoped_command`:

| Placeholder | Expands to |
|---|---|
| `{cargo_packages}` | Space-joined `-p <pkg>` flags, for example `-p nit-tui -p nit-core`. Best for cargo. |
| `{packages}` | Plain space-joined package names, for example `nit-tui nit-core`. Best for `just`, `make`, and scripts. |

For a language other than Rust, wrap your project's scoped operations in scripts or justfile recipes and call them from `scoped_command`. Example for a pnpm workspace:

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

`{cargo_packages}` and `{packages}` are substituted only when the prompt scope maps onto the `crates/<pkg>/` layout. For non-Rust projects the current scope detection finds no packages, so `scoped_command` never fires unless you extend `derive_cargo_packages` in `crates/nit-tui/src/swarm/dashboard.rs`. Contributions for language-agnostic scope mapping are welcome.

### Config resolution order

1. `[[swarm.gates.custom]]` entries exist: use them as written and ignore the detected bundle.
2. `[swarm.gates] default` names a bundle, for example `"rust-ci"`: use that bundle's built-in commands.
3. `[swarm.gates] default = "none"`: skip verification.
4. `"auto"` or no config: detect from workspace marker files and use the matching bundle.

A malformed custom-gate entry shows up as a `config-error:...` segment in the mission's "gates:" system message. nit then falls back to the detected bundle so the swarm still makes progress.

### Genome quality gate

The `genome-quality` gate is independent of bundle and custom selection. When `state.settings.genome.genome_gate_enabled` is true, it runs automatically as a background task and scores the structural quality of the files the integrator touched. Its results go into the verifier's prompt so the report can include a `genome-quality` entry next to the bundle gates.

### Output artifacts

Every verify pass writes:

- `.nit/swarm/<mission-id>/gates/report.json`: the structured `GateReport` with per-gate `ok`, `status`, and `notes`, plus `overall_ok`.
- `.nit/swarm/<mission-id>/gates/output.txt`: the verifier agent's raw command output, truncated to `SWARM_VERIFY_MAX_CHARS`.
- `.nit/swarm/<mission-id>/gates/verify.md`: a readable summary of the two files above.

## Structured Task Artifacts (`swarm_artifacts`)

A plan may declare the artifacts a task should produce, for example `artifacts: ["files","diffs","commands",...]`. The agent's output should then include a JSON code block in this shape:

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

- All swarm data lives under `.nit/swarm/<mission-id>/`.
- Each task's parsed artifacts: `.nit/swarm/<mission-id>/tasks/<task-id>/artifacts.json`.
- Each task's output: `.nit/swarm/<mission-id>/tasks/<task-id>/output.md`.
- Gate outputs: `.nit/swarm/<mission-id>/gates/`. See [Output artifacts](#output-artifacts).
- Agent Ops → Artifacts shows the parsed task artifacts and the verification summary for the selected mission.

If a task declares artifacts but its output has no parseable JSON block, nit posts a mission message:

`Swarm artifacts: task 'integrate' declared artifacts but no parseable swarm_artifacts JSON block was found.`

### Serialization format

Artifacts and on-disk task state are pretty-printed JSON, so you can read them with `cat`, `jq`, and `git diff`, and downstream agents can read predecessor state straight off disk. Do not switch to compact or binary formats. Do not rename `artifacts.json`, `run.json`, `summary.json`, or `gates/report.json`: prompts and the artifacts views reference those names.

## MCP + Troubleshooting

### Stuck in "Working ..."

The Working/Queued breather stays active while any Codex lane still has an in-flight turn. If a lane shows a stage such as `Context: ...` for a very long time, the underlying MCP request may be hung. Quick checks:

- Agent Ops → MCP tab: confirm `CONNECTED` and look for `last_error`.
- Press `r` to reconnect. This cancels in-flight requests and reinitializes MCP.

### MCP reconnect and context

In MCP mode, reconnecting can invalidate the Codex thread or session id that nit uses for continuations (`codex-reply`).

- MCP reconnect (`r`) preserves saved Codex thread ids.
- If Codex later reports `Session not found for thread_id ...`, nit drops the stored thread id for that agent so the next prompt starts a fresh thread instead of looping on a broken resume.
- MCP stop (`x`) clears saved thread ids. The next prompt starts a new thread.

For more stable resume behaviour under a flaky MCP transport, run nit with `--codex-runtime exec`. Exec mode uses `codex exec` processes and can resume sessions without a persistent MCP server.

### Optional safety valve: idle timeouts

To stop an MCP hang from pinning the UI forever, enable an idle timeout:

```text
NIT_MCP_TURN_IDLE_TIMEOUT_SECS=600
```

It is off by default because cancelling a hung turn can force a new session and break continuity for long prompts. The full set of timeout, planner, and budget env vars is in `docs/ENVIRONMENT.md`.

## Planned

- Runbooks and presets: one-keystroke bulk and lab workflows with editable templates, such as repo health, bug triage, perf investigation, refactor plan, and ship a feature.
- Role assignment UI: pick the planner, integrator, judge, verifier, and proposer lenses from the roster, and show them in chat and the DAG.
- Persistence and replay: rerun with the same plan, re-judge, re-integrate, re-verify, and compare runs.
- DAG controls: retry or skip one task, re-run the judge, or re-run verify without restarting the mission.
- Acceptance criteria and scoring: require `done_when` plus verification commands for integrate, and show missing artifacts, failed gates, and unmet criteria.

## Example Prompts

Bulk, implicit. Select `bulk` in the roster, then send:

```text
do a quick repo health check and suggest next steps
```

Bulk with roles and lenses:

```text
@swarm template=bulk
Triage this UI regression. Use proposer lenses (minimal diff, correctness, UX clarity, tests).
Judge picks one approach + acceptance criteria + exact commands. Integrator implements.
```

Parallel through a template line (implicit):

```text
Template: parallel
Investigate why the DAG view is slow; propose 3 fixes; include risks.
```

Explicit agent count:

```text
@swarm 3 template=bulk scan the repo and propose a small but high-impact cleanup
```
