# File checklists for agent requests

On eligible Claude chats, Nit can run a short preparatory turn before the selected agent starts. It supplies your request, the working directory and up to 50 names from the directory's top level. The preparatory agent classifies the request as reading, writing, mixed or conversational. For writing or mixed work, it may append a file checklist to the original prompt.

The checklist gives the main agent candidate files to examine. A top-level listing can miss deeply nested code, generated sources or files described by behavior rather than by name. Name any essential paths and acceptance checks in your request.

## Control the extra turn

Intake is enabled by default. To disable it in your workspace, add this top-level setting to `.nit/config.toml`.

```toml
intake_enabled = false
```

To disable it for one launch, use `NIT_INTAKE_DISABLED=1 nit`. Ordinary Codex chats skip the preparatory call by default. Swarm, shadow, broadcast and explicit queue or clone commands have their own dispatch paths. Intake also skips very short prompts.

## When the checklist is missing

The preparatory turn requests read-only access. Nit accepts an addition only when the original prompt stays intact and the expected checklist heading follows it. A malformed reply, a failed backend call or the 30-second timeout causes the main agent to receive the original request. Look in Agent Ops → Diag for `intake.skipped`, `intake.timeout`, `intake.parse_failed`, `intake.prefix_violation` or `intake.turn_failed`.

Use `/abort` to cancel before dispatch. Cancellation drops the deferred request. If the main agent already started, use its normal cancellation controls. [Agent keybindings](KEYBINDINGS.md)
