# Intake Agent

The intake agent is a hidden, single-step LLM preprocessor that runs
before each chat dispatch to classify the operator's intent and decide
whether to append a `## FILE CHECKLIST (non-negotiable)` block to the
prompt.

It replaces the deleted `is_real_work` heuristic in
`crates/nit-tui/src/app/chat_input.rs` — a hard-coded gate that had
known false-negatives on inflected verbs, polite forms, and read-intent
prefixes shadowing later writer verbs.

## Role

Classify, do not execute. The intake agent **never** touches files,
runs commands, or does the work. It only:

1. Reads the operator's raw prompt.
2. Reads the target agent's working directory plus a depth-1 listing.
3. Emits a JSON decision selecting one of `read` / `write` / `mixed` /
   `conversational`.
4. On `write` / `mixed` it APPENDS (never replaces) the FILE CHECKLIST
   block to the raw prompt.

The runtime then dispatches the operator's chosen agent with either the
augmented or raw prompt depending on the intake decision.

## Settings

`Settings::intake_enabled: bool` (in `nit_core::config::Settings`) gates
the entire feature.

- **Default**: `true`. Set `intake_enabled = false` in the global or
  workspace `config.toml` to opt out persistently, or use the
  `NIT_INTAKE_DISABLED=1` env override for a runtime kill switch.
- When `false`, every chat dispatch hands the operator's prompt to the
  runner verbatim. No intake LLM call happens.

## Kill switch

`NIT_INTAKE_DISABLED=1` (or `=true`, case-insensitive) takes precedence
over `Settings::intake_enabled`. Read at the **first** executable line
of `intake::start` (per chat dispatch — one syscall), so live ops can
disable without restarting `nit`. Useful when the intake agent is
producing noisy classifications and you need to fall back to the raw
prompt path immediately.

## Backend selection

Intake's lane provisioning has two layers:

1. **Override hook** — `AgentsState::intake_agent_id: Option<String>`
   lets the operator pin a specific lane id as the intake clone
   source. When set to a claude-class lane id, intake fires for that
   lane regardless of which agent the chat dispatch targets; this is
   the documented escape hatch for running a cheap claude
   preprocessor in front of a non-claude writer.
2. **Backend guard** — when the override is `None`, intake clones the
   **target agent's** lane only when that lane is claude-class. The
   `SYSTEM_PROMPT` is calibrated for haiku-style instruction-following
   (strict fenced-JSON output) and the 30s `INTAKE_TIMEOUT` assumes
   haiku-tier latency; on codex/gemini lanes a real intake turn would
   commonly time out into passthrough while burning a full reasoning
   turn's cost. Skipping is not silent: `intake::start` pushes a
   structured `intake.skipped: backend=<kind> target=<id> reason=non_claude_target`
   Info diag so operators have a breadcrumb for grep / observability.

So with the default settings (no override), targeting:

| Target lane kind | Intake fires? | Diag emitted |
|---|---|---|
| Claude (haiku, sonnet, opus) | yes | none |
| Codex (`gpt-5-codex` etc.) | no — skipped | `intake.skipped: backend=codex …` |
| Gemini | no — skipped | `intake.skipped: backend=gemini …` |
| Mock / Unknown | no — skipped | `intake.skipped: backend=mock …` (or `unknown`) |

To exercise intake on a non-claude target, pin
`state.agents.intake_agent_id = Some("<some-claude-lane-id>")`.

The lane id format is `<base>#intake-<run_id>` — coexists with
`#shadow-`, `#chat-clone-`, `#swarm-` conventions. In multipane the
base id encodes the pane (`<model>#mp-pane-NN`), so the intake lane
becomes `<model>#mp-pane-NN#intake-<run_id>`.

## Read-only contract

Every `RunTurn` dispatched against an intake lane sets `read_only: true`
in the runner config. The check is wire-level:

```
let read_only = crate::shadow::parse_shadow_lane_id(&model).is_some()
    || crate::intake::parse_intake_lane_id(&model).is_some();
```

For Claude this maps to `--allowedTools Read,Glob,Grep` only (no Write,
Edit, MultiEdit, NotebookEdit, Bash). For Codex this maps to a
read-only sandbox. The classifier role and the runner-level tool
restriction are paired guards: the system prompt asks for JSON-only
output AND the toolchain refuses any write/exec attempt. If a future
intake variant legitimately needs to scaffold a file before dispatch,
both the prompt and the read-only predicate must change in lockstep.

