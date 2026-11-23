# Compare candidate changes in Git worktrees

Multiway is an experimental, single-pane Claude workflow. Use it to try alternative changes to the same task without applying them to your checked-out files. Each candidate runs in a separate Git worktree. Nit checks it with your gates, ranks the surviving candidates and saves a graph of the search.

## Enable and run

Open a Git workspace with `NIT_MULTIWAY=1 nit --agents claude .`, then enter a request in Agent Chat. For example, `@multiway k=2 mood=balanced Fix the parser offset for truncated input and exercise the regression test.`

```text
@multiway [mood=explore|balanced|exploit] [k=N] <task>
```

| Option | Meaning | Default |
|---|---|---|
| `mood` | How wide the search stays. `explore` keeps every viable branch, `exploit` follows the current best, `balanced` sits between. | `balanced` |
| `k` | How many candidates each expansion forks. Clamped by the effective swarm size. | `3` |

The default search budget is 64 candidate nodes and 32 turns. The broadcast form below uses one expansion.

- Each turn runs in its own git worktree under
  `<state_dir>/multiway/worktrees/<mission>/…`. nit never edits your working
  tree.
- The search DAG is saved to `<state_dir>/multiway/<mission>.json`. Progress
  shows on the status line.
- `/abort` and `/abort all` stop the search, remove the worktrees, and prune
  `refs/nit-multiway/<mission>/*`.
- With the flag off, `@multiway …` is sent as an ordinary chat prompt.

## Reaching it from other commands

Add the `mode=multiway` token to `@shadow`, `@swarm`, `@all`, or an ordinary chat request to start a candidate search.

- `@shadow mode=multiway <task>`: a `k=2`, `balanced` search. The judge evaluates merges.
- `@swarm [N] [template=lab|parallel|bulk] [mission=…] mode=multiway <task>`:
  `N` maps to `k`. `parallel` and `bulk` map to `explore`, `lab` to
  `balanced`. The mission type selects the task role. nit assigns the run its own archive ID.
- `@all mode=multiway <task>`: one expansion with `k` equal to the fan-out
  count, value-ranked.
- A bare chat prompt with the token runs with the default mood and `k`, and the
  prompt is the task.

Without the token, or with the flag off, every command behaves as normal and
`mode=multiway` stays literal prompt text.

## Watching a search

During a run, inspect the candidates and the budget in either view.

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

## Choose the checks that candidates must pass

Each candidate receives a structural score from the genome report. Its inputs include simulation generations for different source encodings and their agreement. A failed gate excludes a candidate from further expansion. A high score ranks the candidate within this search. Check behavior, maintainability and performance independently before adopting a change. [Code encoders and scores](SEEDS.md)

By default nit detects a gate bundle per worktree: `rust-ci`, `node-ci`,
`python-ci`, or `go-ci`. These are the same bundles used by [swarm verification](SWARM.md#verification-gates).
`NIT_MULTIWAY_GATES` overrides them:

| Value | Effect |
|---|---|
| Unset | Auto-detect the bundle. |
| `;`-separated commands | Those commands replace the bundle. Each is split into argv and run in the worktree with no shell. All must pass. |
| Empty (`NIT_MULTIWAY_GATES=`) | Skip configured build and test commands. Rank using the structural score. |

A gate program that cannot be started fails that node instead of aborting the
search. When every candidate fails its gates, the result says so and points at
the override. For a `uv` project that means
`NIT_MULTIWAY_GATES="uv run pytest -q"`. Nit also checks scanned source files for matching opening and closing Git conflict markers.

## How candidates are selected

After each expansion, `exploit` follows the highest-ranked viable candidate, `balanced` retains about half, and `explore` retains all viable candidates. If preferred candidates run out, the scheduler can return to a candidate it held in reserve.

Balanced and explore can merge the top two viable candidates from one expansion. A clean merge uses `git merge-tree --write-tree`, available with Git 2.38 or newer. If there is a conflict, a Claude judge turn can resolve it in an isolated worktree. Nit checks the merged result before considering it for further work. Exploit follows a single branch.

Candidate edits and commits stay in temporary worktrees and refs, which Nit cleans up after a run or cancellation. The working checkout is not automatically updated with the top candidate. Early success means a candidate passed the configured gates and reached the Replicator score threshold.

## Interpret a result

Read the stop reason, the gate outcomes and the number of turns used. A “solution” met the configured gates and tier threshold. Passing a weak or empty gate set says little about correctness. The search tests candidates sequentially and can stop when it exhausts its budget without finding a viable change. The saved graph records what was tried. In the original repository, use `git show <commit>` with the candidate’s commit ID from that graph to read its patch. To keep a candidate, create a named branch at its commit while the object is still available. Git can eventually prune commits left without a reference.

To export a saved graph from a source checkout, replace the path with your mission's graph file.

```sh
cargo run --locked -p nit-multiway --example dag_to_dot -- path/to/mission.json > mission.dot
dot -Tpng mission.dot -o mission.png
```

For a reproducible demonstration of the scheduling policy, run `cargo run --locked -p nit-multiway --example multiway_bench`. The benchmark uses fixed synthetic search trees and a simulated executor. It compares branching policies under a shared budget. Its results do not measure the quality or latency of a live model on your repository.
