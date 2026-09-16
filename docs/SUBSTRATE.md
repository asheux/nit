# Substrate

This document is the contributor reference for nit's living-system layer: the substrate data model, the events that mutate it, the metabolic tick, observers, arbiters, mission memory, the `nit-mcp` tools, persistence, and the TUI overlay. Read it when you change this code. Related docs:

- [LIVING_SYSTEM.md](LIVING_SYSTEM.md): the four coordination roles (worker, observer, arbiter, resolver).
- [SUBSTRATE_TESTING.md](SUBSTRATE_TESTING.md): how to run the tests and verify each feature.
- [SWARM.md](SWARM.md): the per-mission task roles (propose, integrate, judge, and others).
- [ARCHITECTURE.md](ARCHITECTURE.md): overall nit architecture.

## 1. Overview

nit's agents work over a persistent, typed substrate that records coordination state across turns, sessions, and missions. Inside a mission, the DAG-based swarm scheduler runs tasks ([SWARM.md](SWARM.md)). Across missions and between turns, the substrate accumulates signals, claims, assumptions, and mood: observers read it, arbiters act on it, metabolism sweeps it on a wall-clock tick, and mission memory retrieves from it.

Subprocess agents (Codex, Claude) never query the substrate directly. State enters it in two ways:

1. Runtime events. nit handles `TurnCompleted`, `TurnFailed`, `FileWrite`, and similar events and updates the substrate as a side effect.
2. The `nit-mcp` tools, which let a Codex agent write records on purpose (section 9).

## 2. Design rules

- Stigmergy: agents coordinate through traces in the environment, not direct messages.
- Metabolism: decay and pruning run on a wall-clock tick, independent of user activity.
- Generation-relative time: TTLs and cooldowns count turns, not seconds. The generation advances only on `TurnCompleted`.
- Four roles: worker (does work), observer (detects patterns), arbiter (intervenes), resolver (commits). See [LIVING_SYSTEM.md](LIVING_SYSTEM.md).
- Tolerant persistence: missing fields take defaults, corrupt files load as empty state, and old snapshots load into new binaries.
- Observers detect, arbiters act: observers only emit signals; arbiters can redispatch agents.
- Shared retry budget: claim retries and arbiter interventions both draw on `GENOME_RETRY_LIMIT`, so stacked pressure on one agent stays bounded.
- Advisory claims, no rollback: nit does not own the file-write path. A violation triggers a retry prompt, never a rollback.
- IDs are minted on the main thread only: external processes never touch substrate counters.

## 3. Data model

Defined in `crates/nit-core/src/substrate/` (`signals.rs`, `claims.rs`, `assumptions.rs`, and `mod.rs` for `SubstrateState`).

### 3.1 `SubstrateState`

```rust
pub struct SubstrateState {
    pub generation: u64,
    pub signals: HashMap<SignalId, Signal>,
    pub claims: HashMap<ClaimId, Claim>,
    pub observations: Vec<serde_json::Value>,     // reserved
    pub signal_counter: u64,
    pub claim_counter: u64,
    pub assumptions: HashMap<AssumptionId, Assumption>,
    pub assumption_counter: u64,
    pub mood: Mood,
    pub mood_override_until_gen: u64,
    pub mood_quiet_streak: u32,
}
```

Persisted at `.nit/substrate/state.json` (section 10). Every field except `generation` has `#[serde(default)]`, so older snapshots load cleanly.

### 3.2 Signal

```rust
pub struct Signal {
    pub id: SignalId,                     // "{gen}-{posted_by}-{counter}"
    pub kind: SignalKind,
    pub posted_by: String,
    pub posted_at_gen: u64,
    pub target: SignalTarget,
    pub initial_strength: f32,            // default 1.0; observers 1.5; arbiters 2.0
    pub payload: serde_json::Value,
}
```

`SignalKind` and its decay rate per generation:

| Kind                  | Decay per gen | Role                            |
|-----------------------|---------------|---------------------------------|
| `HelpNeeded`          | 0.5           | Urgent; resolves or fades fast  |
| `Lead`                | 0.7           | Suggestive tip                  |
| `Warning`             | 0.8           | Mid-persistence caution         |
| `ClaimViolation`      | 0.85          | Write conflict evidence         |
| `Deadend`             | 0.9           | Long-lived peer warning         |
| `DoneMarker`          | 0.95          | Durable "this happened" fact    |
| `InterventionEmitted` | 0.9           | Arbiter trace                   |

