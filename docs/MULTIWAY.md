# Multiway Engine

> **Status**: All phases (0–7) shipped. **Phase 0 (engine core +
> `GitWorldStore`), Phase 1 (genome+gates value function), Phase 2 (best-first
> frontier + budget + emergent backtrack), Phase 3 (real agent executor +
> `NIT_MULTIWAY` + `@multiway`), Phase 4 (adjudicated merge + per-branch
> substrate), Phase 5 (linear-vs-multiway measurement harness), Phase 6
> (multiway graph view — on-demand Graphviz render + live popup), and Phase 7
> (`mode=multiway` across the dispatch commands) are landed** — green under
> `cargo test -p nit-multiway` and `-p nit-tui`, clippy/fmt clean. Phase 2's
> deterministic trap test proves a multiway search reaches the global optimum
> where an equal-budget greedy search stalls; Phase 4's `tests/merge.rs` proves
> complementary branches merge into a higher-value join, a conflicting pair is
> adjudicated into a two-parent node, and an unresolved (marker-leaving) merge
> is gated without aborting the search; and the persisted DAG — merge node and
> all — round-trips through serde. `cargo run -p nit-multiway --example
> store_smoke -- .` still proves snapshot → fork → commit → merge → restore on a
> real repo leaves the operator tree byte-identical and cleans up its refs, and
> `cargo run -p nit-multiway --example multiway_bench` emits the matched-budget
> linear-vs-multiway comparison. Phase 6 makes a run legible — an on-demand
> `@multiway-graph` DOT/PNG render and a live `Ctrl+Shift+M` popup that streams the
> search tree as the agents reason; Phase 7 reaches the same engine through
> `@shadow` / `@swarm` / `@all` with a uniform `mode=multiway` modifier, default
> behaviour byte-identical without the token. Background model + visualisation:
> `nit-blogs/thesis/multiway/` (`Results-01.nb`, `Framework-01.nb`); the
> empirical basis for "genome = value function, not a grade" is `gol-novelty/`.

## Running a multiway mission (`@multiway`)

The engine is **off by default**; opt in with `NIT_MULTIWAY=1` (see the env-var
tables in `CLAUDE.md` / `docs/ENVIRONMENT.md`). With the flag on, dispatch a
best-first search from Agent Chat:

```
@multiway [mood=explore|balanced|exploit] [k=N] <task>
```

- `mood` sets the frontier trade-off — `explore` keeps more branches alive,
  `exploit` greedily follows the current best, `balanced` (the default) sits
  between.
- `k` is the fork fan-out per expansion (default 3, clamped to the effective
  swarm ceiling); the run is otherwise bounded by a default budget (64 nodes /
  32 turns).

Every turn runs in an isolated git worktree under
`<state_dir>/multiway/worktrees/<mission>/…`; the operator's working tree is
read-only to the engine. The DAG persists to
`<state_dir>/multiway/<mission>.json` and progress shows on a status line.
`/abort` (or `/abort all`) tears the worktrees down and prunes
`refs/nit-multiway/<mission>/*`. v1 is **Claude-only**, **single-pane**, and
expands the *k* candidates **serially** (one in-flight turn). With the flag
off, `@multiway …` is dispatched as an ordinary chat prompt — the code path is
byte-identical to today.

### Reaching multiway from `@shadow` / `@swarm` / `@all`

`@multiway` is the explicit canonical form; the same engine is reachable from the
other front-ends with a uniform `mode=multiway` modifier (Phase 7), so you keep a
command's ergonomics and route into the search:

- `@shadow mode=multiway <task>` — a `k=2`, `balanced` search whose merge oracle
  is the shadow judge.
- `@swarm [N] [template=lab|parallel|bulk] mode=multiway <task>` — `N`/`template`
  map to `k`/`mood` (`parallel`/`bulk` → `explore`, `lab` → `balanced`).
- `@all mode=multiway <task>` — one wide expansion with `k` = the fan-out count,
  value-ranked.

The modifier is **strictly additive**: without `mode=multiway` (or with
`NIT_MULTIWAY` off) every command parses and dispatches exactly as today — the
token is detected and stripped only inside the `multiway_enabled` guard.

### Viewing the search (graph view)

With the flag on, a running search is legible two ways (Phase 6):

- `@multiway-graph` renders the most recently persisted run's DAG to Graphviz and
  opens it in the OS image viewer (spawns `dot -Tpng`; saves the `.dot` and
  surfaces its path when graphviz is absent).
- `Ctrl+Shift+M` (or the `@multiway-popup` command) toggles a live popup (also
  auto-opened when a search starts) that streams the whole search tree — every
  branch, its value/tier, the gate-failures, the kept path, and the in-flight turn —
  updating as expansions land. Close and reopen are non-destructive. See
  `docs/KEYBINDINGS.md`.

## Dispatching this plan

Dispatch **one phase at a time**:
`@swarm mission=general Implement Phase N from docs/MULTIWAY.md`.

Phases 0–2 are a pure, **LLM-free** engine you can unit-test before spending a
token. Stop after each phase, run its acceptance checks, iterate, then go on.

## Vision

Today nit is a **linear single-writer** system: one shared working directory
(`SwarmRun.spawn_cwd`, bound by `cmd.current_dir(cwd)` in `claude_runner.rs`),
a **global** `SubstrateState` whose claims are **locks** (`claims_conflict` →
`ClaimViolation`; see `docs/SUBSTRATE.md`), per-agent **append-only** sessions,
a judge that picks a **text** plan (`docs/SHADOWS.md`), and a single-writer
integrator (`INTEGRATOR_MAX_TURNS = 500`; `docs/SWARM.md`). `/abort` kills with
no rollback. Parallelism is read-only side-tasks over one mutable timeline.

