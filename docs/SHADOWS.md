# Compare two approaches before an agent acts

In Agent Ops → Roster, select a Codex or Claude model and press Enter. In Agent Chat, ask for two approaches to a decision that matters to your project.

```text
@shadow How should the parser report a truncated record while preserving the caller's byte offset?
```

Nit sends the request to two advisory turns independently. A third compares their proposals, and a fourth reviews the chosen approach for missing files, edge cases and assumptions. The selected agent receives those outputs and your original request. A complete run therefore uses five model turns. The two proposals can run concurrently if the runner has capacity.

The chat status moves through **Proposing**, **Judging**, **Reviewing** and **Finalizing**. Only the selected agent appears in the thread. Open Agent Ops → Artifacts → REPLY to read its answer. The advisory stages request read-only access. The final agent uses its usual permissions.

## When it starts automatically

For an eligible single-agent prompt, Nit also considers shadows when the prompt exceeds 500 characters or contains `refactor`, `migrate`, `rewrite`, `implement`, `overhaul` or `restructure`. Broadcasts, active swarm follow-ups and explicit routing such as `@new` or `@queue` take their own paths. Each selected agent can have one shadow run in progress. Further work for that agent can queue.

If an advisory turn fails, Nit can send the original request directly to the main agent. A final reply alone therefore does not establish that all four advisory stages completed. Review the stage messages when that distinction matters.

For tasks with named owners, dependency tracking and verification reports, see [Swarm missions](SWARM.md). For an experimental comparison of edits in separate worktrees, see [Multiway search](MULTIWAY.md).
