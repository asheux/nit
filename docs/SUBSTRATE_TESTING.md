# Substrate — Testing & Usage Guide

A practical walkthrough for verifying every piece of nit's living-system layer. Pair with [`SUBSTRATE.md`](SUBSTRATE.md) for architectural reference.

---

## 1. Quick verification — are we green?

From the nit workspace root:

```bash
# All tests, all crates (slowest)
cargo test --workspace

# Per-crate (fastest focused runs)
cargo test -p nit-core       # ~280 tests
cargo test -p nit-tui        # ~430 tests
cargo test -p nit-mcp        # 6 tests

# Substrate-specific filters
cargo test -p nit-core substrate        # data model + serde + tolerant load
cargo test -p nit-core observer         # observer framework + repeat_failure + global_heat
cargo test -p nit-core arbiter          # arbiter framework + persistent_conflict + budget + cooldown
cargo test -p nit-core mood             # mood enum + auto-transition + manual override
cargo test -p nit-core metabolism       # wall-clock tick (expiry, prune, observer, no-gen-advance)
cargo test -p nit-core assumption       # assumption types + auto-invalidation on FileWrite
cargo test -p nit-core mission_memory   # indexing + retrieval + IDF + upsert dedup
```

All should be 0 failed. If anything red, the substrate layer has regressed — bisect starting from the most recent commit on `main`.

---

## 2. Test coverage map

| What it covers                          | Filter                        | File                                                   |
|-----------------------------------------|-------------------------------|--------------------------------------------------------|
| Substrate data model + round-trip serde | `substrate`                   | `crates/nit-core/src/tests/substrate.rs`               |
| FileWrite auto-claim + assumption invalidation | `file_write`           | `crates/nit-core/src/tests/agent_bus.rs`               |
| Observer framework + each observer       | `observer` / `framework`     | `crates/nit-core/src/tests/observers.rs`               |
| Arbiter framework + cooldown + budget    | `arbiter` / `persistent`     | `crates/nit-core/src/tests/arbiters.rs`                |
| Mood enum + auto-transition + modulations | `mood`                      | `crates/nit-core/src/tests/mood.rs`                    |
| Metabolism tick + no-gen-advance invariant | `metabolism` / `tick`      | `crates/nit-core/src/tests/metabolism.rs`              |
| Cross-mission memory + IDF               | `mission_memory` / `idf`     | `crates/nit-core/src/tests/mission_memory.rs`          |
| Substrate overlay tab cycle              | `substrate_overlay`          | `crates/nit-core/src/tests/state.rs`                   |
| Signals body rendering                   | `signals_view`               | `crates/nit-tui/src/widgets/signals_view.rs`           |
| Claims body rendering                    | `claims_view`                | `crates/nit-tui/src/widgets/claims_view.rs`            |
| Assumptions body rendering               | `assumptions_view`           | `crates/nit-tui/src/widgets/assumptions_view.rs`       |
| Substrate overlay popup                  | (manual)                     | `crates/nit-tui/src/widgets/substrate_overlay.rs`      |
| MCP JSON-RPC protocol                    | (nit-mcp package only)       | `crates/nit-mcp/src/server.rs` inline tests            |

---

## 3. Live observation — the TUI walkthrough

Run nit against a workspace:

```bash
cd /path/to/your/project
nit
```

On startup, nit loads `.nit/substrate/state.json` (or initializes fresh if absent).

### 3.1 Open the substrate overlay

Substrate inspection lives in a popup overlay, not in the Visualizer pane itself.

**Open**: press **F3**, or type `:substrate` / `:sub` / `:sig` / `:claims` / `:assumptions` / `:asm`.

The overlay shows a three-tab inline title:

```
 SUBSTRATE   SIGNALS   CLAIMS   ASSUMPTIONS    F3/Esc close   Tab: switch   j/k: scroll
```

Press `Tab` to cycle tabs; mouse-click on a tab label also cycles (clicking the active tab closes the overlay). `Esc` or `F3` closes it.

The Visualizer pane's title is always prefixed by a 4-character mood glyph — visible even when the overlay is closed:

- `[E.]` — Exploration
- `[C.]` — Consolidation (default)
- `[D!]` — Defensive

### 3.2 Observe a signal through its lifecycle

Drive a turn in nit (any agent, any prompt).

