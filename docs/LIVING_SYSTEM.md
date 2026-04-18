# Living System — Coordination Roles

nit is designed as a living system, not a stateless tool. Agents operate on a persistent **substrate** that records coordination state — signals, claims, assumptions — and sweeps itself on every turn boundary plus a wall-clock metabolic tick. Four coordination roles act over this substrate.

This document is the roster. For the per-mission **task-role** catalog (what an agent *does* inside one swarm run — `propose`, `integrate`, `review`, etc.), see [`SWARM.md`](SWARM.md).

---

## Substrate primitives (context)

The substrate lives in `crates/nit-core/src/substrate.rs` and persists at `.nit/substrate/state.json`. Key primitives:

- **Signals** — stigmergic traces (`DoneMarker`, `Warning`, `ClaimViolation`, `InterventionEmitted`, etc.). Strength decays per-kind; pruned below threshold.
- **Claims** — typed write-intent (`ExclusiveWrite | SharedRead | AppendOnly | Soft`) with TTL and a compatibility matrix. Auto-asserted on `FileWrite`; conflicts trigger retries.
- **Assumptions** — typed read-dependencies. Auto-invalidated when a conflicting `FileWrite` lands; a Warning signal is emitted toward the assumption's poster.
- **Generation counter** — advances on `TurnCompleted`, not wall-clock. Decay and expiry are generation-relative.
- **Metabolism** — wall-clock tick every 5s (`crates/nit-core/src/metabolism.rs`). Expires past-TTL claims/assumptions, prunes decayed signals, runs observers and arbiters.
- **Mission memory** — cross-mission retrieval (`crates/nit-core/src/mission_memory.rs`). Surfaces past similar missions to the planner.
- **nit-mcp — deliberate agent agency** — `crates/nit-mcp/` exposes three MCP tools (`emit_signal`, `assert_claim`, `assert_assumption`) that let subprocess Codex agents write directly into the substrate. nit-tui binds a per-process UDS listener; `codex mcp-server` is launched with a `-c mcp_servers.nit=...` override so the model can discover the nit tool set. Requests arrive as `AgentBusEvent::*Request` variants whose ids are minted atomically on the main thread (external processes never touch the counters). Unix-only in v1.

---

## The four coordination roles

### Worker

**Purpose:** performs the actual mission work — reads code, writes diffs, runs tests, synthesizes outputs. Workers *do* things.

**Where it lives:** swarm task roles. See [`SWARM.md`](SWARM.md). Current task roles:

- `propose` — survey and plan (read-only)
- `judge` — compare proposals; select or synthesize
- `integrate` — apply edits (single-writer per mission)
- `review` — audit integrated code; flag risks
- `test` — run gates (`cargo test`, `clippy`, language-specific CI)
- `research` / `computational-research` — exploration missions
- `synthesizer` — produce final mission summary
- `verifier` — implicit post-integration gate check

**Cadence:** turn-driven. Each agent turn is a step of work.

**Substrate interactions:**
- Signals: auto-emits `DoneMarker` on `TurnCompleted`, `Warning` on `TurnFailed` (via runtime derivation — see `crates/nit-core/src/agent_bus.rs`).
- Claims: auto-asserts `ExclusiveWrite` on `FileWrite` (TTL 3 gens).
- Assumptions / deliberate emission: available via `nit-mcp` — subprocess Codex agents can call `emit_signal`, `assert_claim`, and `assert_assumption` tools to interact with the substrate explicitly. See the *nit-mcp — deliberate agent agency* entry above under substrate primitives.

---

### Observer

**Purpose:** reads the substrate at tick boundaries, detects patterns, emits meta-signals. Observers *surface* structural facts — they do not actuate.

**Where it lives:** `crates/nit-core/src/observers/`. Framework in `mod.rs` (`REGISTERED_OBSERVERS` compile-time array, `run_all(state)`). Individual observers in sibling files.

**Cadence:** runs at `TurnCompleted` (after `advance_generation` + `prune`) and on every metabolic tick.

**Safety invariants:**
- Registry-enforced `posted_by = "observer:{name}"` — observers cannot self-spoof as agents.
- Emissions are buffered in a `Vec` before application; no observer sees another observer's emissions within the same tick.
- Observer signals use `initial_strength = 1.5` (worker default is 1.0) so structural facts outlast worker transients.

