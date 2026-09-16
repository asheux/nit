# Intake Agent

The intake agent is a hidden, single-step LLM call that runs before each chat dispatch. It classifies your prompt and decides whether to append a `## FILE CHECKLIST (non-negotiable)` block before the prompt reaches the agent you chose. This doc covers what intake does, how to turn it off, which backends it runs on, its read-only contract, and how it fails safe.

## What it does

Intake classifies. It never executes. It does not touch files, run commands, or do the work. It only:

1. Reads your raw prompt.
2. Reads the target agent's working directory plus a depth-1 listing of up to 50 entries.
3. Emits a JSON decision with one intent: `read`, `write`, `mixed`, or `conversational`.
4. On `write` or `mixed`, appends the FILE CHECKLIST block to the raw prompt. It never replaces or rewrites your words.

nit then dispatches the agent you chose with either the augmented or the raw prompt.

## Settings and kill switch

`Settings::intake_enabled` in `nit_core::config::Settings` gates the whole feature. The default is `true`. Set `intake_enabled = false` in the global or workspace `config.toml` to opt out. When it is `false`, every chat dispatch hands your prompt to the runner unchanged and no intake call happens.

`NIT_INTAKE_DISABLED=1` (or `=true`, case-insensitive) is a runtime kill switch that takes precedence over `intake_enabled`. nit reads it at the first line of `intake::start` on every chat dispatch, so you can disable intake without restarting nit. Use it when intake produces noisy classifications and you want the raw prompt path right away.

## Backend selection

Intake picks its lane in two layers:

1. Override hook: `AgentsState::intake_agent_id: Option<String>` pins a lane id as the intake clone source. When it names a claude-class lane, intake runs for that lane no matter which agent the dispatch targets. This is how you run a cheap Claude preprocessor in front of a non-Claude writer.
2. Backend guard: when the override is `None`, intake clones the target agent's lane only if that lane is claude-class. The `SYSTEM_PROMPT` is tuned for haiku-style instruction following with fenced JSON output, and the 30s `INTAKE_TIMEOUT` assumes haiku-tier latency. On codex or gemini lanes a real intake turn would often time out into passthrough while burning a full reasoning turn. The skip is not silent: `intake::start` pushes an Info diag, `intake.skipped: backend=<kind> target=<id> reason=non_claude_target`.

With default settings and no override:

| Target lane kind | Intake fires? | Diag emitted |
|---|---|---|
| Claude (haiku, sonnet, opus) | yes | none |
| Codex (`gpt-5-codex` and so on) | no, skipped | `intake.skipped: backend=codex ...` |
| Gemini | no, skipped | `intake.skipped: backend=gemini ...` |
| Mock / Unknown | no, skipped | `intake.skipped: backend=mock ...` (or `unknown`) |

To run intake on a non-Claude target, set `state.agents.intake_agent_id = Some("<some-claude-lane-id>")`.

## Lane id format

Intake lanes use `<base>#intake-<run_id>`, alongside the `#shadow-`, `#chat-clone-`, and `#swarm-` conventions. In multipane the base id encodes the pane (`<model>#mp-pane-NN`), so the intake lane becomes `<model>#mp-pane-NN#intake-<run_id>`.

## Read-only contract

Every `RunTurn` dispatched to an intake lane sets `read_only: true` in the runner config. The check happens at the wire level:

```rust
let read_only = crate::shadow::parse_shadow_lane_id(&model).is_some()
    || crate::intake::parse_intake_lane_id(&model).is_some();
```

For Claude this means `--allowedTools Read,Glob,Grep` only: no Write, Edit, MultiEdit, NotebookEdit, or Bash. For Codex it means a read-only sandbox. The system prompt asks for JSON-only output and the toolchain refuses any write or exec attempt, so the two guards back each other up. If a future intake variant needs to scaffold a file before dispatch, the prompt and the read-only predicate must change together.

## Failure modes

Every non-success path falls back to passthrough: nit dispatches your raw prompt as-is. You never see an error banner.

| Failure | Diag level | Diag source | What happened |
|---|---|---|---|
| Timeout (30s deadline) | `Warn` | `intake.timeout` / `intake.turn_failed` | The intake turn did not return within 30s. `intake::tick_timeout` fires from the main app loop, and the runner's `TurnFailed` after `CancelTurn` completes the resume. Warn keeps the wedge visible, because the chat console hides Info by default. |
| JSON parse failure | `Info` | `intake.parse_failed` | The reply was not a valid fenced `json` block, lacked `augmented_prompt`, or had a non-string body. |
| Prefix violation | `Warn` | `intake.prefix_violation` | `augmented_prompt` did not start with your raw prompt, did not begin its addition with a newline, or omitted the `## FILE CHECKLIST (non-negotiable)` marker. `prompts_leak_test.rs` depends on this guard. |
| Runner failure (`TurnFailed`) | `Warn` | `intake.turn_failed` | The intake runner exited non-zero, ran out of memory, or was cancelled. The resume handles it like a timeout. |
| Backend skip (non-Claude target) | `Info` | `intake.skipped` | The target is not claude-class and `intake_agent_id` is unset. Your prompt dispatches unchanged with no LLM round trip. |
| Failed dispatch (dead runner channel) | `Warn` | `dispatch` | `dispatch_agent_prompt` could not enqueue the intake turn. nit tears down the intake lane and your prompt falls through to the regular dispatch path, so the chat is not stuck for 30s. |

## Passthrough semantics

On any failure, `intake::handle_event_outcome` returns `IntakeResume { prompt: <raw>, .. }`. The event drain replays the deferred dispatch through the same path your prompt would take with `intake_enabled = false`:

- honors `force_new`, so a busy target family gets a clone
- honors `is_agent_busy`, so a mid-turn target enqueues
- carries the original `mission_id`, `prompt_msg_idx`, and `channel`

nit tears down the intake lane, including its active turns, queued turns, and runtime metadata, before the resume fires. This mirrors `shadow::cleanup_shadow_lanes`.

## `/abort`

`/abort` and its siblings route through `chat_input::handle_abort`, which calls `intake::cancel_pending_intake` for the `Current`, `All`, and matching `Agent(<lane>)` scopes. The siblings are `@abort`, Ctrl+C with empty input, Esc Esc, and `x` on a highlighted mission. The deferred dispatch is dropped: you cancelled, so a stale resume must not fire. The intake lane's runner receives a `CancelTurn` so the subprocess is reaped, as `shadow::abort_run` does for shadow runs.

## Cross-references

- [Shadow agents](SHADOWS.md): the sibling pipeline of proposers, judge, and reviewer. Intake mirrors shadow's lane lifecycle and stash-then-resume pattern but runs a single stage.
- [Swarm](SWARM.md): the `@swarm` family is the multi-agent alternative. Intake skips swarm missions, swarm follow-ups, broadcasts, `@new`, `@queue`, and shadow-handled prompts.
- `crates/nit-tui/src/intake.rs`: module source.
- `crates/nit-tui/src/tests/intake.rs`: tests for each intent class, parse failure, timeout, prefix violation, intake-disabled passthrough, per-pane cwd plumbing, the backend guard, the `NIT_INTAKE_DISABLED` kill switch, the read-only lane parser contract, and failed-dispatch cleanup.
