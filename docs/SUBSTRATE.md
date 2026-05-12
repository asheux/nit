# Substrate — The Living-System Reference

This document is the architectural reference for nit's living-system layer: the **substrate** and every coordination primitive built over it. It complements:

- [`LIVING_SYSTEM.md`](LIVING_SYSTEM.md) — the coordination role roster (worker / observer / arbiter / resolver).
- [`SUBSTRATE_TESTING.md`](SUBSTRATE_TESTING.md) — how to run tests and verify each feature with concrete examples.
- [`SWARM.md`](SWARM.md) — the per-mission task-role catalog (propose / integrate / judge / ...).
- [`ARCHITECTURE.md`](ARCHITECTURE.md) — overall nit architecture.

---

## 1. Framing

nit's agents operate on a **persistent, typed, stigmergic substrate** that records coordination state across turns, sessions, and missions. The architecture is **layered**:

- **Inside a mission**: the DAG-based swarm scheduler executes tasks (unchanged from pre-substrate nit).
- **Across missions and between turns**: the substrate accumulates signals, claims, assumptions, observations, and mood. Observers read it, arbiters act on it, metabolism sweeps it on a wall-clock tick, mission memory retrieves from it.

The substrate is **never queried directly by subprocess agents** (Codex / Claude) — they interact with it only via:

1. Runtime-derived events: nit observes `TurnCompleted`, `TurnFailed`, `FileWrite` etc. and emits substrate state as a side effect.
2. The `nit-mcp` MCP tool set, for deliberate emission (Codex only in v1).

Design principles:

- **Stigmergy**: coordination happens through environmental traces, not direct messaging.
- **Metabolism**: decay and pruning sweep state forward on a wall-clock tick, independent of user activity.
- **Generation-relative time**: most TTLs and cooldowns count *turns*, not wall-clock seconds. Metabolism advances wall-clock; generation advances only on `TurnCompleted`.
- **Layered roles**: worker (does work), observer (detects patterns), arbiter (intervenes), resolver (commits). Four distinct roles with distinct inputs, powers, and trigger conditions. See `LIVING_SYSTEM.md`.
- **Tolerant persistence**: state on disk is forward- and backward-compatible — missing fields default; corrupt files degrade to empty state; old substrate snapshots load cleanly into new binaries.

---

## 2. Data model

Defined in [`crates/nit-core/src/substrate/`](../crates/nit-core/src/substrate/) (split across `signals.rs`, `claims.rs`, `assumptions.rs`, `mod.rs` for `SubstrateState`).

### 2.1 `SubstrateState`

```rust
pub struct SubstrateState {
    pub generation: u64,
    pub signals: HashMap<SignalId, Signal>,
    pub claims: HashMap<ClaimId, Claim>,
    pub assumptions: HashMap<AssumptionId, Assumption>,
    pub observations: Vec<serde_json::Value>,     // reserved
    pub signal_counter: u64,
    pub claim_counter: u64,
    pub assumption_counter: u64,
    pub mood: Mood,
    pub mood_override_until_gen: u64,
    pub mood_quiet_streak: u32,
}
```

Persisted at `.nit/substrate/state.json` (single JSON object, atomic write via `nit_utils::fs::write_atomic`). All new fields use `#[serde(default)]` so older on-disk snapshots load cleanly.

### 2.2 Signal

```rust
pub struct Signal {
    pub id: SignalId,                     // "{gen}-{posted_by}-{counter}"
    pub kind: SignalKind,
    pub posted_by: String,
    pub posted_at_gen: u64,
    pub target: SignalTarget,
    pub initial_strength: f32,            // default 1.0; observers 1.5; arbiter-emitted 2.0
    pub payload: serde_json::Value,
}
```

`SignalKind`:

| Kind               | Decay per gen | Role                                   |
|--------------------|---------------|----------------------------------------|
| `HelpNeeded`       | 0.5           | Urgent, resolves or fades fast          |
| `Lead`             | 0.7           | Suggestive tip                          |
| `Warning`          | 0.8           | Mid-persistence caution                 |
| `ClaimViolation`   | 0.85          | Write conflict evidence                 |
| `Deadend`          | 0.9           | Peer warning — long-lived               |
| `DoneMarker`       | 0.95          | Durable "this happened" fact            |
| `InterventionEmitted` | 0.9        | Arbiter trace                           |