1. **Before turn completes**: the Signals sub-tab (F3 overlay) may show old signals from prior turns (decaying).
2. **Turn completes**: a new `DoneMarker` signal appears at the top (highest strength: 1.0 × 0.95⁰ = 1.0). Note: the `gen` counter in the header advances by 1.
3. **Subsequent turns**: the DoneMarker decays at rate 0.95/gen, dropping ~5% per turn.
4. **~60 turns later**: the DoneMarker falls below 0.05 threshold and is pruned at the next `TurnCompleted`.

To accelerate the lifecycle view, run `cargo test -p nit-core mood`:

```
test mood::tests::signal_decay_multiplier_affects_effective_strength ... ok
```

This confirms decay math matches the documented formula.

### 3.3 Trigger a claim violation (manual scenario)

Hard-to-trigger without multi-agent swarm setup. Instead, verify via the integration test:

```bash
cargo test -p nit-core file_write_auto_claim_conflict
```

Expected: the test seeds a pre-existing ExclusiveWrite claim by agent A on path P, fires a FileWrite from agent B to P, and asserts:
- A `ClaimViolation` signal is emitted by B.
- `state.pending_claim_retries` gains a `ClaimRetryRequest`.
- B's new claim is NOT inserted (conflict blocked it).

In a live TUI session with an actual agent conflict, you'd see:
- The `ClaimViolation` signal appear in the Signals sub-tab (F3 overlay) with `posted_by = <violator>`, `target = agent:<violator>`.
- The violating agent gets an auto-dispatched retry prompt: *"CLAIM VIOLATION: you wrote to X but Y holds an ExclusiveWrite claim. Rationale: …. Back off and coordinate — choose a different file or wait for the claim to expire."*

### 3.4 Watch a mood transition

Mood transitions are driven by substrate pressure. To force one:

```bash
# Option A: wait for real agent activity to accumulate warnings
# Option B: inspect the test
cargo test -p nit-core auto_transition_c_to_defensive_on_pressure -- --nocapture
```

In a live session, once 8 `ClaimViolation + Warning + HelpNeeded` signals accumulate within 10 generations, the next metabolic tick flips mood to Defensive. You'll see:

- The mood glyph in the Visualizer pane title flips from `[C.]` to `[D!]`.
- A new `Warning` signal appears on `Global` with payload `{"reason": "mood_auto_transition", "from": "consolidation", "to": "defensive", "pressure": N, "source": "auto"}`.
- The metabolic tick interval shortens from 5s to 3s — sweeps happen faster.
- The arbiter per-tick budget rises from 2 to 4.
- The observer `repeat_failure` threshold drops from 2 to 1 — HelpNeeded signals emit sooner.

Hysteresis prevents thrashing: to return to Consolidation, pressure must drop to ≤4.

### 3.5 See cross-mission memory in action

After running at least 2 missions in the same workspace:

```bash
# Inspect the index
cat .nit/memory/index.json | jq '.missions[] | {mission_id, title, tags: .tags[:10]}'
```

When you start a new swarm mission, the planner prompt now includes a section like:

```
Prior similar missions (read-only context — do not re-plan these, use as precedent):
- mis-003 [parallel, DONE]: Refactor crates/nit-gol module
    * File-by-file refactor plan for all 18 nit-gol files …
    * Introduced snapshot trait at catalog boundary …
    files: crates/nit-gol/src/analyze.rs, crates/nit-gol/src/catalog/mod.rs, …
```

This is injected automatically by the planner at `crates/nit-tui/src/swarm/prompts.rs::build_planner_prompt`. No user action required.

### 3.6 Inspect substrate state directly

```bash
cat .nit/substrate/state.json | jq '{ generation, mood, mood_quiet_streak,
    signals_count: (.signals | length),
    claims_count: (.claims | length),
    assumptions_count: (.assumptions | length) }'
```

Example output after a few turns:

```json
{
  "generation": 14,
  "mood": "consolidation",
  "mood_quiet_streak": 2,
  "signals_count": 6,
  "claims_count": 2,
  "assumptions_count": 0
}
```

Detailed signal inspection:

```bash
cat .nit/substrate/state.json | jq '.signals | to_entries | map({
    id: .key,
    kind: .value.kind,
    by: .value.posted_by,
    target: .value.target,
    gen: .value.posted_at_gen,
    strength: .value.initial_strength
})'
```

---

## 4. MCP tool examples (deliberate agent emission)

Prerequisite: Unix host. Codex must support `-c mcp_servers.<name>=...` overrides.

When nit spawns Codex with the nit-mcp config injected, Codex exposes three tools to the model. From the model's perspective, a tool call looks like:

### 4.1 `emit_signal`

```json
{
  "name": "emit_signal",
  "arguments": {
    "kind": "deadend",
    "target": { "kind": "file", "path": "crates/nit-core/src/foo.rs" },
    "payload": {
      "tried": "Extracting trait Foo; the trait bounds conflict with existing generics.",
      "suggestion": "Consider enum dispatch instead."
    },
    "strength": 1.2
  }
}
```

After apply, the substrate gains a Signal with `kind: Deadend`, `posted_by: "codex-session"` (or whatever `NIT_MCP_AGENT_ID` was set to at spawn), `initial_strength: 1.2`. The Signals sub-tab (F3 overlay) shows it immediately; it decays at rate 0.9/gen.

### 4.2 `assert_claim`

```json
{
  "name": "assert_claim",
  "arguments": {
    "kind": "exclusive_write",
    "target": {
      "kind": "region",
      "path": "crates/nit-core/src/substrate/signals.rs",
      "start_line": 100,
      "end_line": 180
    },
    "ttl_gens": 5,
    "rationale": "Refactoring the decay math; do not edit this region for the next 5 turns."
  }
}
```

After apply, the claim is inserted with the requested TTL multiplied by the current mood's `claim_ttl_multiplier`:

- Consolidation: `ttl_gens = (5 * 1.0).max(1) = 5`
- Defensive: `ttl_gens = (5 * 1.5).max(1) = 7`
- Exploration: `ttl_gens = (5 * 0.75).max(1) = 3`

Any subsequent FileWrite by another agent to that region (or the whole file) emits a ClaimViolation.

### 4.3 `assert_assumption`

```json
{
  "name": "assert_assumption",
  "arguments": {
    "target": {
      "kind": "file",
      "path": "crates/nit-core/src/substrate/signals.rs"
    },
    "fact": {
      "kind": "api_signature",
      "snapshot": "fn prune_signals_below(&mut self, threshold: f32) -> usize"
    },
    "ttl_gens": 10,
    "rationale": "Plan assumes prune_signals_below keeps this exact signature."
  }
}
```

If any other agent subsequently writes to `crates/nit-core/src/substrate/signals.rs` before the assumption expires, the assumption is removed and a Warning signal is posted **to the original asserter** (`target: agent:codex-session`) with the full removed assumption in the payload — so the agent learns its world-model is stale.

### 4.4 Error shapes

Malformed arguments: MCP response with error code `-32602` ("Invalid params"), message containing the serde error text.

Unknown tool name: code `-32601` ("Method not found").

Back-channel timeout (nit-tui not responding): code `-32603` ("Internal error"), message `"event channel closed"` or timeout text.

---

## 5. Concrete test scenarios you can run right now

### Scenario A — "Signal lifecycle end-to-end"

```bash
cargo test -p nit-core -- --nocapture \
    signal_round_trip_serialization \
    decay_monotonic_lazy \
    decay_rate_varies_by_kind \
    prune_removes_below_threshold
```

Confirms: serde preserves signals; decay is monotonic; HelpNeeded fades faster than DoneMarker; pruning drops sub-threshold signals.

### Scenario B — "Claim conflict + retry is really wired"

```bash
cargo test -p nit-core file_write_auto_claim_conflict_emits_violation_and_queues_retry
```

Asserts the full chain: FileWrite → auto-claim attempt → conflict detected → ClaimViolation signal emitted → `pending_claim_retries` populated.

### Scenario C — "Assumption invalidation end-to-end"

```bash
cargo test -p nit-core file_write_invalidates_overlapping_assumption_and_emits_warning
cargo test -p nit-core file_write_invalidates_assumption_even_when_auto_claim_conflicts
```

Asserts: write to assumed path → assumption removed → Warning signal targets the original poster (not the writer).

### Scenario D — "Arbiter actually dispatches retries"

```bash
cargo test -p nit-core turn_completed_integration_queues_intervention
cargo test -p nit-core intervention_downgrades_to_signal_only_when_retry_budget_exhausted
```

Asserts: 3 mutual ClaimViolations between a pair → persistent_conflict arbiter emits `InterventionEmitted` signal AND pushes `Intervention` onto `pending_interventions`. When `genome_retry_count ≥ ARBITER_RETRY_LIMIT`, the intervention kind downgrades to `EmitSignalOnly`.

### Scenario E — "Mood actually modulates behavior"

