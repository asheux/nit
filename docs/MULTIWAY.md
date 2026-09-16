# Multiway Engine

Multiway is an experimental search mode for agent missions. A linear mission
has one agent editing one working tree. Multiway instead forks a task into
several candidate edits in isolated git worktrees and scores each result. It
keeps the best branches and backtracks when a branch stalls. It is off by
default and works with Claude only. This doc covers how to run a search, how
to watch it, how scoring and merging work, and where the code lives.

## Enable and run

Set `NIT_MULTIWAY=1` before starting nit (see `docs/ENVIRONMENT.md`). Then
dispatch a search from Agent Chat:

```text
@multiway [mood=explore|balanced|exploit] [k=N] <task>
```

| Option | Meaning | Default |
|---|---|---|
| `mood` | How wide the search stays. `explore` keeps every viable branch, `exploit` follows the current best, `balanced` sits between. | `balanced` |
| `k` | How many candidates each expansion forks. Clamped by the effective swarm size. | `3` |

Every run has a budget of 64 nodes and 32 turns.

- Each turn runs in its own git worktree under
  `<state_dir>/multiway/worktrees/<mission>/…`. nit never edits your working
  tree.
- The search DAG is saved to `<state_dir>/multiway/<mission>.json`. Progress
  shows on the status line.
- `/abort` and `/abort all` stop the search, remove the worktrees, and prune
  `refs/nit-multiway/<mission>/*`.
- With the flag off, `@multiway …` is sent as an ordinary chat prompt.

## Reaching it from other commands

Add the `mode=multiway` token to `@shadow`, `@swarm`, `@all`, or a bare chat
prompt to route that command into the engine:

- `@shadow mode=multiway <task>`: a `k=2`, `balanced` search. The shadow judge
  is the merge oracle.
- `@swarm [N] [template=lab|parallel|bulk] [mission=…] mode=multiway <task>`:
  `N` maps to `k`. `parallel` and `bulk` map to `explore`, `lab` to
  `balanced`. `mission=` is the search mission id. The prompt is the task.
- `@all mode=multiway <task>`: one expansion with `k` equal to the fan-out
  count, value-ranked.
- A bare chat prompt with the token runs with the default mood and `k`, and the
  prompt is the task.

Without the token, or with the flag off, every command behaves as normal and
`mode=multiway` stays literal prompt text.

## Watching a search

Both views need the flag on. Nothing renders when it is off.

- `@multiway-graph` renders the most recent saved DAG as Graphviz DOT, runs
  `dot -Tpng`, and opens the image with the OS viewer. The status line shows
  `multiway: rendering graph…` while it works. When graphviz is missing, nit
  saves the `.dot` file and shows its path.
- `Ctrl+Shift+M`, or `@multiway-popup`, toggles a live popup. It opens on its
  own when a search starts. Closing and reopening loses nothing.

The popup shows the whole search as a depth-indented tree, one node per row:

| Element | Meaning |
|---|---|
| Header | Mood and `k`, turns and nodes against the budget, best score and tier, and the stop reason. |
| Row colour | The node's value, red to green. |
| Status glyph | frontier `●`, held `⊙`, expanded `○`, gate-failed `✗`, solution `★`. |
| Score and tier | The node's value and genome tier. |
| Turn summary | What the agent tried, with its changed-file count. |
| Gold path | The best path kept so far. |
| Next marker | The node the engine will expand next. |

Nodes deeper than six levels collapse into one marker.

### Roster controls

With the flag on, the agent roster gains two selector rows and two buttons:

| Control | Effect |
|---|---|
| `Mood:` row (`explore`, `balanced`, `exploit`) | The default search mood. Starts at `balanced`. |
| `Mode:` row (`linear`, `multiway`) | With `multiway` picked, a dispatch with no `mode=` token routes through the engine using the selected mood. Starts at `linear`. |
| `[ Live view ]` | Toggles the live popup. |
| `[ Graph ]` | Renders and opens the DAG. Greyed while there is no DAG to render. |

A typed `mode=` or `mood=` token always overrides the roster selection.

## Gates and the value function

Each node's value comes from the genome report plus hard gates. The report
(`nit_core::compute_genome_report`) gives a tier, per-encoder generations, and
a consistency measure; see `docs/SEEDS.md`. A node that fails a gate is marked
non-viable and never expanded.