**Current observers:**
- **`repeat_failure`** — ≥2 `Warning` signals from the same agent within 5 generations → emits `HelpNeeded` targeting that agent. Self-silencing if a recent observer-emitted `HelpNeeded` already exists.
- **`global_heat`** — total signal count > 100 → emits `Warning` on `Global`. 10-generation cooldown.

---

### Arbiter

**Purpose:** detects structural failures that observers have surfaced (persistent conflicts, deadlocks, stuck slots) and *actuates* corrective interventions. Arbiters are the first primitive with teeth — they redispatch agents with escalated prompts, not just surface signals.

**Where it lives:** `crates/nit-core/src/arbiters/`. Framework in `mod.rs`:

- `REGISTERED_ARBITERS` — compile-time array.
- `run_all(state)` — collects raw proposals.
- `reduce_proposals(state, raw, retry_limit)` — policy layer: cooldown check, per-tick budget, downgrade to `EmitSignalOnly` when retry budget exhausted.
- `apply_interventions(state, reduced)` — emits `InterventionEmitted` signal per intervention AND pushes onto `state.pending_interventions` for nit-tui to drain.

**Cadence:** runs *after* observers at `TurnCompleted` and on every metabolic tick. Sees observer-emitted signals from the same tick.

**Safety guards (all in `reduce_proposals`):**
- Per-(arbiter, target) cooldown: 10 generations.
- Per-tick budget: 2 interventions max.
- Shared retry budget with claim-retries (via `GENOME_RETRY_LIMIT` in `nit-tui`, mirrored as `ARBITER_RETRY_LIMIT` in `nit-core`).
- No self-loop: arbiters do not read `InterventionEmitted` signals.

**Actuation:** nit-tui's `drain_pending_interventions` pops each intervention, dispatches an escalated prompt via `dispatch_agent_prompt`, and consumes one slot of the shared retry budget. Runs right after `drain_pending_claim_retries` so already-retrying agents aren't doubly escalated.

**Current arbiters:**
- **`persistent_conflict`** — ≥3 mutual `ClaimViolation` signals between an agent pair in 10 generations → `RedispatchWithEscalatedPrompt` on the lexicographically-larger agent with message: *"ARBITER: you and {other} have conflicted on {paths} {n} times in {w} generations. You must permanently yield this resource for this mission."*

---

### Resolver

**Purpose:** the actuation boundary — where proposed/intended state becomes durable state.

**Where it lives:** nit's current architecture does not have an explicit `Resolver` type. The role is distributed:

- **`AgentBusEvent::apply()`** (`crates/nit-core/src/agent_bus.rs`) — resolves event-driven state changes (emit signal, assert claim/assumption, turn completion, etc.).
- **`drain_pending_claim_retries` / `drain_pending_interventions`** (`crates/nit-tui/src/app/mod.rs`) — resolve queued corrective actions into agent dispatches.
- **`write_swarm_run_provenance`** (`crates/nit-tui/src/app/mod.rs`) — resolves mission completion into durable `.nit/swarm/` artifacts.
- **`SubstrateState::save`** — resolves in-memory substrate into `.nit/substrate/state.json`.

**Notable non-resolution:** nit does NOT own the subprocess file-write path. Codex/Claude agents write files directly; nit observes via `FileWrite` events after the fact. Claim-based guarding is advisory (violations trigger retries, not rollbacks).

A future phase may unify the distributed resolver into an explicit type if actuation requires stronger sequencing guarantees.

---

## Adding a new role

When introducing a new coordination primitive:

1. **Decide which role type extends.** Observer for read-only pattern detection; arbiter for actuation; worker for new swarm task types.
2. **Mirror the existing framework.** Fn-pointer type, compile-time `REGISTERED_*` array, optional policy layer for actuators.
3. **Register it** in the relevant module's const array.
4. **Update this document** under the relevant role's "Current" list.
5. **Write tests** mirroring the existing patterns in `crates/nit-core/src/tests/`.

## Related docs

- [`SWARM.md`](SWARM.md) — per-mission task-role catalog, DAG orchestration, template selection.
- [`ARCHITECTURE.md`](ARCHITECTURE.md) — overall nit architecture.
- [`GENOME.md`](GENOME.md) — code-quality feedback loop (separate biological framing for file-structure analysis; distinct from the coordination-role framing here).