```bash
cargo test -p nit-core auto_transition_c_to_defensive_on_pressure
cargo test -p nit-core manual_override_blocks_auto_transition
cargo test -p nit-core metabolism_reads_mood_adjusted_interval
cargo test -p nit-core observer_repeat_failure_uses_mood_threshold
cargo test -p nit-core file_write_auto_claim_ttl_respects_mood
```

Five tests chain the full modulation story: pressure → Defensive → shortened tick interval → observer fires at lower threshold → auto-claim TTL stretches.

### Scenario F — "Metabolism runs on wall clock, NOT turns"

```bash
cargo test -p nit-core tick_does_not_advance_generation
cargo test -p nit-core tick_expires_claims_past_ttl
cargo test -p nit-core tick_prunes_decayed_signals
cargo test -p nit-core tick_is_noop_when_idle
cargo test -p nit-core tick_saves_only_when_dirty
```

Confirms the invariant: `tick()` sweeps stale state but **does not touch the generation counter**. Noop ticks do not write to disk.

### Scenario G — "Mission memory actually indexes and retrieves"

```bash
cargo test -p nit-core build_index_from_corpus_fixture
cargo test -p nit-core retrieve_returns_expected_ordering
cargo test -p nit-core retrieve_path_bonus_boosts_file_overlap
cargo test -p nit-core idf_weight_prefers_rare_matches
cargo test -p nit-core upsert_mission_dedupes_by_id
```

Verifies the full index + query pipeline with fixture missions.

### Scenario H — "MCP server speaks the protocol correctly"

```bash
cargo test -p nit-mcp
```

Six tests cover: `initialize` handshake, `tools/list` returns three tools, each tool call builds the correct back-channel request, malformed args return `-32602`, unknown tool returns `-32601`.

---

## 6. Sample on-disk state after a full session

**`.nit/substrate/state.json`** (pretty-printed, abridged):

```json
{
  "generation": 27,
  "signals": {
    "27-gpt-test-0": {
      "id": "27-gpt-test-0",
      "kind": "done_marker",
      "posted_by": "gpt-test",
      "posted_at_gen": 27,
      "target": { "kind": "agent", "agent_id": "gpt-test" },
      "initial_strength": 1.0,
      "payload": { "message": "Applied refactor.", "thread_id": "thread-xyz", "mission_id": "mis-007" }
    },
    "24-observer:repeat_failure-0": {
      "id": "24-observer:repeat_failure-0",
      "kind": "help_needed",
      "posted_by": "observer:repeat_failure",
      "posted_at_gen": 24,
      "target": { "kind": "agent", "agent_id": "claude-opus" },
      "initial_strength": 1.5,
      "payload": { "reason": "repeat_failure", "warning_count": 2, "window_gens": 5, "agent_id": "claude-opus" }
    }
  },
  "claims": {
    "25-gpt-test-0": {
      "id": "25-gpt-test-0",
      "kind": "exclusive_write",
      "target": { "kind": "file", "path": "src/main.rs" },
      "claimed_by": "gpt-test",
      "claimed_at_gen": 25,
      "ttl_gens": 3,
      "rationale": "auto-claim from FileWrite"
    }
  },
  "assumptions": {},
  "observations": [],
  "signal_counter": 18,
  "claim_counter": 5,
  "assumption_counter": 0,
  "mood": "consolidation",
  "mood_override_until_gen": 0,
  "mood_quiet_streak": 1
}
```

**`.nit/memory/index.json`** (abridged):

```json
{
  "version": 1,
  "missions": [
    {
      "mission_id": "mis-001",
      "title": "Swarm[parallel]: refactor crates/nit-gol module",
      "template": "parallel",
      "status": "DONE",
      "updated_at": "t+70937",
      "task_ids": ["propose-nit-gol-plan", "integrate-analysis", "review-nit-gol"],
      "task_titles": ["Survey nit-gol", "..."],
      "task_summaries": ["File-by-file refactor plan ...", "..."],
      "files_touched": ["crates/nit-gol/src/analyze.rs", "crates/nit-gol/src/catalog/mod.rs"],
      "tags": ["analyze", "catalog", "gol", "nit-gol", "refactor", "snapshot", "..."]
    }
  ]
}
```

---

## 7. Debugging tips

### "A signal isn't decaying the way I expect"

Effective strength uses mood-adjusted decay. Check the current mood:

```bash
jq '.mood' .nit/substrate/state.json
```