By default nit detects a gate bundle per worktree: `rust-ci`, `node-ci`,
`python-ci`, or `go-ci`. These are the swarm gate bundles from `docs/SWARM.md`.
`NIT_MULTIWAY_GATES` overrides them:

| Value | Effect |
|---|---|
| Unset | Auto-detect the bundle. |
| `;`-separated commands | Those commands replace the bundle. Each is split into argv and run in the worktree with no shell. All must pass. |
| Empty (`NIT_MULTIWAY_GATES=`) | No gates. Genome-only valuation. |

A gate program that cannot be started fails that node instead of aborting the
search. When every candidate fails its gates, the result says so and points at
the override. For a `uv` project that means
`NIT_MULTIWAY_GATES="uv run pytest -q"`. The empty form gives genome-only. A
tree that still holds `<<<<<<<` / `>>>>>>>` conflict markers always fails its
gate.

## How the search works

The engine builds on five primitives:

1. State: a git commit that also carries a substrate snapshot.
2. Turn: one agent turn in a worktree, committed as a new state.
3. Fork: one state becomes `k` candidate successor states.
4. Value: the genome report plus gates, which ranks states.
5. Merge: an adjudicated join of two states.

Frontier admission depends on mood. After an expansion, `exploit` keeps one
child, `balanced` keeps half, and `explore` keeps every viable child. The rest
are held. Backtrack is emergent: when the better children run out, the frontier
re-selects a held node. The search stops early on a node that passes its gates
and reaches Replicator tier, or when the budget runs out.

Merge is mood-gated. `exploit` never merges. `balanced` and `explore` try to
join the top two viable siblings of one expansion:

- A clean merge uses `git merge-tree --write-tree`, which touches no working
  tree. This needs git 2.38 or newer.
- A conflict is restored as base, `a`, and `b`. One judge turn resolves it
  into a two-parent node.
- A merge that fails its gates, or still holds conflict markers, is marked
  `GateFailed` and kept out of the frontier. The search continues.
- Each branch's substrate snapshot is reconciled on merge, biased to `a`.

## Safety rules

- The engine never edits, checks out, or resets your working tree. It reads
  the tree once for the initial snapshot.
- All work happens in isolated worktrees on throwaway refs. Both are removed
  when the search finishes or aborts.
- Every run is bounded by its budget. `k` is clamped by the effective swarm
  size, which respects the file-descriptor ceiling.
- Commits use a fixed engine identity, `nit-multiway <noreply@nit.tools>`,
  passed with `git -c user.name=… -c user.email=…`. Your git identity and
  global config are never used or changed.

## Architecture

The pure engine lives in the `nit-multiway` crate. It spawns no processes and
has no TUI code, so tests drive it with fakes.

| Item | Names |
|---|---|
| Types | `NodeId`; `Node` (commit, value, status, parents, substrate, summary, changed paths); `Edge` with kinds `Turn`, `Fork`, `Merge`, `Backtrack`; `Graph`; `Frontier` |
| Node status | `Open`, `Expanded`, `GateFailed`, `Dominated`, `Held`, `Solution`. v1 never marks a node `Dominated`. |
| Policy | `SearchPolicy { mood, k, budget }`, `Budget { max_nodes, max_turns, max_tokens }`, `Value { gated, score, tier }` |
| Traits | `WorldStore` (snapshot, fork, commit, merge, commit_merge, restore), `TurnExecutor` (run_turn), `Valuer` (value), `MergeOracle` (adjudicate) |
| Entry points | `Engine::run` (merge off), `Engine::run_merging` (merge on), and `run_observed` / `run_merging_observed`, which call a closure with the graph, frontier, and a `RunSnapshot` after each expansion |

`nit-tui` supplies the real implementations:

| Piece | Role |
|---|---|
| `GitWorldStore` | Worktrees, commits, and merges over `git`, following the safety rules above. |
| `RunnerExecutor` | Runs each turn on a dedicated `ClaudeRunner` with the worktree as cwd. |
| `GenomeValuer` | Genome report plus gate commands. |
| `JudgeMergeOracle` | One write-capable judge turn on the conflicted tree, reusing the shadow judge prompt. |
| `MultiwayRuntime` | Reads `NIT_MULTIWAY` and `NIT_MULTIWAY_GATES` once in `from_env`, runs the search off the UI thread, and owns cleanup. |

### Key files