The machine we want is a **parallel best-first search over a non-confluent
rewrite system on content-addressed world states**. Five primitives:

1. **State** — an immutable, content-addressed world (a git commit), carrying a snapshot of the substrate.
2. **Turn** — a rewrite `state → state'` (one agent turn in a worktree, committed).
3. **Fork** — one state → *k* candidate successor states (parallel speculative turns).
4. **Value** — a function ranking states: **`nit_core::compute_genome_report`** (the genome — its true role is the search heuristic, not an aesthetic grade) plus hard build/test gates.
5. **Merge** — an *adjudicated* join of two states. Code edits are **not confluent** (no canonical normal form), so the **judge/arbiter is the merge oracle** — reuse the shadow judge machinery.

A **frontier policy** (driven by `mood`) chooses which state to expand and
**backtracks** to held states when a branch stalls. History becomes a **DAG**
of commits, not a line.

## Non-negotiables

1. **Additive and opt-in.** The existing linear swarm stays the default and
   untouched. Gate everything behind `NIT_MULTIWAY` (default `0`/off). The
   `NIT_MULTIWAY=0` path must be **byte-identical** to today — same rollback
   discipline as `NIT_CLAUDE_POOL=0` / `NIT_PLANNER_LEGACY=1` /
   `NIT_PROMPT_TIERS` off-path. Resolve the flag once at runtime construction;
   a mid-mission env change must not flip behaviour.
2. **`just ci` is the bar.** `cargo fmt -- --check` (hard gate), `cargo clippy
   --all-targets --all-features -- -D warnings` (zero warnings), `cargo test
   --all`, `cargo deny`. During development use targeted `cargo test -p
   <crate>`; run the full suite before declaring a phase done.
3. **`--locked`.** Adding the new crate updates `Cargo.lock` once,
   intentionally — call it out. Do **not** add third-party deps casually;
   prefer `std` + existing workspace deps (`serde`, `serde_json`, `blake3`,
   `nit-utils`). No new network deps — nit makes no network calls; spawn
   `git` / `claude` / `codex` directly, never via a shell.
4. **MSRV 1.88.0.** No newer-than-1.88 language/stdlib features.
5. **Document env vars in both places** — the `CLAUDE.md` table *and*
   `docs/ENVIRONMENT.md` (keep them in sync).

## Safety (correctness, not style)

- **Never mutate, `checkout`, or `reset` the operator's primary working
  tree.** The live editor owns it and auto-saves over disk; a git revert there
  gets clobbered and can corrupt an open buffer. All multiway turns happen in
  **isolated git worktrees** under a dedicated dir; the operator's cwd is
  read-only to the engine (source of the initial snapshot only).
- **Namespace and clean up.** Per-turn commits live on throwaway refs under
  `refs/nit-multiway/<mission>/…` and worktrees under
  `<state_dir>/multiway/worktrees/<mission>/…`. Never commit to or create
  branches on the operator's branch. Remove worktrees and prune refs when a
  mission ends or aborts (mirror the Workflow `isolation: worktree` cleanup).
- **Bounded.** Honour a token/turn budget and a node cap; respect the fd
  ceiling (`compute_effective_max_swarm_size` in `swarm/limits.rs`) — each live
  worktree+turn costs fds, so the frontier width must be clamped by the
  effective swarm cap.
- **Commits use a fixed engine identity** (e.g. `nit-multiway
  <noreply@nit.tools>`) passed explicitly via `git -c user.name=… -c
  user.email=…`, never the operator's identity, and never written to global
  git config.

## Recommended architecture (acceptance criteria are the contract; this is guidance)

- **New crate `nit-multiway`** — pure engine, no process spawning, no TUI.
  - Types: `NodeId`, `Node { commit, value: Option<Value>, status, parents }`,
    `Edge { from, to, kind: Turn|Fork|Merge|Backtrack }`, `Graph`, `Frontier`,
    `SearchPolicy { mood, k, budget }`, `Value { gated: bool, score: f32, tier }`.
  - Traits (so the engine is testable with fakes; real impls live in nit-tui):
    - `WorldStore`: `snapshot(&Path) -> NodeId`, `fork(&NodeId, k) -> Vec<Worktree>`,
      `commit(Worktree) -> NodeId`, `merge(&NodeId,&NodeId) -> Result<NodeId, Conflicts>`,
      `restore(&NodeId) -> Worktree` (for backtrack).
    - `TurnExecutor`: `run_turn(Worktree, task) -> TurnResult` (a fake returns
      scripted edits; the real one drives the runners).
    - `Valuer`: `value(&NodeId) -> Value` (real impl = genome + gates).
    - `MergeOracle`: `adjudicate(a, b, conflicts) -> ResolvedState` (real impl = judge turn).
  - The search loop: best-first expand by `Value.score` among gated-open nodes;
    fork `k`; prune gate-fails and dominated nodes; backtrack = the frontier
    re-selecting a held node when children regress; stop on budget/solution.
- **Integration in `nit-tui`** — a `MultiwayRuntime` implementing the traits:
  - `GitWorldStore` over `git` worktrees/commits (the safety rules above).
  - `RunnerExecutor` driving `claude_runner` / `codex_runner` (reuse `RunTurn`).
  - `GenomeValuer` = `nit_core::compute_genome_report` + the existing gate
    bundles (`rust-ci`/`node-ci`/`python-ci`/`go-ci` in `swarm.rs`).
  - `JudgeMergeOracle` reusing `shadow.rs` judge-prompt machinery.
  - A `@multiway <task>` chat command (parse in `app/chat_input.rs`) and the
    `NIT_MULTIWAY` flag; persist the DAG via `nit_utils::fs::write_atomic` to
    `<state_dir>/multiway/<mission>.json` (serde, like the swarm artifact tree).