Effective strength is computed lazily: `initial * clamp(rate / m, 0.01, 0.999)^(current_gen - posted_at_gen)`, where `m` is `mood.modulation().signal_decay_multiplier`. A signal is pruned once its effective strength drops below `DEFAULT_PRUNE_THRESHOLD = 0.05`.

`SignalTarget`: `File { path }` | `Agent { agent_id }` | `Global`.

### 3.3 Claim

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

Two claims conflict only if their targets overlap and their kinds are incompatible (`claims_conflict`):

|                 | ExclusiveWrite | SharedRead | AppendOnly | Soft  |
|-----------------|----------------|------------|------------|-------|
| ExclusiveWrite  | ❌             | ❌         | ❌         | ✅    |
| SharedRead      | ❌             | ✅         | ✅         | ✅    |
| AppendOnly      | ❌             | ✅         | ✅         | ✅    |
| Soft            | ✅             | ✅         | ✅         | ✅    |

Target overlap (`targets_overlap`):

- `Global` overlaps everything.
- `File(p)` overlaps `File(p)`. Different paths do not overlap.
- `Region(p, s1, e1)` overlaps `Region(p, s2, e2)` when `s1 <= e2 && s2 <= e1`.
- `Region(p, _, _)` overlaps `File(p)`.

A claim is expired when `current_gen >= claimed_at_gen + ttl_gens`. `expire_claims(current_gen)` drops expired entries.

Auto-claim: every `FileWrite` event asserts an `ExclusiveWrite` claim on the path with `ttl_gens = (3 * mood.claim_ttl_multiplier).max(1)`. If it conflicts, the new claim is not inserted; nit emits a `ClaimViolation` signal and queues a `ClaimRetryRequest` on `state.pending_claim_retries`.

### 3.4 Assumption

```rust
pub struct Assumption {
    pub id: AssumptionId,
    pub target: AssumptionTarget,
    pub fact: serde_json::Value,   // opaque; nit does not inspect it
    pub posted_by: String,
    pub posted_at_gen: u64,
    pub ttl_gens: u64,
    pub rationale: String,
}
```

`AssumptionTarget` has the same shape as `ClaimTarget`. Assumptions never conflict with each other, so `assert_assumption` cannot fail.

Auto-invalidation: every `FileWrite` removes each non-expired assumption whose target overlaps the written path. For each one, nit emits a `Warning` signal targeting the assumption's original poster, not the writer, with the removed `Assumption` in the payload.

### 3.5 Observations

`observations: Vec<serde_json::Value>` is a reserved slot for a future observation record type. Nothing writes to it yet.

### 3.6 Mood

Defined in `crates/nit-core/src/mood.rs`.

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

| Modulation                 | Exploration | Consolidation | Defensive |
|----------------------------|-------------|---------------|-----------|
| `metabolic_tick`           | 10s         | 5s            | 3s        |
| `arbiter_max_per_tick`     | 1           | 2             | 4         |
| `repeat_failure_threshold` | 3           | 2             | 1         |
| `signal_decay_multiplier`  | 1.1         | 1.0           | 0.85      |
| `claim_ttl_multiplier`     | 0.75        | 1.0           | 1.5       |

`signal_decay_multiplier` divides the decay rate, so a value below 1.0 slows decay. `claim_ttl_multiplier` above 1.0 lengthens TTLs. Defensive mood keeps warnings and holds claims longer; Exploration churns faster.

Auto-transition runs once per metabolic tick. `pressure` is the number of `ClaimViolation`, `Warning`, and `HelpNeeded` signals posted in the last 10 generations.

- `Consolidation -> Defensive` when `pressure >= 8`.
- `Defensive -> Consolidation` when `pressure <= 4` (hysteresis).
- `Consolidation -> Exploration` when `pressure <= 1` for 3 consecutive ticks (the quiet streak).
- `Exploration -> Consolidation` when `pressure >= 3` (instant snap-back).
- `Defensive` and `Exploration` never switch directly; the path always goes through `Consolidation`.

Manual override: `AgentBusEvent::SetMood { mood, source }` sets the mood and locks auto-transitions for `MOOD_OVERRIDE_LOCK_GENS = 20` generations. Every mood change, auto or manual, emits a `Warning` signal on `Global` with the `source` in its payload.

