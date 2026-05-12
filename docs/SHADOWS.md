# Shadow Agents

Shadow agents are hidden support agents that augment a **single selected
agent** with richer context before it answers the user's prompt. Unlike
`@swarm`, which plans a DAG of roles across the roster, shadows run a small
fixed pipeline behind the scenes for **one** agent.

## Pipeline

```
                     user prompt
                          |
                          v
         +----------------+----------------+
         |                |                |
     propose-a        propose-b
         |                |
         +----->  judge  <+
                   |
                 review
                   |
                   v
            main agent (user-selected)
                   |
                   v
             final response
```

1. **propose-a** and **propose-b** each draft an independent candidate
   approach in parallel. They do not see each other's work.
2. **judge** compares the two proposals and either picks the stronger one or
   synthesizes a hybrid plan.
3. **review** stress-tests the judged plan — hunts for edge cases, missed
   files, broken assumptions.
4. The **main agent** (the one the user selected in the roster) then runs
   with all four shadow outputs prepended to the original prompt as advisory
   context, and produces the final answer.

## When shadows activate

Shadows run in one of two modes:

- **Explicit**: the user types `@shadow <prompt>`. This always activates
  the pipeline, regardless of prompt length or keywords.
- **Auto**: for heavy prompts — those longer than 500 characters, or
  containing any of `refactor`, `migrate`, `rewrite`, `implement`,
  `overhaul`, `restructure` — the pipeline activates automatically when a
  single agent is selected.

Auto mode is suppressed for:

- `@swarm`, `@all`, `@new`, `@queue` (and its alias `@q`) — these prefixes
  take precedence.
- Swarm-mission followups — shadows don't run inside an active swarm.
- Broadcasts — shadows augment one agent, not a fan-out.

## UI behaviour

- Shadow lanes are created in the roster with `lane.shadow = true`. The
  roster panel and agent-chat pane both filter them out, so the user sees
  only the main agent's work.
- While shadows run, the breather above the chat surfaces the current
  stage: `Proposing ...`, `Judging ...`, `Reviewing ...`, or
  `Finalizing ...`.
- Shadow messages never appear in the chat — only the main agent's
  response after the pipeline completes.
- Shadow lanes are torn down automatically when the main agent completes
  its turn, or if any shadow turn fails (in which case the main agent is
  re-dispatched with the unaugmented prompt, as a graceful fallback).

## Concurrency

Only one shadow run can be active per main agent at a time. If a run is
already in flight, new prompts to the same agent go through the normal
queueing logic. Different main agents can have independent shadow runs in
parallel.

## Module map

- `crates/nit-tui/src/shadow.rs` — `ShadowRuntime`, pipeline stages,
  `parse_shadow_command`, `should_auto_enable_shadows`,
  prompt builders for each stage.
- `crates/nit-tui/src/app/chat_input.rs` — detects `@shadow` / auto, calls
  `ShadowRuntime::start`.
- `crates/nit-tui/src/app/mod.rs` — forwards `TurnCompleted` / `TurnFailed`
  events into `ShadowRuntime::handle_event_outcome`.
- `crates/nit-tui/src/widgets/agent_console_view/` — derives the stage
  label from live state via `shadow_stage_label_from_state` and hides
  shadow-agent messages from the chat view.
- `crates/nit-tui/src/tests/shadow.rs` — DAG integration tests.

## Lane id format

Shadow clones use the id pattern:

```
<base_id>#shadow-<run_id>-<role>
```

e.g. `codex-main#shadow-01-propose-a`. Parse back with
`shadow::parse_shadow_lane_id`.

## Tuning the auto-heuristic

`should_auto_enable_shadows` lives in `shadow.rs` and is intentionally
conservative — we don't want short questions to spawn four extra agents.
The length threshold and keyword list are both straightforward to tune
when real usage data suggests either false positives or misses.