- **Per-branch substrate:** `SubstrateState` is `Serialize`/`Deserialize` —
  snapshot it onto each `Node` so branches hold divergent claims/signals and
  merges reconcile them. (Phase 4; v1 may keep it global.)

## Phases

### Phase 0 — Engine core + `GitWorldStore`, no agents
**Goal.** Stand up `nit-multiway` with the data model, traits, the search-loop
skeleton, and a real `GitWorldStore`, exercised by a **fake `TurnExecutor`**
(deterministic scripted mutators) and a **fake `Valuer`**.
**Acceptance.** `cargo test -p nit-multiway` green on a scratch repo in a temp
dir: fork a node into *k* worktrees, commit a scripted edit in each, merge two
non-conflicting nodes, restore an older node, and prove the operator's primary
tree is never touched. `just ci` green. No new third-party deps.

### Phase 1 — Value function (genome + gates)
**Goal.** Real `Valuer`: `Value` from `compute_genome_report` (tier +
per-encoder generations + consistency) combined with hard gates (build/test).
Gate-failing nodes are marked non-viable and excluded from expansion.
**Acceptance.** Unit test (fake executor) over fixture files where the
known-better edit scores higher and a known-broken edit is gated out.
`cargo test -p nit-multiway` + `-p nit-tui` green.

### Phase 2 — Best-first frontier + backtrack + budget — shipped
**Goal.** Frontier ordered by value, expand top, fork *k*, prune,
**backtrack** to a held node when the current branch's children regress, stop
on budget/solution. `SearchPolicy` parameterised by `mood`
(explore = wider/keep-more, exploit = greedy).
**Acceptance.** A **deterministic trap test** (fake executor) where greedy
single-path search gets stuck at a local optimum but the multiway search
reaches the global solution at equal budget — assert exactly that gap. The
persisted DAG round-trips through serde. `just ci` green.
**Shipped.** `crates/nit-multiway/src/{search,policy,graph}.rs`:
`policy::frontier_admission(mood, viable)` is the mood knob (Exploit → 1,
Balanced → half, Explore → all viable children admitted; the remainder recorded
as `Held`), `policy::is_solution` early-stops on a non-gated `Replicator`-tier
node, and `Graph::{save,load}` persist the DAG via `nit_utils::fs::write_atomic`
+ `serde_json`. Backtrack is **emergent** — the frontier re-selects a `Held`
node through `pop_best` when better children run out; v1 emits no
`EdgeKind::Backtrack` and never marks a distinct lower-scoring sibling
`Dominated` (which could prune the only path to the global optimum). All score
comparisons use `f32::total_cmp`. The trap test and a populated-`Value`
persistence round-trip live in `crates/nit-multiway/tests/search.rs`.

### Phase 3 — Real agent executor + flag + `@multiway` — shipped
**Goal.** Drive the real runners as `TurnExecutor` in isolated worktrees; ship
`NIT_MULTIWAY` and `@multiway <task>`. Off-path (`NIT_MULTIWAY=0`) byte-identical.
**Acceptance.** End-to-end `@multiway` run on a tiny real task in a scratch
repo produces a DAG and a gated solution; `/abort` tears down worktrees/refs
cleanly; with the flag off, behaviour and code path are unchanged. `just ci`
green; full `cargo test --all` green. Env var documented in `CLAUDE.md` +
`docs/ENVIRONMENT.md`.
**Shipped.** `crates/nit-tui/src/multiway/executor.rs` — `RunnerExecutor`
drives a dedicated `ClaudeRunner` via `ClaudeCommand::RunTurn` with the worktree
as cwd (never the operator tree); `NoMergeOracle` is the Phase-4 placeholder.
`crates/nit-tui/src/multiway/runtime.rs` — `MultiwayRuntime` reads `NIT_MULTIWAY`
once in `from_env` and runs the search off the UI thread; `abort` / `abort_all`
/ `cleanup_finished` and normal completion all drop the `GitWorldStore`, so
worktrees and `refs/nit-multiway/<mission>/*` are always cleaned up.
`@multiway [mood=…] [k=N] <task>` parses in `crates/nit-tui/src/app/chat_input.rs`;
when `NIT_MULTIWAY` is on the chat submit hands the input to the run loop in
`app/runner.rs`, which owns the `MultiwayRuntime`, starts the search on a
dedicated `ClaudeRunner`, surfaces progress on the status line, and routes
`/abort` to its worktree/ref teardown; `k` is clamped by
`effective_max_swarm_size()`. v1 is **Claude-only**, **single-pane**, and
expands the *k* candidates **serially**; the DAG persists to
`<state_dir>/multiway/<mission>.json`. With `NIT_MULTIWAY=0`, `@multiway …`
dispatches as a literal chat prompt and no `MultiwayRuntime` is constructed —
byte-identical to today, the rollback pattern mirroring `NIT_CLAUDE_POOL=0`.

