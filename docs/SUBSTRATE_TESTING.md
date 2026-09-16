# Substrate Testing

How to run and verify nit's living-system layer: the test commands and filters, a coverage map, a TUI walkthrough, `nit-mcp` tool examples, runnable scenarios, sample on-disk state, and debugging tips. It is for contributors. [SUBSTRATE.md](SUBSTRATE.md) is the architecture reference.

## 1. Quick verification

From the nit workspace root:

```bash
# All tests, all crates (slowest)
cargo test --workspace

# Per crate
cargo test -p nit-core
cargo test -p nit-tui
cargo test -p nit-mcp

# Substrate-specific filters
cargo test -p nit-core substrate        # data model + serde + tolerant load
cargo test -p nit-core observer         # observer framework + each registered observer
cargo test -p nit-core arbiter          # arbiter framework + persistent_conflict + budget + cooldown
cargo test -p nit-core mood             # mood enum + auto-transition + manual override
cargo test -p nit-core metabolism       # wall-clock tick (expiry, prune, observers, no gen advance)
cargo test -p nit-core assumption       # assumption types + auto-invalidation on FileWrite
cargo test -p nit-core mission_memory   # indexing + retrieval + IDF + upsert dedup
```

All of these should pass. Run `cargo test --all` for the live count. If anything fails, the substrate layer has regressed; bisect from the most recent commit on `main`.

## 2. Test coverage map

| What it covers | Filter | File |
|---|---|---|
| Substrate data model + round-trip serde | `substrate` | `crates/nit-core/src/tests/substrate.rs` (signals), `tests/substrate/claims.rs`, `tests/substrate/assumptions.rs` |
| FileWrite auto-claim + assumption invalidation | `file_write` | `crates/nit-core/src/tests/agent_bus.rs` |
| Observer framework + each observer | `observer` / `framework` | `crates/nit-core/src/tests/observers.rs` |
| Arbiter framework + cooldown + budget | `arbiter` / `persistent` | `crates/nit-core/src/tests/arbiters.rs` |
| Mood enum + auto-transition + modulations | `mood` | `crates/nit-core/src/tests/mood.rs` |
| Metabolism tick + no-gen-advance invariant | `metabolism` / `tick` | `crates/nit-core/src/tests/metabolism.rs` |
| Cross-mission memory + IDF | `mission_memory` / `idf` | `crates/nit-core/src/tests/mission_memory.rs` |
| Substrate overlay tab cycle | `substrate_overlay` | `crates/nit-core/src/tests/state/help_and_commands.rs` |
| Signals body rendering | `signals_view` | `crates/nit-tui/src/widgets/signals_view.rs` |
| Claims body rendering | `claims_view` | `crates/nit-tui/src/widgets/claims_view.rs` |
| Assumptions body rendering | `assumptions_view` | `crates/nit-tui/src/widgets/assumptions_view.rs` |
| Substrate overlay popup | (manual) | `crates/nit-tui/src/widgets/substrate_overlay.rs` |
| MCP JSON-RPC protocol | (nit-mcp package only) | `crates/nit-mcp/tests/discovery.rs`, `crates/nit-mcp/tests/tools_call.rs` |

## 3. TUI walkthrough

Run nit in a workspace:

```bash
cd /path/to/your/project
nit
```

On startup nit loads `.nit/substrate/state.json`, or starts with an empty substrate if the file is absent.

### 3.1 Open the substrate overlay

Press `F3` or `Ctrl+Space`, or type `:substrate`, `:sub`, `:sig`, `:signals`, `:claims`, `:assumptions`, or `:asm`. The overlay title shows three tabs:

```text
 SUBSTRATE   SIGNALS   CLAIMS   ASSUMPTIONS    F3/Esc close   Tab: switch   j/k: scroll
```

`Tab` cycles tabs; clicking a tab label also cycles, and clicking the active tab closes the overlay. `F3`, `Esc`, `q`, or `Ctrl+Space` closes it. The top bar always shows the current mood, even with the overlay closed:

- `MOOD: EXPLORATION`
- `MOOD: CONSOLIDATION` (default)
- `MOOD: DEFENSIVE`

### 3.2 Follow a signal through its lifecycle

Run a turn in nit with any agent and any prompt.