| Path | Contents |
|---|---|
| `crates/nit-multiway/src/{node,edge,graph,frontier,policy,value}.rs` | Data model, frontier, and the policy knobs `frontier_admission`, `is_solution`, `merge_candidates` |
| `crates/nit-multiway/src/search.rs` | `Engine`, the search loop, the merge step, the observer hook |
| `crates/nit-multiway/src/traits.rs` | The four traits |
| `crates/nit-multiway/src/git_store.rs` | `GitWorldStore` |
| `crates/nit-multiway/src/testing.rs` | Fake executor, valuer, and oracle for tests and the bench |
| `crates/nit-multiway/tests/` | Engine tests, including the trap, merge, substrate, and observer tests |
| `crates/nit-tui/src/multiway/runtime.rs` | `MultiwayRuntime`, the gate override, the `k` clamp, `ALL_GATED_HINT` |
| `crates/nit-tui/src/multiway/executor.rs` | `RunnerExecutor`, `JudgeMergeOracle` |
| `crates/nit-tui/src/multiway/valuer.rs` | `GenomeValuer` |
| `crates/nit-tui/src/multiway/dispatch.rs` | `extract_mode_multiway`, `all_multiway_policy` |
| `crates/nit-tui/src/shadow.rs`, `crates/nit-tui/src/swarm/multiway.rs` | `shadow_multiway_policy`, `swarm_multiway_policy` |
| `crates/nit-tui/src/multiway/render.rs` | DOT to PNG render and the OS opener |
| `crates/nit-tui/src/multiway/view_build.rs`, `crates/nit-core/src/state/multiway_view.rs` | The `MultiwayView` the popup reads |
| `crates/nit-tui/src/widgets/multiway_popup.rs` | The live popup |
| `crates/nit-tui/src/widgets/agent_ops_view.rs` | The roster Mood and Mode rows and buttons |
| `crates/nit-tui/src/app/chat_input.rs`, `crates/nit-tui/src/app/runner.rs` | Command parsing and the run loop |
| `crates/nit-tui/tests/multiway_*.rs` | Runtime, executor, valuer, render, dispatch, and view tests |

### Examples

| Command | What it does |
|---|---|
| `cargo run -p nit-multiway --example store_smoke -- [repo-path]` | Runs snapshot, fork, commit, merge, restore, and cleanup on a real repo. Proves your working tree is unchanged and the refs are gone. Defaults to the current directory. |
| `cargo run -p nit-multiway --example multiway_bench -- [out.json]` | The linear-vs-multiway benchmark. Prints CSV and, with a path, writes the rows as JSON. |
| `cargo run -p nit-multiway --example dag_to_dot -- <mission.json>` | Prints a saved DAG as Graphviz DOT. Pipe it to `dot -Tpng -o graph.png`. |

The bench runs a LINEAR arm (`exploit`, `k=1`) and a MULTIWAY arm (`explore`,
`k=2`) at the same budget. The scenario set is fixed: `trap`, `smooth`, and
`starved_trap`. It emits one CSV row per scenario and arm with the columns
`scenario,arm,reached_solution,final_tier,final_score,turns,nodes,stop_reason,budget_turns,budget_nodes,wall_us,winner`.
The winner per scenario is `linear`, `multiway`, or `tie`, and a tie is
reported as a tie. Both arms run with merge off. `wall_us` is informational
only. The bench is deterministic and never spawns an agent.

## Contributor rules

- Everything is additive and opt-in behind `NIT_MULTIWAY`. The off path stays
  unchanged.
- Resolve the flag once when the runtime is built. A mid-mission env change
  must not flip behaviour.
- `just ci` must pass: `cargo fmt -- --check`, `cargo clippy --all-targets
  --all-features -- -D warnings`, `cargo test --all`, and `cargo deny`.
- Build with `--locked`. Do not add dependencies casually; prefer `std` and
  existing workspace crates. No network deps. Spawn `git` and `claude`
  directly, never through a shell.
- MSRV is 1.88.0.
- Document every env var in both `CLAUDE.md` and `docs/ENVIRONMENT.md`.

## Limitations

- Claude only.
- Single pane.
- The `k` candidates are expanded one at a time.
- No distributed or multi-machine execution.

## Planned

- Resolve a CLI-valid Claude model instead of falling back to bare `claude`.
- Show failed turns instead of a bare `frontier-empty`.
- Right-align the roster buttons.
- Lifecycle colours in the popup: active, pending, and visited nodes.
- Show multiway activity in the chat pane.
- A Codex backend.