Effective strength (lazy): `initial * (rate / mood_multiplier)^(current_gen - posted_at_gen)` where `mood_multiplier` is `state.substrate.mood.modulation().signal_decay_multiplier`. Pruned when below `DEFAULT_PRUNE_THRESHOLD = 0.05`.

`SignalTarget`: `File { path }` | `Agent { agent_id }` | `Global`.

### 2.3 Claim

```rust
pub struct Claim {
    pub id: ClaimId,
    pub kind: ClaimKind,
    pub target: ClaimTarget,
    pub claimed_by: String,
    pub claimed_at_gen: u64,
    pub ttl_gens: u64,
    pub rationale: String,
}
```

`ClaimKind`: `ExclusiveWrite` | `SharedRead` | `AppendOnly` | `Soft`.

`ClaimTarget`: `File { path }` | `Region { path, start_line, end_line }` | `Global`.

**Compatibility matrix** (two claims conflict iff their targets overlap AND their kinds are incompatible):

|                 | ExclusiveWrite | SharedRead | AppendOnly | Soft  |
|-----------------|----------------|------------|------------|-------|
| ExclusiveWrite  | ❌             | ❌         | ❌         | ✅    |
| SharedRead      | ❌             | ✅         | ✅         | ✅    |
| AppendOnly      | ❌             | ✅         | ✅         | ✅    |
| Soft            | ✅             | ✅         | ✅         | ✅    |

Target overlap semantics:
- `Global` overlaps with anything.
- `File(p)` + `File(p)` overlap; `File(p1)` + `File(p2)` with `p1 != p2` do not.
- `Region(p, s1, e1)` + `Region(p, s2, e2)` overlap iff `s1 ≤ e2 && s2 ≤ e1`.
- `Region(p, _, _)` + `File(p)` overlap (region is a subset of file).

TTL semantics: a claim is expired iff `current_gen ≥ claimed_at_gen + ttl_gens`. `expire_claims(current_gen)` drops expired entries.

Auto-claim: every `FileWrite` event auto-asserts an `ExclusiveWrite` claim with `ttl_gens = (3 * mood.claim_ttl_multiplier).max(1) as u64`. Conflicts emit `ClaimViolation` signals and queue a `ClaimRetryRequest` on `state.pending_claim_retries`.

### 2.4 Assumption

```rust
pub struct Assumption {
    pub id: AssumptionId,
    pub target: AssumptionTarget,
    pub fact: serde_json::Value,   // opaque — nit doesn't inspect
    pub posted_by: String,
    pub posted_at_gen: u64,
    pub ttl_gens: u64,
    pub rationale: String,
}
```

`AssumptionTarget` has the same shape as `ClaimTarget`. Assumptions have no compatibility lattice (read-vs-read never conflicts) — `assert_assumption` is infallible.

**Auto-invalidation**: every `FileWrite` event removes any non-expired assumption whose target overlaps the written path and emits a `Warning` signal targeting the assumption's **original poster** (not the writer) with the full removed `Assumption` embedded in the payload.

### 2.5 Observations

`Vec<serde_json::Value>` — reserved slot for a future polymorphic observation record type. Not currently written by any primitive.

### 2.6 Mood

Defined in [`crates/nit-core/src/mood.rs`](../crates/nit-core/src/mood.rs).

```rust
pub enum Mood { Exploration, #[default] Consolidation, Defensive }

pub struct MoodModulation {
    pub metabolic_tick: Duration,
    pub arbiter_max_per_tick: usize,
    pub repeat_failure_threshold: usize,
    pub signal_decay_multiplier: f32,
    pub claim_ttl_multiplier: f32,
}
```

Modulation table:

| Modulation                    | Exploration | Consolidation | Defensive |
|-------------------------------|-------------|---------------|-----------|
| `metabolic_tick`              | 10s         | 5s            | 3s        |
| `arbiter_max_per_tick`        | 1           | 2             | 4         |
| `repeat_failure_threshold`    | 3           | 2             | 1         |
| `signal_decay_multiplier`     | 1.1         | 1.0           | 0.85      |
| `claim_ttl_multiplier`        | 0.75        | 1.0           | 1.5       |