1. Before the turn completes, the Signals tab may show old, decaying signals from earlier turns.
2. When the turn completes, a new `DoneMarker` appears at the top with strength 1.0 (1.0 × 0.95^0). The `gen` counter in the header advances by 1.
3. On each later turn the `DoneMarker` decays by 0.95 per generation, about 5 percent per turn.
4. After about 60 turns it drops below the 0.05 threshold and is pruned at the next `TurnCompleted`.

To check the decay math without waiting, run `cargo test -p nit-core mood` and look for:

```text
test mood::tests::signal_decay_multiplier_affects_effective_strength ... ok
```

### 3.3 Trigger a claim violation

This is hard to trigger by hand without a multi-agent swarm. Use the integration test instead:

```bash
cargo test -p nit-core file_write_auto_claim_conflict
```

The test seeds an `ExclusiveWrite` claim by agent A on path P, fires a `FileWrite` from agent B to P, and asserts:

- B emits a `ClaimViolation` signal.
- `state.pending_claim_retries` gains a `ClaimRetryRequest`.
- B's new claim is not inserted.

In a live session with a real conflict you see:

- The `ClaimViolation` signal in the Signals tab, with `posted_by = <violator>` and `target = agent:<violator>`.
- The violating agent gets an automatic retry prompt: `CLAIM VIOLATION: you wrote to X but Y holds an ExclusiveWrite claim. Rationale: …. Back off and coordinate — choose a different file or wait for the claim to expire.`

### 3.4 Watch a mood transition

Substrate pressure drives mood transitions. To see one, either wait for real agent activity to accumulate warnings, or run the test:

```bash
cargo test -p nit-core auto_transition_consolidation_to_defensive_at_pressure_threshold -- --nocapture
```

In a live session, once 8 `ClaimViolation`, `Warning`, or `HelpNeeded` signals accumulate within 10 generations, the next metabolic tick flips the mood to Defensive. You see:

- The top-bar badge changes from `MOOD: CONSOLIDATION` to `MOOD: DEFENSIVE`.
- A new `Warning` signal on `Global`, posted by `mood`, with payload `{"reason": "mood_auto_transition", "from": "consolidation", "to": "defensive", "pressure": N, "source": "auto"}`.
- The metabolic tick interval shortens from 5s to 3s.
- The arbiter per-tick budget rises from 2 to 4.
- The `repeat_failure` observer threshold drops from 2 to 1, so `HelpNeeded` signals fire sooner.

Hysteresis prevents thrashing: pressure must drop to 4 or less before the mood returns to Consolidation.

### 3.5 See cross-mission memory in action

After at least 2 missions in the same workspace:

```bash
# Inspect the index
cat .nit/memory/index.json | jq '.missions[] | {mission_id, title, tags: .tags[:10]}'
```

When you start a new swarm mission, the planner prompt includes a section like:

```text
Prior similar missions (read-only context — do not re-plan these, use as precedent):
- mis-003 [parallel, DONE]: Refactor crates/nit-gol module
    * File-by-file refactor plan for all 18 nit-gol files …
    * Introduced snapshot trait at catalog boundary …
    files: crates/nit-gol/src/analyze.rs, crates/nit-gol/src/catalog/mod.rs, …
```

`build_planner_prompt` in `crates/nit-tui/src/swarm/prompts.rs` injects this. No user action is needed.

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

Per-signal detail:

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

## 4. MCP tool examples

Prerequisites: a Unix host, and a Codex build that supports `-c mcp_servers.<name>=...` overrides. When nit spawns Codex with the nit-mcp config, Codex exposes three tools to the model. From the model's side a tool call looks like this.

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

After apply, the substrate holds a `Signal` with `kind: Deadend`, `posted_by: "codex-session"` (or whatever `NIT_MCP_AGENT_ID` was at spawn), and `initial_strength: 1.2`. The Signals tab shows it at once. It decays at 0.9 per generation.

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

After apply, the claim's TTL is the requested value times the current mood's `claim_ttl_multiplier`:

- Consolidation: `ttl_gens = (5 * 1.0).max(1) = 5`
- Defensive: `ttl_gens = (5 * 1.5).max(1) = 7`
- Exploration: `ttl_gens = (5 * 0.75).max(1) = 3`