Formula: `effective = initial * (decay_rate / mood.signal_decay_multiplier)^(current_gen - posted_at_gen)`.

Defensive mood's `0.85` multiplier *divides* into the base decay rate — so Warning's 0.8 becomes effectively `0.8 / 0.85 ≈ 0.94`, slowing decay. A common confusion: smaller multiplier does NOT mean faster decay.

### "A claim I asserted isn't in `claims_iter`"

`claims_iter` filters expired claims. Check `claimed_at_gen + ttl_gens` against `current_gen`:

```bash
jq '.claims | to_entries | map({id: .key, expires: (.value.claimed_at_gen + .value.ttl_gens)})' \
    .nit/substrate/state.json
jq '.generation' .nit/substrate/state.json
```

### "My observer isn't emitting"

Two common causes:
- **Self-silencing**: most observers check for recent observer-emitted signals targeting the same entity before re-emitting. See each observer's cooldown logic.
- **Mood threshold**: `repeat_failure`'s threshold depends on mood. At `Exploration`, you need 3 warnings to trigger; at `Defensive`, just 1.

### "Metabolism doesn't seem to be running"

Metabolism runs on every TUI frame AFTER the current interval has elapsed. Check:
- Process is running (metabolism is in-process, not cron).
- Mood's tick interval: 3s / 5s / 10s.
- Nothing triggers redraw unless the tick is non-noop — idle ticks are silent.

Force a visible tick by introducing substrate state that will change:

```bash
# Modify state.json to have an expired claim, then let metabolism expire it
jq '.claims["forced"] = {id: "forced", kind: "exclusive_write", target: {kind:"file", path:"nonexistent"},
    claimed_by:"test", claimed_at_gen: 0, ttl_gens: 1, rationale: "test"}' .nit/substrate/state.json > tmp.json
mv tmp.json .nit/substrate/state.json
# Restart nit and wait 5s — the "forced" claim should be gone from the Claims sub-tab (F3 overlay)
```

### "MCP tool calls aren't reaching nit"

Likely causes:
1. **Unix-only**: Windows builds compile but have no listener.
2. **`nit-mcp-server` binary not found**: check `target/release/nit-mcp-server` exists next to the `nit` binary.
3. **Socket path mismatch**: check the env var `NIT_MCP_BACKCHANNEL_SOCKET` is being passed through to Codex's child process config.
4. **Codex `-c` syntax**: the inline-TOML override is a best-guess in v1; inspect `codex mcp list --json` against a running nit instance to verify the `nit` entry appears.

---

## 8. Extending safely

When you add a new primitive (observer / arbiter / mood modulation / MCP tool), the tests you MUST add:

1. **A silent-case test** — asserts the primitive does not fire when its trigger condition is not met.
2. **A positive-case test** — asserts it does fire when the condition is met, with the expected payload.
3. **A self-silencing / cooldown test** — asserts repeat triggers within the cooldown window don't re-emit.
4. **An integration test** — asserts the primitive runs via `TurnCompleted` and/or `metabolism::tick`.

See `crates/nit-core/src/tests/observers.rs` and `arbiters.rs` for templates.

---

## 9. Common gotchas

- **Generation ≠ wall-clock seconds**. Metabolism moves forward on wall clock; generation only advances at `TurnCompleted`. A 1-hour idle session does not age signals by 1 hour worth of decay — the decay stays at whatever gen you left off at (it's turn-relative).
- **`FileWrite` events are observed, not generated**. nit does not own the write path. If your test harness doesn't fire `FileWrite` events, the substrate won't see file activity.
- **`posted_by` attribution is coarse in MCP**. All MCP-originated emissions carry the session-level `NIT_MCP_AGENT_ID`. Per-turn attribution isn't available in v1.
- **Claim violations do NOT roll back writes**. The violating FileWrite has already hit disk. Claim enforcement is advisory — violations trigger retries, not rollbacks.
- **The mood decay multiplier is DIVIDED, not multiplied**. Slower decay ⇒ larger effective rate ⇒ multiplier `< 1.0` divides *down* the rate ⇒ slower decay. (Yes, this was a bug caught during implementation; the correct direction is now baked in.)
- **Observations slot is reserved but unused**. Don't write to it yet — a future phase may type it.
- **`assert_claim` from MCP DOES honor mood TTL multiplier; the older `AssertClaim` variant (fully-formed claim) does NOT**. This is deliberate — one comes from an agent's request (respects system mood), the other is pre-formed and passes through as-is.