Semantics: `signal_decay_multiplier < 1.0` ⇒ slower decay (values preserved longer). `claim_ttl_multiplier > 1.0` ⇒ longer TTL (resources held longer). Defensive mood preserves warnings and holds claims; Exploration churns faster.

**Auto-transition** (evaluated once per metabolic tick):

Let `pressure = count(ClaimViolation + Warning + HelpNeeded signals posted in last 10 generations)`.

- `Consolidation → Defensive` if `pressure ≥ 8`.
- `Defensive → Consolidation` if `pressure ≤ 4` (hysteresis).
- `Consolidation → Exploration` if `pressure ≤ 1` for 3 consecutive metabolic ticks (quiet-streak requirement).
- `Exploration → Consolidation` if `pressure ≥ 3` (instant snap-back).
- `Defensive ↔ Exploration`: never direct, always via Consolidation.

**Manual override**: `AgentBusEvent::SetMood { mood, source }` sets the mood and locks auto-transitions for `MOOD_OVERRIDE_LOCK_GENS = 20` generations. Every mood change (auto or manual) emits a `Warning` signal on `Global` with `source` in the payload.

---

## 3. Event system

Defined in [`crates/nit-core/src/agent_bus/`](../crates/nit-core/src/agent_bus/) (split across `mod.rs`, `upsert.rs`, `turn_lifecycle.rs`, `turn_completion.rs`, `turn_error.rs`, `claims_signals.rs`, `file_ops.rs`, `mood_control.rs`, `token_count.rs`, `helpers.rs`).

### 3.1 Event taxonomy

**Runtime events** (emitted by subprocess runners):
- `TurnStarted { agent_id, mission_id, resume_thread_id }`
- `TurnCompleted { agent_id, mission_id, thread_id, token_count, message }`
- `TurnFailed { agent_id, mission_id, thread_id, token_count, message }`
- `FileWrite { agent_id, mission_id, path }`
- `TokenCount { agent_id, mission_id, token_count }`
- ... plus several others unrelated to the substrate.

**Substrate-mutation events** (fully-formed records):
- `EmitSignal { signal: Signal }`
- `AssertClaim { claim: Claim }`
- `AssertAssumption { assumption: Assumption }`
- `SetMood { mood: Mood, source: String }`