### Phase 4 — Adjudicated merge + per-branch substrate — shipped
**Goal.** Real `MergeOracle`: clean git merge where hunks don't conflict, a
**judge turn** (shadow machinery) to resolve where they do. Snapshot
`SubstrateState` per node and reconcile on merge.
**Acceptance.** Test: two branches making complementary changes merge into a
single gated, higher-value solution; a conflicting pair is resolved by the
judge (mock in unit tests, real in an integration test behind the flag).
`just ci` green.
**Shipped.** The engine gained an **opt-in** merge step that leaves the Phase 2/3
`run` path byte-identical: `Engine::run_merging` (merge ON) and the private
`run_inner`/`expand(merge)`/`try_merge` in `crates/nit-multiway/src/search.rs`,
with the trigger extracted to `policy::merge_candidates` (mood-gated — `Exploit`
never merges; `Balanced`/`Explore` join the top two viable siblings of one
expansion). A **clean** join goes through `GitWorldStore::merge`'s object-level
`merge-tree --write-tree` (no working tree touched); a **conflict** is restored as
base/`a`/`b`, adjudicated by the oracle, and recorded via `commit_merge` as a
two-parent node. Per-branch substrate is captured with the infallible
`SubstrateState::load(&tree)` in `commit_child` and reconciled at the join with
`SubstrateState::reconcile(a, b)` onto `Node::merged` (a-biased, parents in the
canonical `[a, b]` order; v1 mints no per-branch substrate, so the reconciled
snapshot is the default — the seam is wired for v2). A gate-failed **or**
conflict-marker merge is recorded `GateFailed` and withheld from the frontier,
never an `Err` (only operator-cancel/runner-gone aborts) — the dead-branch rule.
In `nit-tui`, `crates/nit-tui/src/multiway/executor.rs` adds the real
`JudgeMergeOracle` (drives one write-capable judge turn with `cwd = a`, reusing
`shadow::build_merge_judge_prompt`; `NoMergeOracle` stays as the rollback
placeholder) and a cloneable `SharedExecutor` wrapper, so
`crates/nit-tui/src/multiway/runtime.rs` shares **one** runner (one fd budget, one
turn-id `seq`) between the engine executor and the oracle and calls `run_merging`. The
genome valuer (`crates/nit-tui/src/multiway/valuer.rs`) force-gates any tree still
holding `<<<<<<<`/`>>>>>>>` bookends, so an unresolved judge can never pass as a
solution. **Acceptance met.** `crates/nit-multiway/tests/merge.rs` covers all four
outcomes — complementary clean merge into a higher-value two-parent node (and its
`Graph::save`/`load` round-trip), conflict adjudicated into a two-parent node,
unresolved markers gated without aborting, and `merge_supported() == false`
degrading to fork-only — asserting `parents.len() == 2` and the `EdgeKind::Merge`
edges throughout. `crates/nit-multiway/tests/substrate.rs` asserts the merge node
carries `reconcile(parent_a, parent_b)`, and
`crates/nit-tui/tests/multiway_runtime.rs` drives the production `JudgeMergeOracle`
over a scripted judge so a real adjudicated merge runs end-to-end offline. The
clean/conflict store mechanics are also covered in
`crates/nit-multiway/tests/git_store.rs` and the oracle's in-place contract in
`crates/nit-tui/tests/multiway_executor.rs`. No new env var (the feature stays
under `NIT_MULTIWAY`).

### Phase 5 — Measurement harness (the verdict) — shipped
**Goal.** Answer the question that justifies the whole thing: **does multiway +
backtracking beat the linear single-writer at equal token/turn budget?** A
harness (an example or `--multiway-bench` mode) runs a fixed task set both ways
at matched budget and emits metrics (success rate, final value/tier, turns,
wall-clock) as CSV/JSON — same spirit as `gol-novelty/`.
**Acceptance.** A reproducible comparison run with numbers. **A null result is
acceptable and informative** — if the line matches multiway at equal budget,
that is the finding, and we say so.
**Shipped.** `crates/nit-multiway/examples/multiway_bench.rs` (run with `cargo run
-p nit-multiway --example multiway_bench`) — a self-contained harness, like
`store_smoke.rs`:
an inline scratch repo plus the public engine API and `testing` doubles, no
external deps. It runs each scenario through a **LINEAR** arm (`mood = Exploit`,
`k = 1`) and a **MULTIWAY** arm (`mood = Explore`, `k ≥ 2`) under the **same
`Budget`** (shared `Copy` value — matched budget is structural, not eyeballed),
over a fixed scenario set that includes both the `tests/trap.rs` fixture (where
multiway wins) **and** a no-trap control where greedy already ties — so a
null/tie result surfaces honestly rather than being cherry-picked. It emits one
CSV row per `scenario × arm`
(`scenario,arm,reached_solution,final_tier,final_score,turns,nodes,stop_reason`)
plus a `winner ∈ {linear,multiway,tie}` per scenario, and dumps the rows as a JSON
array; `turns`/`nodes` are the matched-budget evidence columns. It is deterministic
(no clock/random — the engine forbids them) and never drops or re-runs a scenario
to manufacture a win. Both arms call `run` (merge OFF): Phase 5's headline is the
frontier+backtrack advantage, and merge on the scripted stack is degenerate — a
merge-on column is a noted follow-up. The CI-tested honesty harness
(`src/bench.rs` + `tests/bench.rs`) is deferred; the measurement example satisfies
the v1 verdict.