## Failure modes

Each non-success path falls back to **passthrough**: the operator's raw
prompt is dispatched as-is. The operator never sees an error banner.

| Failure | Diag level | Diag source | Description |
|---|---|---|---|
| **Timeout** (30s deadline) | `Warn` | `intake.timeout` / `intake.turn_failed` | The intake turn did not return within 30s. Driven by `intake::tick_timeout` from the main app loop; the runner's eventual `TurnFailed` (after `CancelTurn`) completes the resume. **Warn** (promoted from Info): the deferred operator dispatch is wedged on this event and the chat console suppresses Info by default — promoting to Warn means a 30s wedge stays visible. |
| **JSON parse failure** | `Info` | `intake.parse_failed` | The intake reply was not a valid fenced ` ```json ` block, missing `augmented_prompt`, or had a non-string body. |
| **Prefix violation** | `Warn` | `intake.prefix_violation` | The intake's `augmented_prompt` did NOT start with the operator's verbatim raw prompt, did not begin its augmentation with a newline, or omitted the literal `## FILE CHECKLIST (non-negotiable)` marker. **Warn** (not Info) because this is the load-bearing guard for `prompts_leak_test.rs` — a regression here lets every leak token reach the runner. |
| **Runner failure** (TurnFailed) | `Warn` | `intake.turn_failed` | The intake runner exited non-zero, OOMed, or was operator-cancelled. Indistinguishable from timeout from the resume's perspective. |
| **Backend skip** (non-claude target) | `Info` | `intake.skipped` | The selected target is not a claude-class lane and `intake_agent_id` is unset. The operator's prompt dispatches verbatim with no LLM round-trip. See **Backend selection**. |
| **Failed dispatch** (dead runner channel) | `Warn` | `dispatch` (from chokepoint) | `dispatch_agent_prompt` could not enqueue the intake turn. The synthetic intake lane is torn down and the operator's prompt falls through to the regular dispatch path so the chat is not wedged for 30s. |

## Passthrough fallback semantics

On any failure path, `intake::handle_event_outcome` returns
`IntakeResume { prompt: <raw>, .. }`. The event-drain replays the
deferred dispatch through the same code path the operator's prompt
would have hit if `intake_enabled` were false:

- Honors `force_new` (creates a clone if the target family is busy).
- Honors `is_agent_busy` (enqueues if the target is mid-turn).
- Carries the original `mission_id`, `prompt_msg_idx`, and `channel`.

The intake lane itself is torn down (active turns, queued turns,
runtime metadata) before the resume fires — same shape as
`shadow::cleanup_shadow_lanes`.

## Operator interaction with `/abort`

`/abort` (and its sibling triggers — `@abort`, Ctrl+C with empty input,
Esc-Esc, mission `x`) routes through `chat_input::handle_abort`, which
calls `intake::cancel_pending_intake` for `Current`, `All`, and matching
`Agent(<lane>)` scopes. The deferred dispatch is **dropped** — the
operator explicitly cancelled and a stale resume must not fire.

The intake lane's runner receives a `CancelTurn` so the actual
subprocess is reaped, mirroring how `shadow::abort_run` handles
in-flight shadow runs.

## Cross-references

- [Shadow agents](SHADOWS.md) — sibling pipeline for advisory
  proposers / judges / reviewers. Intake mirrors shadow's lane
  lifecycle and stash-then-resume pattern, but runs only one stage.
- [Swarm](SWARM.md) — the `@swarm` family is the alternative to intake
  for multi-agent dispatch. Intake skips for swarm missions, swarm
  followups, broadcasts, `@new`, `@queue`, and shadow-handled prompts.
- `crates/nit-tui/src/intake.rs` — module source.
- `crates/nit-tui/src/tests/intake.rs` — spec tests covering each
  intent class, parse failure, timeout, prefix violation,
  intake-disabled passthrough, per-pane cwd plumbing, the backend
  guard (codex/gemini skip with `intake.skipped` diag, claude
  override path), the `NIT_INTAKE_DISABLED` kill switch, the
  intake-lane `read_only` parser contract, and the failed-dispatch
  cleanup path.