**Substrate-mutation events with ID mint-on-apply** (used by `nit-mcp` back-channel — external processes can't mint substrate counters safely):
- `EmitSignalRequest { posted_by, kind, target, payload, initial_strength }`
- `AssertClaimRequest { claimed_by, kind, target, ttl_gens, rationale }` — honors `mood.claim_ttl_multiplier`
- `AssertAssumptionRequest { posted_by, target, fact, ttl_gens, rationale }`

### 3.2 `AgentBusEvent::apply(&mut AppState)` sequence on `TurnCompleted`

```text
1. Apply token counts, update agent status, store thread ids, update mission status.
2. Emit DoneMarker signal for the completing agent (at pre-advance generation).
3. advance_generation() — gen += 1.
4. prune_signals_below(DEFAULT_PRUNE_THRESHOLD) — drop faded signals.
5. expire_claims(current_gen) — drop expired claims.
6. expire_assumptions(current_gen) — drop expired assumptions.
7. Run observers (Vec-buffered); emit each returned signal.
8. Run arbiters (Vec-buffered); reduce_proposals applies policy; apply_interventions emits + queues.
9. save(&workspace_root) — persist substrate to disk.
```

`TurnFailed` is similar but **does not advance generation** (failure ≠ superstep), emits a `Warning` instead of `DoneMarker`, and skips observer/arbiter runs.

`FileWrite` performs auto-claim + assumption-invalidation but does not touch the generation counter.

### 3.3 Emission buffering invariant

Observers' and arbiters' returned `Vec<_>` of proposals are collected first, then applied. An observer/arbiter running in tick N **cannot** see another observer/arbiter's emissions from the same tick. This prevents intra-tick cascades.

---

## 4. Metabolism

Defined in [`crates/nit-core/src/metabolism.rs`](../crates/nit-core/src/metabolism.rs).

Wall-clock heartbeat that runs independently of turn boundaries. Integrated via frame-time check in nit-tui's main loop:

```rust
if last_metabolism.elapsed() >= nit_core::metabolism::tick_interval_for(state.substrate.mood) {
    let outcome = nit_core::metabolism::tick(state);
    if !outcome.is_noop() { needs_redraw = true; }
    last_metabolism = Instant::now();
}
```

`tick(&mut AppState) -> MetabolicTickOutcome`:

```text
1. pressure = state.substrate.pressure_in_window(10)
2. mood_quiet_streak += 1 if pressure ≤ 1 else 0
3. auto_transition → if unlocked, may shift mood and emit mood-shift Warning
4. claims_expired = expire_claims(current_gen)
5. assumptions_expired = expire_assumptions(current_gen)
6. signals_pruned = prune_signals_below(DEFAULT_PRUNE_THRESHOLD)
7. observer_emissions = run_all(state) → emit each
8. arbiter_interventions = run_all + reduce + apply
9. if dirty, save(&workspace_root)
```

`advance_generation` is **not** called — gen counts turns, not wall-clock ticks. `MetabolicTickOutcome::is_noop()` returns true iff nothing changed; noop ticks skip the save entirely (prevents disk thrashing when idle).

Cadence is mood-modulated: 3s / 5s / 10s for Defensive / Consolidation / Exploration.

---

## 5. Observers

Defined in [`crates/nit-core/src/observers/`](../crates/nit-core/src/observers/).

Fn-pointer-based, compile-time registered:

```rust
type ObserverFn = fn(&AppState) -> Vec<ObservedEmission>;
pub struct Observer { pub name: &'static str, pub run: ObserverFn }
pub const REGISTERED_OBSERVERS: &[Observer] = &[
    repeat_failure::OBSERVER,
    global_heat::OBSERVER,
];
```

Registry-enforced `posted_by = "observer:{name}"` — observers cannot self-spoof as agents. Observer emissions use `initial_strength = OBSERVER_INITIAL_STRENGTH = 1.5`.

**Current observers**:

- **`repeat_failure`** — emits `HelpNeeded` when an agent posts ≥`mood.repeat_failure_threshold` Warning signals within 5 generations. Self-silences if a recent observer-emitted HelpNeeded already targets the agent.
- **`global_heat`** — emits a Global Warning when total signal count exceeds 100, with a 10-generation cooldown.

See `LIVING_SYSTEM.md` for the role definition.

---

## 6. Arbiters

Defined in [`crates/nit-core/src/arbiters/`](../crates/nit-core/src/arbiters/).

Same fn-pointer shape as observers, plus a policy layer:

```rust
pub fn run_all(state: &AppState) -> Vec<(&'static str, InterventionProposal)>;
pub fn reduce_proposals(state, raw, retry_limit) -> Vec<Intervention>;
pub fn apply_interventions(state, reduced);
```

`reduce_proposals` enforces:
- Per-(arbiter, target) cooldown of 10 generations (checks for recent `InterventionEmitted` signals).
- Per-tick budget of `mood.arbiter_max_per_tick` (1 / 2 / 4 for Exploration / Consolidation / Defensive).
- Downgrade to `EmitSignalOnly` when `state.genome_retry_count ≥ ARBITER_RETRY_LIMIT`.

`apply_interventions` emits an `InterventionEmitted` signal per intervention AND pushes `Intervention` entries onto `state.pending_interventions`. nit-tui's `drain_pending_interventions` (in `crates/nit-tui/src/app/genome_retry.rs`) pops each, consumes one slot of the shared `GENOME_RETRY_LIMIT` budget, and calls `dispatch_agent_prompt` with the escalated prompt.

**Current arbiter**: `persistent_conflict` — detects ≥3 mutual `ClaimViolation` signals between an agent pair within 10 gens and emits a "permanently yield" prompt targeting the lexicographically-larger agent.

---

## 7. Mission memory

Defined in [`crates/nit-core/src/mission_memory/`](../crates/nit-core/src/mission_memory/) (split across `mod.rs`, `index.rs`, `io.rs`, `search.rs`).

Indexes completed missions from `.nit/swarm/<mission-id>/` (title, template, task summaries, touched files, precomputed tags) into a single file at `.nit/memory/index.json`.

**Retrieval**: `retrieve_similar(&index, query, scope_file_tokens, exclude, k)`:
- Tokenize query + file-path tokens (lowercase, strip stopwords, split snake_case, split paths on `/ \ .`).
- Compute IDF-weighted Jaccard against each mission's `tags`: `score = weighted_overlap / weighted_union` where weights are `log((N+1)/(df+1)) + 1`.
- Add title-term boost (cap 0.3) and file-path overlap bonus (cap 0.2).
- Filter exclude list, drop zero-score hits, sort descending, truncate to K.

**Integration**: at swarm planner construction (`crates/nit-tui/src/swarm/prompts.rs::build_planner_prompt` + `crates/nit-tui/src/swarm/runtime.rs::build_followup_planner_prompt`), retrieves top-3 excluding the current mission and injects as a "Prior similar missions" section before "Operator request:". Bounded to ~1-2 KB added.

**Update**: `upsert_mission(workspace_root, mission_id)` called after `summary.json` is written in `write_swarm_run_provenance`. Best-effort (discards Result). Bootstrap: `load_or_build` on first query in a session.

---

## 8. nit-mcp — deliberate agent emission

Defined in [`crates/nit-mcp/`](../crates/nit-mcp/). Unix-only in v1 (Windows compiles but has no listener).

**Architecture**:

```
[nit-tui main thread]
  │  binds UDS listener /tmp/nit-mcp-{pid}.sock
  │  spawns `codex mcp-server -c mcp_servers.nit={command="nit-mcp-server", env=...}`
  ▼
[codex mcp-server]
  │  when the model calls a nit tool, Codex spawns `nit-mcp-server` as its child
  ▼
[nit-mcp-server binary]
  │  reads MCP stdio JSON-RPC
  │  on tools/call, sends NDJSON request over UDS → awaits ack → responds to Codex
  ▼
[nit-tui listener thread]
  │  reads NDJSON, constructs AgentBusEvent::*Request, sends on event channel
  │  replies ack
```

**Tools**:
- `emit_signal(kind, target, payload?, strength?)`
- `assert_claim(kind, target, ttl_gens, rationale)` — honors mood TTL multiplier
- `assert_assumption(target, fact, ttl_gens, rationale)`

**ID mint-on-apply**: the `*Request` variants carry no id / `posted_at_gen`. The main thread mints these atomically during `apply()` via `next_signal_id` / `next_claim_id` / `next_assumption_id`. The external process never touches substrate counters.

**Known limitations**:
- Coarse session-level agent attribution via `NIT_MCP_AGENT_ID` env var.
- Codex `-c mcp_servers.nit=...` TOML inline-table syntax is defensive but unverified against live Codex.
- Claude has no MCP support — these tools are Codex-only.

---

## 9. Persistence

- **`.nit/substrate/state.json`** — atomic write via `nit_utils::fs::write_atomic`. Load is tolerant (missing/corrupt → `Default`). All new fields use `#[serde(default)]`.
- **`.nit/memory/index.json`** — atomic write, tolerant load, single file (not sharded; cost is O(N log N) at write, negligible at hundreds of missions).
- **`.nit/swarm/<mission-id>/`** — pre-existing mission artifacts (unchanged by the substrate layer; indexed by mission memory).

---

## 10. TUI observability

The substrate is inspected via a popup overlay opened from anywhere in the TUI.

**Open the overlay**:
- Keybind: **F3**
- Commands: `:substrate`, `:sub`, `:sig` (all open on Signals tab); `:claims`, `:assumptions`, `:asm` (open on the named tab)

The overlay has three internal sub-tabs:

1. `SIGNALS` — live signal table. Columns STR / KIND / BY / TARGET / AGE / ID, sorted by effective strength descending, color-coded by kind, width-adaptive.
2. `CLAIMS` — live claim table. Columns TTL / KIND / BY / TARGET / AGE / ID, sorted by remaining TTL descending.
3. `ASSUMPTIONS` — live assumption table. Columns TTL / BY / TARGET / AGE / RATIONALE / ID.

**Tab switching inside overlay**: `Tab` key cycles; mouse-click on tab labels also cycles (clicking the active tab closes).
**Scroll**: mouse wheel (shared across all three sub-tabs).
**Close**: `F3` or `Esc`.

The mood glyph (`[E.]` / `[C.]` / `[D!]`) is always visible in the Visualizer pane's title bar — no need to open the overlay to see the current mood.

APPLY / SEED / SNAP / SEARCH buttons in the Visualizer title remain always-visible and clickable.

---

## 11. Code map

| Concern                      | Crate / File                                                          |
|------------------------------|-----------------------------------------------------------------------|
| Substrate types              | `crates/nit-core/src/substrate/` (`signals.rs`, `claims.rs`, `assumptions.rs`, `mod.rs`) |
| Mood                         | `crates/nit-core/src/mood.rs`                                         |
| Metabolism                   | `crates/nit-core/src/metabolism.rs`                                   |
| Observers (framework + two)  | `crates/nit-core/src/observers/`                                      |
| Arbiters (framework + one)   | `crates/nit-core/src/arbiters/`                                       |
| Mission memory               | `crates/nit-core/src/mission_memory/`                                 |
| Event bus + apply            | `crates/nit-core/src/agent_bus/`                                      |
| AppState + drain queues      | `crates/nit-core/src/state/`                                          |
| Signals tab widget           | `crates/nit-tui/src/widgets/signals_view.rs`                          |
| Claims tab widget            | `crates/nit-tui/src/widgets/claims_view.rs`                           |
| Assumptions tab widget       | `crates/nit-tui/src/widgets/assumptions_view.rs`                      |
| Visualizer pane              | `crates/nit-tui/src/widgets/visualizer_view.rs`                       |
| Substrate overlay popup      | `crates/nit-tui/src/widgets/substrate_overlay.rs`                     |
| Claim-retry / intervention drains, metabolism tick | `crates/nit-tui/src/app/mod.rs` (`drain_pending_claim_retries`, `drain_pending_interventions`, main loop) |
| MCP back-channel listener    | `crates/nit-tui/src/mcp_backchannel.rs`                               |
| Codex runner MCP wiring      | `crates/nit-tui/src/codex_runner/`                                    |
| nit-mcp MCP server           | `crates/nit-mcp/src/server.rs`                                        |
| nit-mcp protocol types       | `crates/nit-mcp/src/protocol.rs`                                      |
| nit-mcp back-channel client  | `crates/nit-mcp/src/backchannel.rs`                                   |
| nit-mcp binary               | `crates/nit-mcp/src/main.rs`                                          |

---

## 12. Design-decision ledger

- **Generation ≠ wall-clock.** TTLs and cooldowns are generation-relative so decay stays meaningful between turns. Metabolism advances wall-clock alone; generation only advances on `TurnCompleted`. Trade-off: during idle, TTLs don't expire via generation — metabolism handles the wall-clock-based sweep anyway.
- **Claims + assumptions are a pair.** Claims express *write intent* ("I'm taking this"); assumptions express *read dependencies* ("I depend on this being true"). Both share target geometry, TTL semantics, and auto-interactions with `FileWrite`.
- **Observers detect, arbiters actuate.** Observers emit signals only; arbiters can redispatch agents. This keeps detection and intervention separable and testable in isolation.
- **Shared retry budget.** Claim-retries and arbiter interventions share `GENOME_RETRY_LIMIT` so stacked pressure on one agent is bounded.
- **No subprocess write interception.** nit does not own the file-write path — subprocess agents write directly. Claim-based guarding is advisory (violations trigger retries, not rollbacks). Hard-blocking would require runner-side snapshot/rollback machinery, explicitly deferred.
- **Tolerant load everywhere.** Every persistent field uses `#[serde(default)]`; corrupt files degrade to `Default`. Avoids version-lock on disk.
- **ID mint on main thread only.** External processes (nit-mcp) never touch substrate counters. `*Request` variants exist specifically to let the main thread mint IDs atomically during `apply()`.

---

## 13. Extension points

### Adding a new signal kind
1. Add variant to `SignalKind` in `crates/nit-core/src/substrate/signals.rs`.
2. Add decay rate in `SignalKind::decay_rate()`.
3. Update `signals_view.rs` for kind-label rendering.
4. Update `LIVING_SYSTEM.md` if it represents a new coordination meaning.

### Adding a new claim kind
1. Add variant to `ClaimKind`.
2. Extend the compatibility matrix in `claims_conflict`.
3. Update `claims_view.rs` color mapping if desired.

### Adding a new observer
1. Create `crates/nit-core/src/observers/<name>.rs` with a `pub const OBSERVER: Observer` and `fn observe(&AppState) -> Vec<ObservedEmission>`.
2. Declare `pub mod <name>;` in `observers/mod.rs`.
3. Append to `REGISTERED_OBSERVERS`.
4. Add a test mirroring `repeat_failure`'s self-silencing pattern.
5. Document in `LIVING_SYSTEM.md` under `Current observers`.

### Adding a new arbiter
Same shape as observer, but also:
- Choose actuation kind (`RedispatchWithEscalatedPrompt` or `EmitSignalOnly`).
- Self-silencing happens for free via `reduce_proposals` cooldown if you emit `InterventionEmitted` on matching targets.

### Adding a new tool to nit-mcp
1. Extend `BackchannelRequest` in `crates/nit-mcp/src/protocol.rs`.
2. Add tool schema + handler in `crates/nit-mcp/src/server.rs`.
3. Add a matching `AgentBusEvent::*Request` in `crates/nit-core/src/agent_bus/` (extend the relevant submodule and re-export from `mod.rs`) with an `apply` arm that mints IDs.
4. Extend `mcp_backchannel.rs`'s `backchannel_to_event` function.

### Adding a new mood modulation
1. Add field to `MoodModulation` in `crates/nit-core/src/mood.rs`.
2. Provide per-mood values in `Mood::modulation()`.
3. Consume at the relevant call site.

### Adding a new mood
1. Add variant to `Mood`.
2. Add modulation row in `Mood::modulation()`.
3. Update `auto_transition` to handle the new state's entry/exit thresholds.
4. Add TUI glyph in `signals_view.rs`.

---

## 14. Session journey (2026-04-16 → 2026-04-18)

The living-system layer was built in 16 commits across three days:

| # | Commit   | What shipped                                                        |
|---|----------|---------------------------------------------------------------------|
| 1 | `f55ceda` | Phase 1 — `SubstrateState` scaffold + persistence                   |
| 2 | `46f0f45` | Phase 2 — typed signals with lazy decay                             |
| 3 | `be47f24` | Phase 2 — persistence wiring at `TurnCompleted`                     |
| 4 | `11af8c8` | Micro-2.5 — runtime-derived emission (DoneMarker / Warning)         |
| 5 | `8513c25` | Phase 3 — Substrate Signals tab in Visualizer pane                  |
| 6 | `1530451` | Phase 5 — observer role + `repeat_failure` + `global_heat`          |
| 7 | `bf953ed` | Phase 3-lattice — claim lattice with retry consequence              |
| 8 | `35ca746` | Claims tab — Visualizer gains third tab                             |
| 9 | `4aed709` | Phase 7 — metabolism (wall-clock sweep)                             |
| 10 | `caf47b0` | Phase 4 — assumption manifests                                      |
| 11 | `a7e6cea` | Phase 8 — cross-mission structural memory                           |
| 12 | `9cff7e6` | Phase 6 — arbiter role + `persistent_conflict`                      |
| 13 | `b31cfa6` | Docs — `LIVING_SYSTEM.md` role roster                               |
| 14 | `1afe566` | Phase 9 — mood (system-wide tuning modulator)                       |
| 15 | `d89ba22` | Polish — assumptions tab + IDF weighting + mood v2 modulations      |
| 16 | `c974ed7` | MCP deferred B — `nit-mcp` crate for deliberate agent emission      |

All commits on `main`, not pushed, not merged upstream.