### Phase 6 — Multiway graph view ("how the agents thought") — shipped
**Goal.** Make the search legible: render the DAG so an operator can *see* the
agents reasoning — every branch tried, its value/tier, the gate-failures pruned,
the backtrack to a held node, and the kept path to the solution. The persisted
`<state_dir>/multiway/<mission>.json` already carries nodes (value, status,
parents) and typed edges (`Turn`/`Fork`/`Merge`/`Backtrack`); this phase turns
that record into a picture.
**Prerequisites (both tracks).** (1) Thread each turn's `summary`/`changed_paths`
onto its node so labels show *what the agent attempted*, not just a commit SHA.
(2) For live updates, give the pure engine a no-op-default **observer hook** —
e.g. `Engine::run_observed(.., &mut dyn FnMut(&Graph, &Frontier, &RunStats))`
fired after each `expand` — so progress streams without the engine depending on
nit-tui (`run` delegates to it with a no-op observer; existing tests are
unaffected). Today the runtime emits a single `MultiwayEvent::Progress` at the
end; the observer is what turns that into a live feed.
**Two tracks:**
- **6a — rendered graph in a window (the rich node-link picture, on demand).** A
  command (e.g. `@multiway-graph`, or a keybinding) renders the DAG to an image
  and opens it in the OS image viewer. Flow: the TUI shows a `rendering multiway
  graph…` **loading** state while a worker thread builds the image off the UI
  thread, then spawns the platform opener — `open` / `xdg-open` / `start`,
  directly, no shell — on the finished file. To make the *same* window show
  "loading" then the graph, write a placeholder image to the target path, open it
  immediately, then overwrite it with the final render (Preview and most viewers
  reload a changed file). Rendering: emit **Graphviz DOT** from the persisted
  `Graph` and spawn `dot -Tpng`/`-Tsvg` when graphviz is on PATH (matches the
  spawn-external-CLI convention, **no new Rust deps**); when `dot` is absent, save
  the `.dot`/`.svg` and surface the path. (A pure-Rust `layout`→SVG path avoids
  the external tool if we accept the dep.) This is an **on-demand snapshot**, not
  live — real time is 6b. The same DOT / `mwRun` mapping also feeds
  `nit-blogs/thesis/multiway/Results-01.nb` for a publication-quality render.
- **6b — live popup ("watch the agents think").** A toggleable **popup window**
  (a `widgets/` overlay alongside `artifacts_popup` / `gate_monitor_view`),
  opened by a keybinding — and auto-opened when a search starts; a `/`-command
  alias is fine — that renders the *whole* search live: every branch/turn, not
  just the in-flight one. Each node is a row in a depth-indented tree (v1's DAG is
  a tree; Phase-4 merges render as multi-parent refs), coloured by value
  (red→green) with a status glyph (frontier ●, held ⊙, expanded ○, gate-failed ✗,
  solution ★), its score/tier, and its turn summary. Highlight the current
  frontier, the best/kept path (gold), and the node whose turn is **in flight**
  (spinner); a header shows mood·k, turns·nodes vs budget, best score/tier, and
  the live stop reason. It redraws on each streamed `MultiwayView`, drained where
  `drive_multiway` already polls the channel, so it updates in real time as
  expansions land. Scroll/collapse for big trees; closing and reopening is
  non-destructive (it reads the live model, owns no state). Opt-in like the rest
  of the engine; the keybinding is documented in `docs/KEYBINDINGS.md`. (A true
  node-link picture stays track 6a — terminals draw trees, not graphs, well.)
**Acceptance.** 6a: the command shows a loading state, then opens an image of the
run's DAG in a viewer window; the graph matches the run (node count; the kept
path ends at the accepted solution) and degrades gracefully (saved `.dot`/path)
when graphviz is absent. 6b: the TUI tree updates live during an `@multiway` run,
with the in-flight turn and the kept path marked, and close/reopen is
non-destructive. The reference picture is `Results-01.nb` — the model we drew by
hand, now fed real data.
**Shipped.** Both tracks landed behind `NIT_MULTIWAY`. **Prerequisites:** each
turn's `summary` + `changed_paths` are threaded onto its `Node`
(`crates/nit-multiway/src/{node,search}.rs`, both `#[serde(default)]` so
pre-Phase-6 DAGs still load), and the pure engine gained a no-op-default
**observer hook** — `Engine::run_observed` / `run_merging_observed` firing a
`FnMut(&Graph, &Frontier, &RunSnapshot)` after each `expand`; `run` / `run_merging`
delegate through it with a no-op closure, so the Phase 2–5 tests and the bench stay
byte-identical (`crates/nit-multiway/tests/observer.rs`). The observer takes a
cheap-to-clone, read-only `RunSnapshot`, never `&mut RunStats`, so a UI closure
can't reach into the search accumulator. **6a** — the `@multiway-graph` chat command renders the most
recently persisted run's DAG: `Graph::to_dot()` (`graph.rs`, every label `dot_escape`d because node
summaries are untrusted agent text) feeds `crates/nit-tui/src/multiway/render.rs`,
which writes a `.dot`, spawns `dot -Tpng` and the platform opener
(`open`/`xdg-open`/`start`, no shell) on a worker thread behind a status-line
"rendering…" state, and degrades to surfacing the saved `.dot` path when graphviz is
absent (`RenderOutcome::{Image, DotOnly}`). **6b** — a live popup
(`crates/nit-tui/src/widgets/multiway_popup.rs`) renders the whole search as a
depth-indented tree: a value colour ramp, a status glyph per node
(frontier/held/expanded/gate-failed/solution), score·tier, the turn summary with its
changed-file count, the gold kept path, and the `frontier.peek_best()` "next" marker.
It reads a plain-data `nit_core::MultiwayView`
(`crates/nit-core/src/state/multiway_view.rs`) streamed as `MultiwayEvent::View` from
`build_multiway_view` (`crates/nit-tui/src/multiway/view_build.rs`) and drained where
`drive_multiway` already polls; the popup owns no state, so `Ctrl+Shift+M` (the
`is_multiway_popup_toggle_key` chord mirroring `Ctrl+Shift+T`, or the
`@multiway-popup` command; auto-opened on search start) closes and reopens
non-destructively. **Acceptance met:** `crates/nit-tui/tests/multiway_render.rs` covers the 6a render — a
well-formed DOT, the graphviz-absent → `DotOnly` degrade, and the platform opener
spawned on the finished image — and the 6b popup (proven by
`crates/nit-tui/src/widgets/tests/multiway_popup.rs`) updates live during a run, now
painted in `crates/nit-tui/src/app/draw.rs` with the in-flight and kept-path nodes
marked. No new env var — the feature stays under `NIT_MULTIWAY`.

