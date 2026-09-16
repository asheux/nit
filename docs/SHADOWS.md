# Shadow Agents

Shadow agents are hidden helpers that give one selected agent richer context before it answers your prompt. Where `@swarm` plans a DAG of roles across the roster, shadows run a small fixed pipeline behind a single agent. This doc covers the pipeline, when it runs, what you see, and where the code lives.

## Pipeline

```text
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

1. `propose-a` and `propose-b` each draft a candidate approach in parallel. They do not see each other's work.
2. `judge` compares the two proposals and either picks the stronger one or builds a hybrid plan.
3. `review` stress-tests the judged plan: edge cases, missed files, broken assumptions.
4. The main agent, the one you selected in the roster, runs with all four shadow outputs prepended to your prompt as advisory context and writes the final answer.

## When shadows run

- Explicit: type `@shadow <prompt>`. This always runs the pipeline, whatever the prompt length or wording.
- Auto: the pipeline runs on its own when a single agent is selected and the prompt is longer than 500 characters or contains any of `refactor`, `migrate`, `rewrite`, `implement`, `overhaul`, `restructure`.

Auto mode is suppressed for:

- `@swarm`, `@all`, `@new`, and `@queue` (alias `@q`). These prefixes take precedence.
- Follow-ups inside an active swarm mission.
- Broadcasts. Shadows augment one agent, not a fan-out.

## What you see

- Shadow lanes are roster lanes with `lane.shadow = true`. The roster panel and the agent-chat pane filter them out, so you see only the main agent's work.
- While shadows run, the breather above the chat shows the current stage: `Proposing ...`, `Judging ...`, `Reviewing ...`, or `Finalizing ...`.
- Shadow messages never appear in the chat. Only the main agent's response shows, after the pipeline completes.
- nit tears the shadow lanes down when the main agent finishes its turn. If any shadow turn fails, nit tears them down too and re-dispatches the main agent with the plain prompt.

## Concurrency

Each main agent can have one shadow run in flight at a time. While one is running, new prompts to that agent go through the normal queueing path. Different main agents can run shadow pipelines in parallel.

## Lane id format

Shadow clones use this id pattern:

```text
<base_id>#shadow-<run_id>-<role>
```

For example `codex-main#shadow-01-propose-a`. `shadow::parse_shadow_lane_id` parses it back.

## Module map

- `crates/nit-tui/src/shadow.rs`: `ShadowRuntime`, the pipeline stages, `parse_shadow_command`, `should_auto_enable_shadows` with its length threshold and keyword list, and the prompt builders for each stage.
- `crates/nit-tui/src/app/chat_input.rs`: detects `@shadow` and auto mode, calls `ShadowRuntime::start`.
- `crates/nit-tui/src/app/mod.rs`: forwards `TurnCompleted` and `TurnFailed` events to `ShadowRuntime::handle_event_outcome`.
- `crates/nit-tui/src/widgets/agent_console_view/`: derives the stage label from live state through `shadow_stage_label_from_state` and hides shadow messages from the chat view.
- `crates/nit-tui/src/tests/shadow.rs`: integration tests for the pipeline.