Any later `FileWrite` by another agent to that region, or to the whole file, emits a `ClaimViolation`.

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

If another agent writes to `crates/nit-core/src/substrate/signals.rs` before the assumption expires, nit removes the assumption and posts a `Warning` to the original asserter (`target: agent:codex-session`) with the removed assumption in the payload. The agent learns that its world model is stale.

### 4.4 Error shapes

- Malformed arguments: code `-32602` (invalid params). The message contains the serde error text.
- Unknown tool name: code `-32601` (method not found).
- Back-channel failure: code `-32603` (internal error). The message is `back-channel error: ...` when the socket connect or I/O fails (2 second timeout), or `nit event channel closed` when nit-tui's event channel is gone.

## 5. Scenarios you can run now

### A. Signal lifecycle end to end

```bash
cargo test -p nit-core -- --nocapture \
    signal_round_trip_serialization \
    decay_is_monotonic_and_lazy \
    decay_rate_varies_by_kind \
    prune_removes_below_threshold
```

Confirms: serde preserves signals; decay is monotonic; `HelpNeeded` fades faster than `DoneMarker`; pruning drops signals under the threshold.

### B. Claim conflict and retry are wired

```bash
cargo test -p nit-core file_write_auto_claim_conflict_emits_violation_and_queues_retry
```

Asserts the full chain: `FileWrite`, auto-claim attempt, conflict detected, `ClaimViolation` emitted, `pending_claim_retries` populated.

### C. Assumption invalidation end to end

```bash
cargo test -p nit-core file_write_invalidates_overlapping_assumption_and_emits_warning
cargo test -p nit-core file_write_invalidates_assumption_even_when_auto_claim_conflicts
```

Asserts: a write to an assumed path removes the assumption, and the `Warning` targets the original poster, not the writer.

### D. Arbiters dispatch retries

```bash
cargo test -p nit-core turn_completed_integration_queues_intervention
cargo test -p nit-core intervention_downgrades_to_signal_only_when_retry_budget_exhausted
```

Asserts: 3 mutual `ClaimViolation`s between a pair make `persistent_conflict` emit an `InterventionEmitted` signal and push an `Intervention` onto `pending_interventions`. When `genome_retry_count >= ARBITER_RETRY_LIMIT`, the intervention downgrades to `EmitSignalOnly`.

### E. Mood modulates behavior

```bash
cargo test -p nit-core auto_transition_consolidation_to_defensive_at_pressure_threshold
cargo test -p nit-core manual_override_blocks_auto_transition
cargo test -p nit-core metabolism_reads_mood_adjusted_interval
cargo test -p nit-core observer_repeat_failure_uses_mood_threshold
cargo test -p nit-core file_write_auto_claim_ttl_respects_mood
```

Together these cover the chain: pressure raises Defensive, the tick interval shortens, the observer fires at a lower threshold, and auto-claim TTLs stretch.

### F. Metabolism runs on the wall clock, not turns

```bash
cargo test -p nit-core tick_does_not_advance_generation
cargo test -p nit-core tick_expires_claims_past_ttl
cargo test -p nit-core tick_prunes_decayed_signals
cargo test -p nit-core tick_is_noop_when_idle
cargo test -p nit-core tick_saves_only_when_dirty
```

Confirms: `tick()` sweeps stale state but never touches the generation counter, and noop ticks do not write to disk.

### G. Mission memory indexes and retrieves

```bash
cargo test -p nit-core build_index_from_corpus_fixture
cargo test -p nit-core retrieve_returns_expected_ordering
cargo test -p nit-core retrieve_path_bonus_boosts_file_overlap
cargo test -p nit-core idf_weight_prefers_rare_matches
cargo test -p nit-core upsert_mission_dedupes_by_id
```

Verifies the index and query pipeline against fixture missions.

### H. The MCP server speaks the protocol

```bash
cargo test -p nit-mcp
```

Covers the `initialize` handshake, `tools/list` returning three tools, each tool call building the right back-channel request, malformed arguments returning `-32602`, and an unknown tool returning `-32601`.

## 6. Sample on-disk state

`.nit/substrate/state.json`, abridged to one signal and one claim:

```json
{
  "generation": 27,
  "signals": {
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

`.nit/memory/index.json`, abridged to one mission:

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

## 7. Debugging tips

### "A signal isn't decaying the way I expect"

Effective strength uses the mood-adjusted decay rate. Check the mood:

```bash
jq '.mood' .nit/substrate/state.json
```

Formula: `effective = initial * (decay_rate / mood.signal_decay_multiplier)^(current_gen - posted_at_gen)`.

Defensive mood's `0.85` multiplier divides the base rate, so Warning's 0.8 becomes about `0.8 / 0.85 = 0.94`, which slows decay. A smaller multiplier does not mean faster decay.

### "A claim I asserted isn't in `claims_iter`"

`claims_iter` skips expired claims. Compare `claimed_at_gen + ttl_gens` with `current_gen`:

```bash
jq '.claims | to_entries | map({id: .key, expires: (.value.claimed_at_gen + .value.ttl_gens)})' \
    .nit/substrate/state.json
jq '.generation' .nit/substrate/state.json
```

### "My observer isn't emitting"

Two common causes:

- Self-silencing: most observers check for a recent observer-emitted signal on the same target before emitting again. See each observer's cooldown logic.
- Mood threshold: `repeat_failure` needs 3 warnings in Exploration, 2 in Consolidation, and 1 in Defensive.

### "Metabolism doesn't seem to be running"

Metabolism runs on the first TUI frame after the current interval has elapsed. Check:

- The process is running. Metabolism is in-process, not cron.
- The mood's tick interval: 10s Exploration, 5s Consolidation, 3s Defensive.
- Idle ticks are silent. Nothing redraws unless the tick changed something.

Force a visible tick by adding state that must change:

```bash
# Add an already-expired claim, then let metabolism expire it
jq '.claims["forced"] = {id: "forced", kind: "exclusive_write", target: {kind:"file", path:"nonexistent"},
    claimed_by:"test", claimed_at_gen: 0, ttl_gens: 1, rationale: "test"}' .nit/substrate/state.json > tmp.json
mv tmp.json .nit/substrate/state.json
# Restart nit and wait one tick (5s in Consolidation): the "forced" claim disappears from the Claims tab
```

### "MCP tool calls aren't reaching nit"

Likely causes:

1. Unix only: Windows builds compile but have no listener.
2. `nit-mcp-server` not found: check that `target/release/nit-mcp-server` exists next to the `nit` binary.
3. Socket path mismatch: check that `NIT_MCP_BACKCHANNEL_SOCKET` reaches Codex's child process config.
4. Codex `-c` syntax: the inline TOML override is a best guess in v1. Run `codex mcp list --json` against a running nit instance and check that the `nit` entry appears.

## 8. Extending safely

When you add a primitive (observer, arbiter, mood modulation, or MCP tool), add these tests:

1. A silent case: the primitive does not fire when its trigger condition is not met.
2. A positive case: it fires when the condition is met, with the expected payload.
3. A self-silencing or cooldown case: repeat triggers inside the cooldown window do not fire again.
4. An integration case: the primitive runs through `TurnCompleted` and/or `metabolism::tick`.

See `crates/nit-core/src/tests/observers.rs` and `arbiters.rs` for templates.

## 9. Common gotchas

- Generation is not wall-clock seconds. Metabolism moves on the wall clock; the generation advances only at `TurnCompleted`. An hour of idle time does not age signals, because decay is turn-relative.
- `FileWrite` events are observed, not generated. nit does not own the write path. If your test harness does not fire `FileWrite` events, the substrate never sees file activity.
- `posted_by` is coarse for MCP. Every MCP emission carries the session-level `NIT_MCP_AGENT_ID`. There is no per-turn attribution in v1.
- Claim violations do not roll back writes. The violating write has already hit disk. Enforcement is advisory: violations trigger retries, not rollbacks.
- The mood decay multiplier divides, it does not multiply. A multiplier below 1.0 raises the effective retention rate, which slows decay.
- The observations slot is reserved and unused. Do not write to it yet.
- `assert_claim` from MCP applies the mood TTL multiplier; the fully formed `AssertClaim` event does not. One is an agent's request and follows the system mood; the other is a pre-formed record and passes through as is.