### Phase 7 — `mode=multiway` across the dispatch commands — shipped
**Goal.** Multiway is an execution *backend*, not just a command. Let the existing
front-ends route into it with a uniform `mode=multiway` modifier, so the search is
reachable from `@swarm` / `@shadow` / `@all` (and bare chat) — **without changing
any of their default behavior**. `@multiway …` stays the explicit canonical form;
`mode=multiway` on another command is the same engine reached through that
command's ergonomics.
**Mechanism.** Each command parser detects an optional `mode=multiway` token. The
dispatch router — generalising today's `if multiway_enabled && parse_multiway_command`
gate (`app/chat_input.rs`) — routes to `MultiwayRuntime` **only** when
`multiway_enabled && mode == multiway`; otherwise the command's existing parse and
dispatch run **unchanged and byte-identical**. The engine is untouched: it already
takes a `Task` + `SearchPolicy`, so this phase is front-end parsing plus a
per-command param→policy mapping.
**Per-command mapping (params → `SearchPolicy` / `Task`):**
- `@shadow mode=multiway <task>` — the natural generalisation: a k=2 (or N) search
  whose **merge oracle reuses the shadow judge** (Phase 4); propose-a/-b are the
  first fork. The richest fit.
- `@swarm [N] [template=lab|parallel|bulk] [mission=…] mode=multiway <task>` —
  `N`/`template` map to `k`/`mood` (parallel/bulk → Explore + wider `k`, lab →
  Balanced), `mission=` → the search mission id, `<task>` → the search task. v1
  treats the prompt as the task (no planner DAG); planner-feeds-search is a later
  option.
- `@all mode=multiway <task>` — fan-out → one expansion with `k` = the fan-out
  count, value-ranked.
