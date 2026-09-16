# Living System

nit is built as a living system, not a stateless tool. Agents work over a persistent substrate that records signals, claims, and assumptions, and that sweeps itself at every turn boundary and on a wall-clock metabolic tick. Four coordination roles act over it: worker, observer, arbiter, and resolver. This document is their roster, for contributors. [SUBSTRATE.md](SUBSTRATE.md) owns the data model, events, metabolism, and MCP tools. [SWARM.md](SWARM.md) owns the per-mission task roles.

## Substrate primitives

The substrate lives in `crates/nit-core/src/substrate/` and persists at `.nit/substrate/state.json`. In brief:

- Signals: traces such as `DoneMarker`, `Warning`, `ClaimViolation`, and `InterventionEmitted`. Strength decays per kind; faded signals are pruned.
- Claims: typed write intent (`ExclusiveWrite | SharedRead | AppendOnly | Soft`) with a TTL and a compatibility matrix. Asserted automatically on `FileWrite`; conflicts trigger retries.
- Assumptions: typed read dependencies. A conflicting `FileWrite` invalidates them and sends a `Warning` to the poster.
- Generation counter: advances on `TurnCompleted`, not on the clock. Decay and expiry are generation-relative.
- Metabolism: a wall-clock tick every 10s in Exploration, 5s in Consolidation, or 3s in Defensive mood. It expires claims and assumptions past their TTL, prunes decayed signals, and runs observers and arbiters.
- Mission memory: cross-mission retrieval that shows similar past missions to the planner.
- nit-mcp: three MCP tools (`emit_signal`, `assert_claim`, `assert_assumption`) that let subprocess Codex agents write to the substrate. Requests arrive as `AgentBusEvent::*Request` events whose ids are minted on the main thread. Unix only in v1.

## The four coordination roles

### Worker

Purpose: does the mission work. Reads code, writes diffs, runs tests, produces outputs.

Where it lives: the swarm task roles `propose`, `judge`, `integrate` (the single writer per mission), `review`, `test`, `research`, and `computational-research`. Verification and synthesis are planner phases, not task roles. [SWARM.md](SWARM.md) describes each.

Cadence: turn-driven. Each agent turn is one step of work.

Substrate interactions:

- Signals: nit emits `DoneMarker` on `TurnCompleted` and `Warning` on `TurnFailed`.
- Claims: nit asserts an `ExclusiveWrite` claim on every `FileWrite`, with a TTL of 3 generations times the mood's `claim_ttl_multiplier`.
- Assumptions and direct emission: Codex agents can call the `nit-mcp` tools.

### Observer

Purpose: reads the substrate at tick boundaries, detects patterns, and emits meta-signals. Observers report structural facts; they never act.

Where it lives: `crates/nit-core/src/observers/`. `mod.rs` holds the compile-time `REGISTERED_OBSERVERS` array and `run_all(state)`; each observer is a sibling file.

Cadence: at `TurnCompleted`, after `advance_generation` and pruning, and on every metabolic tick.

Invariants:

- The registry sets `posted_by = "observer:{name}"`, so an observer cannot pose as an agent.
- Emissions are buffered in a `Vec` before they apply. No observer sees another observer's emissions from the same tick.
- Observer signals use `initial_strength = 1.5` (workers default to 1.0), so structural facts outlast worker transients.

Current observers:

- `repeat_failure`: an agent with at least `repeat_failure_threshold` `Warning` signals within 5 generations (3 in Exploration, 2 in Consolidation, 1 in Defensive) gets a `HelpNeeded`. Silent while a recent observer-emitted `HelpNeeded` for that agent exists.
- `global_heat`: more than 100 signals in total emit a `Warning` on `Global`, with a 10-generation cooldown.
- `sparse_plan`: a `planner:<agent>` with at least 3 `Warning` signals carrying `reason = "unresolved_dep"` within 10 generations gets a `HelpNeeded`. Self-silences like `repeat_failure`.

### Arbiter

Purpose: acts on the structural failures observers report (persistent conflicts, repeated failures, broken plans) by redispatching agents with escalated prompts.

Where it lives: `crates/nit-core/src/arbiters/`. `mod.rs` holds:

