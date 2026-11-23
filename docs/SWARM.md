# Run an agent team on one engineering task

A swarm splits a request into named tasks, waits for prerequisites, and collects the outputs into a mission report. Use it when several people could reasonably investigate different parts of the same problem. A single-agent chat has less setup for a small edit.

In Agent Ops → Roster, select a Codex or Claude model and press Enter. That model plans the work and writes the final summary. Give it a concrete issue and a way to check the outcome.

```text
@swarm 3 template=bulk Fix the parser's incorrect offset on truncated input. Compare a decoder change with a caller-side fix. Preserve offsets for valid input and add a regression test.
```

Open **Agent Ops → DAG** to see the task IDs, owners, dependencies and blocked work. **Agent Chat** shows active and queued turns. After completion, open **Agent Ops → Artifacts** for the selected mission and inspect the diff, task outputs and verification report.

## Choose the team and workflow

| Template | Choose it for | How work is shared |
|---|---|---|
| `lab` (default) | A change that needs investigation, implementation and review | Work follows a dependency graph. One integrator owns the edits. |
| `parallel` | Independent pieces of work or separate research topics | Tasks can run as soon as their prerequisites finish. Give writers distinct paths. |
| `bulk` | Several competing solutions to one problem | Read-only proposers present candidates, a judge compares them, and an integrator implements the selected approach. |

The chat syntax is `@swarm [all|N] [template=...] [mission=...] [budget=ROLE:N] <request>`. The short forms are `t=` and `m=`. The default explicit swarm has four agents including the planner. The eligible roster, template and available file descriptors can reduce the number that starts. The launch message reports any reduction. `--codex-max-parallel-turns` controls how many turns can run at once. Queued agents can belong to the same mission.

In Roster, press **1**, **2** or **3** to select lab, parallel or bulk. Mark preferred models for parallel or bulk with Space. Nit can clone a marked model when more workers are requested. With no priority marks it can clone the selected model. Expanding a model exposes role preferences. A preferred role guides the planner. Bulk also lets you designate the integrator.

A plain request containing a `Template: ...` line can start a swarm. Selecting parallel or bulk in Roster can also route ordinary prompts to a swarm when the roster meets the implicit-launch conditions. Switch back to lab for ordinary single-agent chat.

## Research missions

Use `mission=research` for a source survey and `mission=computational-research` for measurements, models or experiments. The default `general` mission uses engineering roles. State the question, deliverables and required evidence. Specify an output filename or extension if the report must fit another workflow.

```text
@swarm 4 t=parallel m=computational-research Compare parser throughput on ASCII and UTF-8 fixtures. Save the fixture sizes, commands, repeated timings and limitations in benchmark.md.
```

A parallel research plan can assign separate topic files to research workers, have judges compare the survey and written results, and use one integrator for the master index. Bulk research converges on one consolidated output. Review the source, inputs and method behind each reported result. Agreement across agents does not constitute independent replication.

## Plan and task dependencies

The planner supplies a directed acyclic graph. The validator can ask for bounded repairs. Execution starts when tasks have valid prerequisites. A judge typically depends on proposals or research, an integrator on the judge, and review and testing on integration. Tasks assigned to one agent run in sequence.

Strict dependency validation rejects missing IDs and cycles. To allow Nit to drop invalid dependencies with a warning, configure the workspace explicitly. The optional role mapping below changes the inferred order.

```toml
[swarm]
dag_validation = "repair"

[swarm.role_deps]
judge = ["research", "computational-research", "propose"]
integrate = ["judge", "research", "computational-research", "propose"]
review = ["integrate"]
test = ["integrate"]
```

Place these settings in `.nit/config.toml`. A warning about a dropped dependency means a task may run without an upstream result that the planner expected.

## Verification gates

A verifier runs the configured checks and reports their outcomes. Open **DAG** for gate status and **Artifacts** for details. A mission can finish with no gates configured. Check which commands ran and whether they cover the requested behavior before accepting the change.

Nit detects built-in bundles from workspace markers. Rust uses formatting, Clippy and tests. Node uses npm scripts and tests. Python uses Ruff, mypy and pytest. Go uses gofmt, vet and tests. Each command depends on the corresponding tool being available in the verifier's working environment.

To select a bundle, add the following to `.nit/config.toml`.

```toml
[swarm.gates]
default = "rust-ci"
```

Other values are `auto`, `node-ci`, `python-ci`, `go-ci` and `none`. Custom gates take precedence and replace the built-in list.

```toml
[[swarm.gates.custom]]
name = "parser tests"
command = "cargo test --workspace"
scoped_command = "cargo test {cargo_packages}"
```

When the prompt identifies paths under `crates/<pkg>/`, a scoped Rust gate can restrict the check to those packages. `{cargo_packages}` expands to Cargo's `-p <pkg>` arguments. `{packages}` expands to package names. A root-level or otherwise unmapped scope uses the full command. A scoped pass covers only those packages.

The optional `genome-quality` check reports structural and simulation-derived metrics for touched files. Its thresholds can flag a file for closer inspection. They do not establish functional correctness or performance. Inspect the source change and the tests or benchmarks that exercise it. [Code encoders and scores](SEEDS.md)

## Find the evidence

| Workspace path | Contents |
|---|---|
| `.nit/swarm/<mission-id>/summary.json` | Mission summary |
| `tasks/<task-id>/output.md` under the mission directory | Task response |
| `tasks/<task-id>/artifacts.json` | Parsed files, commands, risks and notes supplied by the task |
| `gates/report.json` | Per-gate results and overall verification status |
| `gates/output.txt` and `gates/verify.md` | Captured verifier output and readable summary |

A task that declares artifacts places a fenced JSON block in its response. Its top-level fields are `type: "swarm_artifacts"`, `version: 1`, `task_id`, `summary` and `artifacts`. A missing-block message means Nit could not parse the structured handoff. Read the task's Markdown output before relying on the artifact list. Use `nit skills swarm-plan` to plan dependencies, `nit skills swarm-handoff` to interpret task evidence and `nit skills research-experiment` to design a measurable comparison. The full catalog is available in the NitSkills pane and through `nit skills`.

## Control cost and cancel work

The DAG displays a per-dependency budget when upstream outputs compete for space. Large teams can leave less of each proposal for the judge to see. The default prompt ceilings in decimal kilobytes are 480 for integration, 320 for judging, 240 for research, 160 for proposals, 120 for review and 96 for tests or unclassified tasks. These limits measure input bytes. Provider token usage also depends on encoding, conversation history, tool definitions and replies.

Use `budget=integrate:600k` for a per-mission override. Nit can shorten dependency excerpts or the structural-metrics section when a role prompt exceeds its budget. The files and reports remain available for inspection. [Environment settings](ENVIRONMENT.md)

- Enter `/abort` in Agent Chat to cancel the current mission, `/abort <agent-id>` for one agent, or `/abort all` to cancel all. In Missions, select a mission and press **x** to target it directly.
- Cancellation drops queued work and requests termination of active turns. The mission shows **ABORTED** and unfinished tasks can show **Skipped**. Inspect the working tree for edits already made.
- For a stalled Codex turn, check Agent Ops → MCP for the transport, connection status and last error. With a persistent MCP server, **r** reconnects and cancels in-flight requests. **x** stops the server and clears saved thread IDs. A fresh dispatch can use `--codex-runtime exec` on the next launch. [Agent station controls](KEYBINDINGS.md)