**Non-negotiables.** Off-path stays byte-identical: no `mode=multiway` (or flag off)
→ the command parses and dispatches exactly as today (the FROZEN rule from
Non-negotiables #1). Each command keeps its **default** behavior; the modifier is
strictly additive. Document it in `CLAUDE.md`'s Agent-commands section.
**Acceptance.** `@shadow mode=multiway` / `@swarm mode=multiway` / `@all
mode=multiway` each launch a multiway search with the mapped policy and appear in
the 6b live view; the same commands **without** the token (and any command with the
flag off) are byte-identical to today — a routing test asserts the off-path is
untouched. `just ci` green.
**Shipped.** `mode=multiway` is a uniform modifier on `@shadow` / `@swarm` / `@all`
(and bare chat), reaching the Phase 3/4 engine through each command's ergonomics with
**no change to default behaviour**. The router generalises the Phase 3
`if multiway_enabled && parse_multiway_command` gate in
`crates/nit-tui/src/app/chat_input.rs`: when — and only when — `multiway_enabled`, it
checks `@multiway-graph` (6a), then the explicit `@multiway` form, then strips a single
`mode=multiway` token (`dispatch::extract_mode_multiway`) and `classify`es the cleaned
body to a `MultiwaySource`. Detection lives **inside** the `multiway_enabled` guard, so
with the flag off (or no token) `raw` flows untouched to the existing parsers and
`@swarm mode=multiway foo` keeps `mode=multiway` as literal prompt text — byte-identical
to today (the FROZEN rule; asserted by `crates/nit-tui/src/app/tests/multiway.rs`). The
pending search rides as a typed `PendingMultiway { source, command }` of nit-core
primitives (the cleaned command re-parses losslessly, preserving `template`/`mission`),
and the runner maps it to a `(SearchPolicy, Task)` keyed by `source`:
`shadow_multiway_policy` (`shadow.rs`, `k=2` · `Balanced`, the shadow judge is already
the merge oracle), `swarm_multiway_policy` (`crates/nit-tui/src/swarm/multiway.rs`,
`SwarmSize` → `k`, `parallel`/`bulk` → `Explore`, `lab` → `Balanced`), and
`all_multiway_policy` (`crates/nit-tui/src/multiway/dispatch.rs`, `k` = fan-out ·
`Explore`, bounded to one expansion). Every mapper's `k` flows through
`MultiwayRuntime::start`'s `clamp_fork_width`, so the FD ceiling is never exceeded, and
the launched search shows in the 6b live view. **Acceptance met:** the routing test
proves the off-path (flag off, and flag on without the token) is byte-identical across
all front-ends, and each mapper's parameters are unit-tested
(`crates/nit-tui/tests/multiway_dispatch.rs` asserts the `@all` fan-out → clamped `k`).
No new env var — Phase 7 stays under `NIT_MULTIWAY`.

### Phase 8 — Real-repo gate environments
**Problem (found live-testing soapbox-mcp).** The value-gate runs the detected
bundle (`python-ci` → `python -m ruff/mypy/pytest`) **per node in a fresh
worktree with no installed deps**, so every node gate-fails — and worse, bare
`python` is a shell alias, not a binary, so `Command::new("python")` ENOENTs and
the *whole search aborts* on the first valuation. Rust/Go gates survive a clean
checkout (registry/module cache); Python/Node don't. The gate is also
un-overridable.
**Fix — three parts:**
1. **`NIT_MULTIWAY_GATES` override (the unblock).** When set, it *replaces* the
   auto-detected gate commands for the multiway valuer: a `;`-separated list of
   commands, each argv-split and spawned in the worktree (no shell, as today),
   all-must-pass. Empty (`NIT_MULTIWAY_GATES=`) → **no gates** (genome-only
   valuation — the operator's explicit opt-in to fail-open). Unset → auto-detect,
   unchanged. Resolved once at runtime construction and passed to
   `GenomeValuer::new(commands)` instead of `for_tree`. This lets an operator
   point the gate at a runner that resolves deps in a clean tree — e.g.
   soapbox-mcp's `uv`: `NIT_MULTIWAY_GATES="uv run pytest -q"` (uv syncs from
   `uv.lock` + cache per worktree).
2. **Graceful spawn-failure (robustness).** In `GenomeValuer`/`run_gate`, a gate
   whose *program* can't be spawned (ENOENT) must be a **gate failure**
   (`Ok(false)`, node gated), logged once — never the fatal `Err` that today
   aborts the entire search. A missing tool fails its node conservatively; the
   search still completes and stops cleanly.
3. **`python` → `python3`** in `GateBundle::Python` (`swarm/types.rs`). Bare
   `python` is absent on macOS and many hosts. Shared with the swarm — `python3`
   is strictly safer there too; keep the swarm gate tests green.
**Operator feedback.** When a run yields no viable node (all children gated),
the status/result says so and points at the override: *"all candidates
gate-failed; set `NIT_MULTIWAY_GATES` to a command that runs in a clean worktree
(e.g. `uv run pytest -q`), or `NIT_MULTIWAY_GATES=` for genome-only."*
**Acceptance.** `NIT_MULTIWAY_GATES="<cmds>"` runs exactly those per node
(all-must-pass); `=` empty → no gate spawned (genome-only); unset → auto-detect
byte-identical. A unit test injects a non-existent gate program and asserts the
node is gated and the search *completes* (no panic, no abort). The Python bundle
uses `python3` and the swarm gate tests stay green. On a `uv`-based fixture (or
soapbox-mcp), `NIT_MULTIWAY_GATES="uv run pytest -q"` produces viable nodes and a
real fork→merge. New env var documented in `CLAUDE.md` + `docs/ENVIRONMENT.md`;
`just ci` green.

### Phase 9 — Roster controls: Mood / Mode selectors + popup / graph buttons
**Goal.** Surface the multiway controls in the agent roster (`agent_ops_view.rs`)
next to the existing clickable `Template:` / `Mission:` selector rows
(`ROSTER_SWARM_TEMPLATE_LINE` / `ROSTER_SWARM_MISSION_LINE`), so an operator can
pick the search mood/mode and trigger the 6a/6b views by clicking, not only by
typing. Everything here is gated behind `multiway_enabled` (the flag) — with the
flag off, no new row or button renders and the roster is byte-identical.
**Selector rows** (same render + hit-test style as Template/Mission):
- `Mood:   explore   balanced   exploit` — the multiway **search** mood
  (`nit_multiway::policy::Mood`), *not* the substrate mood. Sets a new
  `AgentsState.multiway_default_mood: Mood` (default `Balanced`).
- `Mode:   linear   multiway` — sets `AgentsState.multiway_default_mode_on: bool`
  (default `false`). When `multiway` is selected, a dispatch with no explicit
  `mode=` token routes through the engine using the selected Mood; an explicit
  typed `mode=multiway` / `mood=` always wins over the selector. Routing stays
  flag-gated, so the Phase-7 off-path (flag off) is untouched.
**Buttons** (clickable controls in the roster header/toolbar):
- `[ Live view ]` → toggles `show_multiway_popup` (the Phase-6b `@multiway-popup`).
- `[ Graph ]` → sets `pending_multiway_graph = true` (the Phase-6a
  `@multiway-graph` render+open); greyed/disabled when no persisted DAG exists
  for the active or last mission.
**Wiring.** Clicks route through the existing roster click hit-test in
`app/mouse.rs` + `agent_ops_view`, the same path Template/Mission selector clicks
use. The buttons set the *same* `AgentsState` intent fields the typed commands
set, so `app/runner.rs::drive_multiway` already consumes them — no new run-loop
wiring. The two new selector defaults are read in `chat_input` dispatch alongside
the Phase-7 `extract_mode_multiway` / `parse_multiway_command` logic.
**Acceptance.** With `NIT_MULTIWAY=1` the roster shows the Mood/Mode rows + the
two buttons; clicking a Mood/Mode option updates the default and a subsequent
bare `@swarm`/chat dispatch routes accordingly (a unit test asserts the selection
drives the resolved `SearchPolicy`); clicking *Live view* toggles the popup and
*Graph* renders+opens the DAG; a typed `mode=`/`mood=` token overrides the
selector. **With the flag off, none of the new UI renders and the roster is
byte-identical** (a snapshot/router test asserts it). `just ci` green; no new env
var (Phase 9 lives under `NIT_MULTIWAY`).

### Phase 10 — Live-test fixes: valid model, button align, lifecycle node colours
Found driving `@multiway` on a real repo: the run stalls at the root
(`frontier-empty`, `nodes 1`) because every agent turn fails *silently*.
1. **(BLOCKER) Valid `--model` resolution.** `multiway_model` (`app/runner.rs`)
   takes the first Claude lane's id and falls back to bare `"claude"` — but
   `claude -p --model claude` is rejected ("issue with the selected model
   (claude)"), so every turn errors → no committed child → empty search. (The
   headless `multiway_live` only worked because it passed `--model sonnet`.)
   Resolve a CLI-valid model the way normal Claude turns do
   (`claude_model_slug_for_agent_id` on a real lane), never bare `"claude"`; if
   the roster has no usable Claude lane, refuse to start with a clear message
   rather than launching a doomed search.
2. **Surface turn failures (no silent `frontier-empty`).** When an expansion
   yields no viable child because turns *failed* (not gated), the popup/status
   must say so — e.g. "2/2 turns failed: <reason>" — instead of a bare
   `frontier-empty` that looks identical to "no good edits."
3. **Right-align the action buttons.** `[ Live view ]` / `[ Graph ]` render
   left-aligned; align them flush-right in the roster. Thread the roster width
   into BOTH `roster_multiway_buttons_line` (prepend the pad) AND
   `roster_multiway_button_hit` (offset `col` by the same pad, from the same
   width) so the click hit-test stays in sync; update `multiway_roster_click.rs`
   for the new `(col, width)` signature.
4. **Lifecycle node colours in the 6b popup** (`widgets/multiway_popup.rs`).
   Render every node (drop/relax the deep-`COLLAPSE_DEPTH` hiding so "show all
   nodes" holds), and colour by **lifecycle**, not only value: **active**
   (in-flight, being expanded) one bright highlight; **pending**
   (frontier-eligible: `Open`/`Held`) a second colour; **visited** (`Expanded`)
   dimmed; keep `GateFailed` (✗) and `Solution` (★/gold) distinct. The in-flight
   spinner stays.
5. **Surface the search in the chat pane.** Today `runner::drive_multiway` only
   sets `state.status` (status line) + the popup — no roster lane, no chat
   message — so the chat pane looks empty even on a healthy run, unlike the
   swarm (which posts a `mission_id` breather + lanes + messages). Keep the
   private executor `ClaudeRunner` for *execution* (the resource-isolation the
   design wants), but emit *display* events to the chat: a mission breather row
   keyed by the search's mission id showing live progress (turns/nodes/best),
   plus per-turn activity messages (fork started → edited / NoOp / failed-with-
   reason → merge → solution). The operator should see the agents working in the
   chat pane, not only in the popup.
**Acceptance.** `@multiway` on a real repo with a normal roster launches turns
that actually edit (a fork appears) with no model error; a genuinely failing
turn shows its reason in the popup; the buttons sit flush-right and still click
correctly (test asserts render↔hit-test agree); popup nodes are coloured
active/pending/visited and all render; the chat pane shows the multiway mission
breather + per-turn activity (not only the popup). `just ci` green; off-path
(flag off) unchanged.

### Phase 11 — Codex runner support (multi-backend executor)
**Goal.** v1 multiway is Claude-only (`RunnerExecutor` wraps `ClaudeRunner`).
Let an operator run a search with **Codex** too, chosen from the roster backend.
The runners are already parallel: `CodexRunner::spawn`, `CodexCommand::RunTurn {
model, cwd, mission_id, resume_thread_id, persist_session, reasoning_effort,
prompt, read_only }`, `pub events: Receiver<AgentBusEvent>`, emits
`FileWrite`/`TurnCompleted`/`TurnFailed`, plus `CancelAll`/`CancelTurn` — a
near-exact mirror of the Claude side the executor already drives.
**Design:**
1. **Abstract the backend.** Factor the turn loop (`dispatch` → `await_terminal`
   → `classify_event`) out of `RunnerExecutor` into a small `MultiwayBackend`
   trait — `dispatch(turn_id, cwd, prompt) -> bool`, `events() ->
   &Receiver<AgentBusEvent>`, `cancel_all()` — with `ClaudeBackend`
   (`ClaudeCommand`) and `CodexBackend` (`CodexCommand`; `reasoning_effort` maps
   to `effort`) impls. One executor over the trait; the
   FileWrite/Completed/Failed classification is shared (both runners emit those).
2. **Backend selection.** The runtime picks the backend from the operator's
   chosen roster backend (`AgentLaneKind::Claude` vs `Codex`;
   `roster_selected_backend` / the chosen lane) and resolves a **CLI-valid model
   for that backend** — generalising the Phase-10 Claude model fix to be
   backend-aware (never a bare/invalid slug). Refuse to start if the chosen
   backend has no usable lane.
3. **One engine, either backend.** An `AnyExecutor` enum (`Claude(..)` |
   `Codex(..)`) implements `TurnExecutor` by delegation, so the monomorphic
   `Engine<.., AnyExecutor, ..>` runs whichever backend was selected.
4. **Merge oracle follows the backend.** The Phase-4 `JudgeMergeOracle` runs a
   judge turn — drive it on the *selected* backend's runner, not a hard-wired
   Claude one.
**Scope.** Claude + Codex (the two real runners); the `MultiwayBackend` trait
leaves room for Gemini later. **Depends on Phase 10's model fix** (this
generalises it), so land 10 first.
**Acceptance.** With a Codex backend selected, `@multiway` (and `mode=multiway`)
launches Codex turns that fork / value / merge a DAG exactly as Claude does; a
backend-selection unit test maps `AgentLaneKind` → executor; the shared
classification is covered for both runners (a mock Codex emitting the three
events). `just ci` green; off-path (flag off) unchanged.

## Definition of done
- `NIT_MULTIWAY=1` runs a mission as a multiway search end-to-end; `=0` is
  byte-identical to today.
- The operator's working tree is never mutated by the engine; all work is in
  cleaned-up isolated worktrees.
- `just ci` green; new env vars documented in both places; `Cargo.lock` change
  limited to the one new crate.
- Phase 5 produces an honest multiway-vs-linear measurement.

## Non-goals (v1)
- Replacing or refactoring the linear swarm, shadow, or substrate semantics for
  existing paths.
- TUI visualisation beyond a minimal status line (a richer DAG view is later;
  the WL notebooks already visualise the model).
- Distributed / multi-machine execution.

## Open questions (resolve with the operator during the build)
- Frontier width *k* and budget defaults vs. the fd ceiling.
- Value scalar: how to combine tier, consistency, and gate margin into one
  comparable score (and whether to keep gates strictly hard).
- When to *trigger* a merge vs. keep branches separate (search-driven vs. task-driven joins).
- Whether per-branch substrate is worth its complexity in v1 or defers to v2.