## 4. Event system

Defined in `crates/nit-core/src/agent_bus/` (`mod.rs`, `upsert.rs`, `turn_lifecycle.rs`, `turn_completion.rs`, `turn_error.rs`, `claims_signals.rs`, `file_ops.rs`, `mood_control.rs`, `token_count.rs`, `helpers.rs`).

### 4.1 Event taxonomy

Runtime events, emitted by the subprocess runners:

- `TurnStarted { agent_id, mission_id, resume_thread_id }`
- `TurnCompleted { agent_id, mission_id, thread_id, token_count, message }`
- `TurnFailed { agent_id, mission_id, thread_id, token_count, message }`
- `FileWrite { agent_id, mission_id, path }`
- `TokenCount { agent_id, mission_id, token_count }`
- Several others that do not touch the substrate.

Substrate-mutation events, carrying fully formed records:

- `EmitSignal { signal: Signal }`
- `AssertClaim { claim: Claim }`
- `AssertAssumption { assumption: Assumption }`
- `SetMood { mood: Mood, source: String }`

Request events, whose IDs are minted on apply. The `nit-mcp` back-channel uses these because an external process cannot mint substrate counters safely:

- `EmitSignalRequest { posted_by, kind, target, payload, initial_strength }`
- `AssertClaimRequest { claimed_by, kind, target, ttl_gens, rationale }`, which applies `mood.claim_ttl_multiplier`
- `AssertAssumptionRequest { posted_by, target, fact, ttl_gens, rationale }`

### 4.2 `apply` sequence on `TurnCompleted`

```text
1. Apply token counts, update agent status, store thread ids, update mission status.
2. Emit a DoneMarker signal for the agent, at the pre-advance generation.
3. advance_generation(): gen += 1.
4. prune_signals_below(DEFAULT_PRUNE_THRESHOLD): drop faded signals.
5. expire_claims(current_gen): drop expired claims.
6. Run observers (buffered); emit each returned signal.
7. Run arbiters (buffered); reduce_proposals applies policy; apply_interventions emits and queues.
8. save(&workspace_root): persist the substrate.
```

`TurnCompleted` does not expire assumptions; only the metabolic tick does.

`TurnFailed` does not advance the generation, emits a `Warning` instead of a `DoneMarker`, and skips observers and arbiters.

`FileWrite` runs auto-claim and assumption invalidation and leaves the generation counter alone.

### 4.3 Emission buffering

Observers and arbiters return `Vec`s of proposals. nit collects them all first, then applies them. So an observer or arbiter in tick N never sees another observer's or arbiter's emissions from the same tick, which prevents cascades within a tick.

## 5. Metabolism

Defined in `crates/nit-core/src/metabolism.rs`. The tick is a wall-clock heartbeat, independent of turn boundaries. nit-tui's main loop (`crates/nit-tui/src/app/runner.rs`) checks it once per frame:

```rust
if last_metabolism.elapsed() >= nit_core::metabolism::tick_interval_for(state.substrate.mood) {
    let outcome = nit_core::metabolism::tick(state);
    if !outcome.is_noop() { needs_redraw = true; }
    last_metabolism = Instant::now();
}
```

`tick(&mut AppState) -> MetabolicTickOutcome`:

```text
1. claims_expired = expire_claims(current_gen)
2. assumptions_expired = expire_assumptions(current_gen)
3. signals_pruned = prune_signals_below(DEFAULT_PRUNE_THRESHOLD)
4. pressure = pressure_in_window(10)
5. mood_quiet_streak += 1 if pressure <= 1, else reset to 0
6. auto_transition, unless a manual override is active; a shift emits a Warning on Global
7. observer_emissions = observers::run_all(state); emit each
8. arbiter_interventions = arbiters::run_all + reduce_proposals + apply_interventions
9. save(&workspace_root) if anything changed
```

`advance_generation` is never called here: the generation counts turns, not ticks. `MetabolicTickOutcome::is_noop()` is true when nothing changed, and a noop tick skips the save so an idle session does not thrash the disk. The interval is the mood's `metabolic_tick` (section 3.6).

## 6. Observers

Defined in `crates/nit-core/src/observers/`. Observers are plain function pointers registered at compile time:

```rust
type ObserverFn = fn(&AppState) -> Vec<ObservedEmission>;
pub struct Observer { pub name: &'static str, pub run: ObserverFn }
pub const REGISTERED_OBSERVERS: &[Observer] = &[
    repeat_failure::OBSERVER,
    global_heat::OBSERVER,
    sparse_plan::OBSERVER,
];
```

The registry sets `posted_by = "observer:{name}"`, so an observer cannot pose as an agent. Observer emissions use `initial_strength = OBSERVER_INITIAL_STRENGTH = 1.5`.

Current observers:

- `repeat_failure`: emits `HelpNeeded` targeting an agent that posted at least `mood.repeat_failure_threshold` `Warning` signals (3 / 2 / 1 for Exploration / Consolidation / Defensive) within 5 generations. Stays silent if a recent `HelpNeeded` from this observer already targets that agent.
- `global_heat`: emits a `Warning` on `Global` when the total signal count exceeds 100, with a 10-generation cooldown.
- `sparse_plan`: emits `HelpNeeded` targeting a planner agent when signals posted by `planner:<agent>` include at least 3 `Warning`s with payload `reason = "unresolved_dep"` within 10 generations. The payload carries `unresolved_count` and `missing_deps_sample`. Self-silences like `repeat_failure`.

## 7. Arbiters

Defined in `crates/nit-core/src/arbiters/`. Same function-pointer shape as observers, plus a policy layer:

```rust
pub fn run_all(state: &AppState) -> Vec<(&'static str, InterventionProposal)>;
pub fn reduce_proposals(state, raw, retry_limit) -> Vec<Intervention>;
pub fn apply_interventions(state, reduced);
```

`reduce_proposals` enforces, in this order:

- A per-(arbiter, target) cooldown of `ARBITER_COOLDOWN_GENS = 10` generations, checked against recent `InterventionEmitted` signals. Proposals in cooldown are dropped.
- A downgrade to `EmitSignalOnly` when the target's genome retry count has reached `ARBITER_RETRY_LIMIT = 3`. For an agent pair, both agents must be exhausted; for a mission or global target, any agent.
- A per-tick budget of `mood.arbiter_max_per_tick` (1 / 2 / 4 for Exploration / Consolidation / Defensive).

`apply_interventions` emits one `InterventionEmitted` signal per intervention (strength 2.0) and pushes each `Intervention` onto `state.pending_interventions`. nit-tui's `drain_pending_interventions` (`crates/nit-tui/src/app/genome_retry.rs`) pops each entry and skips `EmitSignalOnly`. It sends the prompt to the agent, or for a pair to the payload's `chosen_recipient`; mission and global targets are not dispatched. Each dispatch consumes one slot of the shared `GENOME_RETRY_LIMIT` budget and goes through `dispatch_agent_prompt`.

Current arbiters:

- `persistent_conflict`: at least 3 mutual `ClaimViolation` signals between an agent pair within 10 generations. Proposes `RedispatchWithEscalatedPrompt` for the pair; the "permanently yield this resource" prompt goes to the lexicographically larger agent.
- `help_needed`: one proposal per agent targeted by a `HelpNeeded` from `observer:repeat_failure` within 5 generations. The prompt tells the agent to stop retrying, name the blocker, and downscope.
- `sparse_plan`: one proposal per planner targeted by a `HelpNeeded` from `observer:sparse_plan` within 10 generations. The prompt tells the planner to fix `deps` entries that name task ids missing from the DAG.

## 8. Mission memory

Defined in `crates/nit-core/src/mission_memory/` (`mod.rs`, `index.rs`, `io.rs`, `search.rs`).

Indexes completed missions from `.nit/swarm/<mission-id>/` (title, template, task summaries, touched files, precomputed tags) into one file, `.nit/memory/index.json`.

Retrieval, `retrieve_similar(&index, query, scope_file_tokens, exclude, k)`:

1. Tokenize the query and file-path tokens: lowercase, drop stopwords, split snake_case, split paths on `/`, `\`, and `.`.
2. Score each mission by IDF-weighted Jaccard against its `tags`: `score = weighted_overlap / weighted_union`, with weights `ln((N + 1) / (df + 1)) + 1`.
3. Add a title-term boost (cap 0.3) and a file-path overlap bonus (cap 0.2).
4. Drop excluded missions and zero scores, sort descending, keep the top `k`.

Integration: when the swarm planner prompt is built (`build_planner_prompt` in `crates/nit-tui/src/swarm/prompts.rs` and `build_followup_planner_prompt` in `crates/nit-tui/src/swarm/runtime.rs`), nit retrieves the top 3 hits, excluding the current mission, and injects them as a "Prior similar missions" section before "Operator request:". This adds about 1 to 2 KB.

Update: `upsert_mission(workspace_root, mission_id)` runs after `summary.json` is written in `write_swarm_run_provenance`. It is best-effort; the result is discarded. `load_or_build` builds the index on the first query of a session.

## 9. nit-mcp: tools for agents to write to the substrate

Defined in `crates/nit-mcp/`. Unix only in v1; Windows builds compile but have no listener.

```text
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

Tools:

- `emit_signal(kind, target, payload?, strength?)`
- `assert_claim(kind, target, ttl_gens, rationale)`, which applies the mood TTL multiplier
- `assert_assumption(target, fact, ttl_gens, rationale)`

nit looks for `nit-mcp-server` next to the running `nit` binary and skips the `-c` override if it is missing. The Codex config passes two environment variables: `NIT_MCP_BACKCHANNEL_SOCKET` and `NIT_MCP_AGENT_ID`. On hosts without Unix sockets the client reads `NIT_MCP_BACKCHANNEL_PORT` and connects to TCP `127.0.0.1`, but nit-tui does not start a listener there.

ID minting: the `*Request` variants carry no id or `posted_at_gen`. The main thread mints them during `apply()` with `next_signal_id`, `next_claim_id`, and `next_assumption_id`. The external process never touches substrate counters.

Known limitations:

- Attribution is per session: every emission carries the `NIT_MCP_AGENT_ID` set at spawn (default `codex-session`).
- The `-c mcp_servers.nit=...` inline TOML override is not verified against live Codex.
- Only the Codex runner gets the MCP config. The Claude runner has none, so the tools are Codex-only.

## 10. Persistence

- `.nit/substrate/state.json`: atomic write via `nit_utils::fs::write_atomic`. Load is tolerant: a missing or corrupt file becomes `Default`.
- `.nit/memory/index.json`: atomic write, tolerant load, one file. Writes cost O(N log N), which is negligible at hundreds of missions.
- `.nit/swarm/<mission-id>/`: mission artifacts, unchanged by the substrate layer and indexed by mission memory.

## 11. TUI overlay

The substrate overlay is a popup you can open from anywhere in the TUI.

- Keys: `F3` or `Ctrl+Space` (outside Insert mode).
- Commands: `:substrate`, `:sub`, `:sig`, `:signals` open the Signals tab; `:claims`, `:assumptions`, `:asm` open the named tab.

Tabs:

1. `SIGNALS`: live signal table. Columns STR / KIND / BY / TARGET / AGE / ID, sorted by effective strength descending, colored by kind, width-adaptive.
2. `CLAIMS`: live claim table. Columns TTL / KIND / BY / TARGET / AGE / ID, sorted by remaining TTL descending.
3. `ASSUMPTIONS`: live assumption table. Columns TTL / BY / TARGET / AGE / RATIONALE / ID.

Inside the overlay, `Tab` cycles tabs. Clicking a tab label also cycles; clicking the active tab closes the overlay. Scroll with `j`/`k`, the arrow keys, `PageUp`/`PageDown`, `Home`, or the mouse wheel. Close with `F3`, `Esc`, `q`, or `Ctrl+Space`.

The top bar always shows the current mood as `MOOD: EXPLORATION`, `MOOD: CONSOLIDATION`, or `MOOD: DEFENSIVE`, colored per mood. The Visualizer title keeps its APPLY / SEED / SNAP / SEARCH buttons.

## 12. Code map