- `REGISTERED_ARBITERS`: compile-time array.
- `run_all(state)`: collects raw proposals.
- `reduce_proposals(state, raw, retry_limit)`: the policy layer. Cooldown check, per-tick budget, downgrade to `EmitSignalOnly` when the retry budget is exhausted.
- `apply_interventions(state, reduced)`: emits one `InterventionEmitted` signal per intervention and pushes it onto `state.pending_interventions` for nit-tui to drain.

Cadence: after observers at `TurnCompleted` and on every metabolic tick, so it sees observer signals from the same tick.

Guards, all in `reduce_proposals`:

- Per-(arbiter, target) cooldown: 10 generations.
- Per-tick budget: `arbiter_max_per_tick`, which is 1 in Exploration, 2 in Consolidation, and 4 in Defensive.
- Shared retry budget with claim retries: `GENOME_RETRY_LIMIT` in `nit-tui`, mirrored as `ARBITER_RETRY_LIMIT` in `nit-core`, both 3.
- No self-loop: arbiter functions never read `InterventionEmitted` signals. Only the cooldown check does.

Actuation: nit-tui's `drain_pending_interventions` (`crates/nit-tui/src/app/genome_retry.rs`) pops each intervention, dispatches its prompt through `dispatch_agent_prompt`, and consumes one slot of the shared retry budget. It runs right after `drain_pending_claim_retries`, so an agent already retrying is not escalated twice.

Current arbiters:

- `persistent_conflict`: at least 3 mutual `ClaimViolation` signals between an agent pair within 10 generations. Redispatches the lexicographically larger agent with: `ARBITER: you and {other} have conflicted on {paths} {n} times in {w} generations. You must permanently yield this resource for this mission. Choose a different file or coordinate through an explicit artifact.`
- `help_needed`: a `HelpNeeded` from `observer:repeat_failure` within 5 generations. Redispatches the agent with an `ARBITER: you have failed {n} times in the last 5 generations` prompt: stop retrying, state the blocker, downscope.
- `sparse_plan`: a `HelpNeeded` from `observer:sparse_plan` within 10 generations. Redispatches the planner with an `ARBITER: your recent plans repeatedly reference task IDs that don't exist in the DAG` prompt: fix the `deps` entries.

### Resolver

Purpose: the commit boundary, where proposed or intended state becomes durable state.

Where it lives: nit has no explicit `Resolver` type. The role is spread across:

- `AgentBusEvent::apply()` (`crates/nit-core/src/agent_bus/`): resolves event-driven changes such as emitted signals, asserted claims and assumptions, and turn completion.
- `drain_pending_claim_retries` and `drain_pending_interventions` (`crates/nit-tui/src/app/genome_retry.rs`): resolve queued corrective actions into agent dispatches.
- `write_swarm_run_provenance` (`crates/nit-tui/src/app/provenance.rs`): resolves mission completion into durable `.nit/swarm/` artifacts.
- `SubstrateState::save`: resolves the in-memory substrate into `.nit/substrate/state.json`.

What it does not resolve: nit does not own the subprocess file-write path. Codex and Claude agents write files directly, and nit sees the `FileWrite` event afterwards. Claim guarding is advisory: violations trigger retries, not rollbacks.

## Adding a new role

When you add a coordination primitive:

1. Pick the role it extends: observer for read-only pattern detection, arbiter for intervention, worker for a new swarm task type.
2. Mirror the existing framework: a function-pointer type, a compile-time `REGISTERED_*` array, and a policy layer if it acts.
3. Register it in the module's const array.
4. Update this document under the role's current members.
5. Write tests that mirror the existing patterns in `crates/nit-core/src/tests/`.

## Related docs

- [SUBSTRATE.md](SUBSTRATE.md): data model, events, metabolism, observers, arbiters, MCP tools.
- [SUBSTRATE_TESTING.md](SUBSTRATE_TESTING.md): how to run and verify the substrate.
- [SWARM.md](SWARM.md): task roles, DAG orchestration, template selection.
- [ARCHITECTURE.md](ARCHITECTURE.md): overall nit architecture.
- [SEEDS.md](SEEDS.md): the code-as-genome feedback loop for file-structure analysis, a separate framing from the roles here.
- [INTAKE.md](INTAKE.md): the hidden intent classifier that runs in front of chat dispatches.
- [SHADOWS.md](SHADOWS.md): the single-agent propose/judge/review pipeline.