| Concern | Crate / file |
|---|---|
| Substrate types | `crates/nit-core/src/substrate/` (`signals.rs`, `claims.rs`, `assumptions.rs`, `mod.rs`) |
| Mood | `crates/nit-core/src/mood.rs` |
| Metabolism | `crates/nit-core/src/metabolism.rs` |
| Observers (framework + three) | `crates/nit-core/src/observers/` |
| Arbiters (framework + three) | `crates/nit-core/src/arbiters/` |
| Mission memory | `crates/nit-core/src/mission_memory/` |
| Event bus + apply | `crates/nit-core/src/agent_bus/` |
| AppState + drain queues | `crates/nit-core/src/state/` |
| Signals tab widget | `crates/nit-tui/src/widgets/signals_view.rs` |
| Claims tab widget | `crates/nit-tui/src/widgets/claims_view.rs` |
| Assumptions tab widget | `crates/nit-tui/src/widgets/assumptions_view.rs` |
| Substrate overlay popup | `crates/nit-tui/src/widgets/substrate_overlay.rs` |
| Mood badge | `crates/nit-tui/src/widgets/top_bar.rs` |
| Visualizer pane | `crates/nit-tui/src/widgets/visualizer_view.rs` |
| Claim-retry and intervention drains | `crates/nit-tui/src/app/genome_retry.rs` (`drain_pending_claim_retries`, `drain_pending_interventions`) |
| Metabolism tick in the main loop | `crates/nit-tui/src/app/runner.rs` |
| Mission provenance + memory upsert | `crates/nit-tui/src/app/provenance.rs` (`write_swarm_run_provenance`) |
| MCP back-channel listener | `crates/nit-tui/src/mcp_backchannel.rs` |
| Codex runner MCP wiring | `crates/nit-tui/src/codex_runner/` |
| nit-mcp MCP server loop | `crates/nit-mcp/src/server.rs` |
| nit-mcp tool schemas + handlers | `crates/nit-mcp/src/tools.rs`, `crates/nit-mcp/src/tools_schema.json` |
| nit-mcp JSON-RPC codes | `crates/nit-mcp/src/jsonrpc.rs` |
| nit-mcp protocol types | `crates/nit-mcp/src/protocol.rs` |
| nit-mcp back-channel client | `crates/nit-mcp/src/backchannel.rs` |
| nit-mcp binary | `crates/nit-mcp/src/main.rs` |

## 13. Extension points

Adding a signal kind:

1. Add the variant to `SignalKind` in `crates/nit-core/src/substrate/signals.rs`.
2. Add its decay rate in `SignalKind::decay_rate()`.
3. Update the kind label in `signals_view.rs`.
4. Update [LIVING_SYSTEM.md](LIVING_SYSTEM.md) if it carries a new coordination meaning.

Adding a claim kind:

1. Add the variant to `ClaimKind`.
2. Extend the compatibility matrix in `claims_conflict`.
3. Update the color mapping in `claims_view.rs` if wanted.

Adding an observer:

1. Create `crates/nit-core/src/observers/<name>.rs` with `pub const OBSERVER: Observer` and `fn observe(&AppState) -> Vec<ObservedEmission>`.
2. Declare `pub mod <name>;` in `observers/mod.rs`.
3. Append it to `REGISTERED_OBSERVERS`.
4. Add a test that mirrors `repeat_failure`'s self-silencing pattern.
5. List it in [LIVING_SYSTEM.md](LIVING_SYSTEM.md) under the observer's current members.

Adding an arbiter: same steps as an observer, in `crates/nit-core/src/arbiters/`, plus:

1. Choose the intervention kind: `RedispatchWithEscalatedPrompt` or `EmitSignalOnly`.
2. Rely on the `reduce_proposals` cooldown for self-silencing; it keys on `InterventionEmitted` signals with matching targets.

Adding a nit-mcp tool:

1. Extend `BackchannelRequest` in `crates/nit-mcp/src/protocol.rs`.
2. Add the tool schema to `crates/nit-mcp/src/tools_schema.json` and a handler in `crates/nit-mcp/src/tools.rs`.
3. Add a matching `AgentBusEvent::*Request` in `crates/nit-core/src/agent_bus/` (extend the relevant submodule and re-export from `mod.rs`) with an `apply` arm that mints IDs.
4. Extend `backchannel_to_event` in `crates/nit-tui/src/mcp_backchannel.rs`.

Adding a mood modulation:

1. Add the field to `MoodModulation` in `crates/nit-core/src/mood.rs`.
2. Give each mood a value in `Mood::modulation()`.
3. Read it at the call site.

Adding a mood:

1. Add the variant to `Mood`.
2. Add its row in `Mood::modulation()`.
3. Update `auto_transition` with the entry and exit thresholds.
4. Add the badge label and color in `crates/nit-tui/src/widgets/top_bar.rs`.
