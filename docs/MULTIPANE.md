# Multipane Mode

> **Status**: design proposal. Not yet implemented. This doc is the
> authoritative plan + the prompt to feed coding agents.

## Vision

A second launch mode for nit that opens a grid of independent **chat
panes**, each operating in its own working directory, all backed by a
single user-chosen agent backend. Editor, agent ops, visualizer, and
the rest of the standard nit UI are unavailable in this mode — only
chat dispatch.

```
$ nit multipane --backend claude-haiku-4-5 --panes 8

┌── pane 0 ─────┬── pane 1 ─────┬── pane 2 ─────┬── pane 3 ─────┐
│ cwd: nit/     │ cwd: ../web/  │ cwd: ../infra │ cwd: ~/scripts│
│               │               │               │               │
│  agent chat   │  agent chat   │  agent chat   │  agent chat   │
│  (focused)    │               │               │               │
│               │               │               │               │
│ > ___         │ > ___         │ > ___         │ > ___         │
├── pane 4 ─────┼── pane 5 ─────┼── pane 6 ─────┼── pane 7 ─────┤
│ ...           │ ...           │ ...           │ ...           │
└───────────────┴───────────────┴───────────────┴───────────────┘
```

Use case: drive N concurrent agent sessions across different projects
from one terminal. Like `tmux` for AI agents.

## Hard requirements (from operator brief)

- **`--backend` is optional**. When supplied (specific lane id or family
  alias), the chosen backend applies to **every pane** — no per-pane
  backend override is allowed. When omitted, every pane opens in
  **roster mode** showing every available backend, and the operator
  picks per pane independently.
- Each pane is **its own session** (chat history, mission state, queued
  turns, in-flight work) anchored at its own **cwd**.
- Agent dispatches from a pane operate inside that pane's cwd —
  subprocess `Command::current_dir(pane.cwd)`.
- A **dir search** at the top of each pane:
  - Plain text → fuzzy-match subdirectories of `pane.cwd`.
  - `../<query>` → search siblings of `pane.cwd` (one level up).
  - `../../<query>` → search siblings of parent (two levels up).
  - … and so on for `../../../`.
  - Enter / select → switch the pane's cwd to the matched directory.
- **Search must be fast** — feels instant on directory trees of 10k+
  entries.
- Default panes per launch: **8** (4 wide × 2 tall) — fit into a normal
  terminal. Operator override: `--panes N`.
- Navigation: **Tab** / **Shift+Tab** to cycle, **mouse click** to
  focus directly.
- In multipane mode, **only the chat pane works**. No editor, no agent
  ops dock, no visualizer. To use those, exit multipane and launch
  nit normally.

## CLI surface

New subcommand on the existing `clap` enum:

```bash
nit multipane                                          # 8 panes, full roster per pane
nit multipane --panes 4                                # 4 panes, full roster per pane
nit multipane --backend claude                         # 8 panes, all locked to Claude family
nit multipane --backend gpt-5 --panes 6                # 3×2 grid, all pre-picked to gpt-5
nit multipane --backend claude-haiku-4-5 --panes 4     # 4 panes, all pre-picked
nit multipane --backend claude --panes 8 --cwd /work   # full composition
```

Note: the operator's shorthand `nit multipane --backend claude 5 --panes 8`
includes a stray positional `5` that clap rejects. The CLI surface is
strictly `--backend <id-or-family>` + `--panes <N>` + `--cwd <path>`.

`--panes` clamps to `[1, 32]`. The grid layout chooses dimensions to
keep panes roughly square (computed as `ceil(sqrt(N))` columns).

`--backend` accepts either a specific lane id (`claude-haiku-4-5`,
`gpt-5`, …) or one of four reserved family aliases — `codex`, `claude`,
`gemini`, `local` — case-insensitive. A specific lane id pre-picks every
pane (the lane is cloned into `state.agents.agents` as
`<id>#mp-pane-NN` and the pane lands directly in chat). A family alias
filters the per-pane roster but leaves panes unselected; the operator
picks within that family. Omitting `--backend` shows the full roster
with per-pane independence. Unknown specific ids exit with the available
ones listed.

## UX spec

### Per-pane layout

```
┌─[pane 0]─ cwd: /Users/me/code/nit ──────────────┐
│ search:  __                                     │  ← 1-row dir search
│ ─────────────────────────────────────────────── │
│ ↳ [haiku] done (see ARTIFACTS)                  │
│   You: refactor crates/nit-utils/src/lib.rs     │  ← chat thread
│ ↳ [haiku] Working ...                           │     (existing
│                                                 │      agent_console_view)
│                                                 │
│ ─────────────────────────────────────────────── │
│ ↳ /abort · Ctrl+C · Esc Esc                     │  ← hint strip
│ ┌── CHAT BOX ────────────────────────────────── ┐
│ │                                               │  ← input
│ └────────────────────────────────────────────── ┘
└─────────────────────────────────────────────────┘
```

The middle and bottom (chat thread + hint + input box) are the
**existing** `agent_console_view::render` output, parameterised on a
per-pane state. Top row is new.

### Dir search modes

The dir search bar at the top accepts free text. The parser looks at
the prefix:

| Input | Meaning |
|---|---|
| `(empty)` | Show siblings of cwd (same as `../`) |
| `foo` | Fuzzy match subdirectories of cwd whose name contains "foo" |
| `../` | Show siblings of cwd |
| `../foo` | Fuzzy match siblings of cwd containing "foo" |
| `../../` | Show siblings of cwd's parent |
| `../../foo` | Fuzzy match siblings of cwd's parent containing "foo" |
| `/abs/path` | Treat as absolute, match descendants |
| `~/foo` | Expand `~` and search |

Results render below the search bar as a small dropdown (max 10 rows).
Up/Down to move, Enter to commit, Esc to cancel and resume the chat
thread underneath. Typing more characters narrows the list live.

When the user commits a directory, the pane:
1. Updates `pane.cwd`.
2. Pushes a system message to the pane: `cwd → /new/path`.
3. The next `@swarm`/free-text dispatch uses the new cwd.

### Focus & input routing

- One pane has focus at any time, drawn with a brighter border.
- Tab cycles forward, Shift+Tab cycles backward. Tab/Shift+Tab never
  move the per-pane roster cursor — they only switch which pane has
  focus.
- Mouse click anywhere inside a pane focuses it.
- Only the focused pane receives keyboard input. Background panes
  still update (turn output streams in, "Working..." breather animates,
  etc.).
- `/abort`, `/abort all`, `/abort <agent-id>`, Ctrl+C with empty input,
  and Esc-Esc within ~500 ms target the focused pane (or every pane,
  for `all`). Issuing them in a roster-mode pane (no committed
  selection) emits a one-line "no agent selected — nothing to abort"
  notice in the pane's chat history rather than a silent drop.

### Roster mode keymap

When a pane has no committed selection, the body of the pane renders the
same tree the Agent OPS Roster tab does:

```
 Template:  lab   parallel   bulk
 Mission:   auto   general   research   computational

 ▾ Codex
   ↳ gpt-5  [Codex]
       [x] medium
       [ ] high
 ▸ Claude
 ▸ Gemini
 ▸ Local
```

- The `Template:` and `Mission:` rows control swarm defaults
  (`state.agents.swarm_default_template` /
  `state.agents.swarm_default_mission`). Click a word to set it. These
  writes are global — clicking from one pane affects all panes and the
  Agent OPS dock.
- Backend headers expand / collapse per pane. Expansion is purely
  cursor-driven: at most one backend group is visible at a time
  (whichever the cursor is on, via `PaneSession.auto_expanded_backend`),
  and the group collapses the moment the cursor walks off it. Two
  panes can be on different backends because the cursor is per-pane.
- Each agent under an expanded backend shows a `↳ Size` branch with one
  `[x]` checkbox per supported reasoning effort. Toggling sets the
  effort in the global `codex_selected_reasoning_effort` /
  `claude_selected_effort` map (matching Agent OPS behaviour).

| Key | Action |
|---|---|
| `↑` / `k`, `↓` / `j` | Move the per-pane cursor through selectable rows (Backend, Agent, SizeBranch, SizeLeaf). Template / Mission rows are skipped — they are click-only. |
| `→` / `l` | Expand the focused row: opens a Backend group, un-collapses an Agent's tree, etc. |
| `←` / `h` | Collapse: closes a Backend group, hides an Agent's Size leaves. |
| `PgUp` / `PgDn` | Jump the cursor by a page (8 rows). |
| `g` / `G` | Jump cursor to the first / last selectable row. |
| `Space` | Toggle the checkbox under the cursor when it sits on a SizeLeaf. |
| `Enter` | Commit: Backend → toggle expand; Agent → materialise `<base>#mp-pane-NN` and switch the pane to chat mode; SizeBranch → toggle agent tree collapse; SizeLeaf → toggle the checkbox. |
| `Tab` / `Shift+Tab` | Cycle focus between panes (never moves the roster cursor). |
| `Mouse left-click` | Focuses the clicked pane and routes to the row under the cursor: Template/Mission word → set the global default; Backend → toggle expand; Agent → materialise + switch to chat; SizeBranch → toggle tree; SizeLeaf → toggle checkbox. |
| `Mouse wheel` | Scrolls the pane's roster viewport (`PaneSession.roster_scroll`). |
| `Ctrl+C`, `Esc Esc` | Emits the no-op "no agent selected — nothing to abort" notice. |

When a pane is already in chat mode:

| Key | Action |
|---|---|
| `Ctrl+R` | Revert the focused pane back to roster mode. Clears `selected_agent_id`, the chat input buffer, and the active mission. The original roster cursor / expansion state are preserved per pane. |
| `PgUp` / `PgDn` | Scroll the chat thread (`PaneSession.chat_thread_scroll`). The auto-stick-to-bottom default returns when the operator scrolls back to 0. |
| `Mouse wheel` | Scrolls the focused pane's chat thread. Wheel events on a different pane don't steal focus — the wheel always targets the pane under the cursor. |

The four reserved family aliases (`codex`, `claude`, `gemini`, `local`)
filter the roster to a single backend family. If the filter matches no
installed lanes (e.g. `--backend gemini` on a host without the Gemini
CLI), the pane shows a single "No <family> agents detected — install
the CLI" line instead of crashing or exiting.

### Disabled features

In multipane mode:
- No editor pane, no Notes pane, no file tree, no Agent Ops dock, no
  visualizer pane.
- Global keybindings that would have switched focus to those panes
  are no-ops (or absent).
- `@swarm` still works inside a pane — it routes to the per-pane agent
  the same way.
- `/abort`, Ctrl+C (empty), Esc-Esc still work in the focused pane.

## State model

### New core types (in `nit-core/src/state/multipane.rs`)

```rust
pub struct PaneSession {
    pub pane_id: usize,                       // 0..N-1; stable for the run
    pub agent_id: String,                     // empty until selection commits
    pub cwd: PathBuf,                         // working directory for this session
    pub chat_input: String,
    pub chat_input_cursor: usize,
    pub chat_input_selection_anchor: Option<usize>,
    pub chat_input_scroll: usize,
    pub chat_prompt_history: Vec<String>,
    pub chat_prompt_history_pos: Option<usize>,
    pub dir_search: Option<DirSearchState>,   // Some when search is active
    pub mission_id: Option<String>,           // current mission anchored to this pane
    pub roster_cursor: usize,                 // position inside the per-pane roster
    pub roster_scroll: usize,                 // viewport top row inside the roster body
    pub auto_expanded_backend: Option<AgentLaneKind>,     // cursor-driven, single-backend latch
    pub auto_expanded_agent: Option<String>,              // cursor-driven, mirrors the agent row
    pub roster_collapsed_agent_ids: HashSet<String>,      // pane-local Size/Role collapse
    pub roster_tree_selected: Option<RosterTreeSelection>, // leaf cursor inside Size branch
    pub chat_thread_scroll: usize,            // separate from chat_input_scroll
    pub selected_agent_id: Option<String>,    // None ⇒ render roster picker
}

pub struct DirSearchState {
    pub query: String,
    pub query_cursor: usize,
    pub results: Vec<PathBuf>,                // populated by dir_search_runner
    pub selected: usize,
    pub generation: u64,                      // request-id latch, runner drops stale results
    pub show_hidden: bool,                    // Alt+f toggle
    /// Index into the directory tree we're searching in. Computed from
    /// query prefix (`../` count) at parse time.
    pub base: PathBuf,
}

pub struct MultipaneState {
    pub backend_agent_id: String,             // operator's --backend verbatim (or "")
    pub panes: Vec<PaneSession>,
    pub focused: usize,                       // 0..panes.len()-1
    pub grid_cols: usize,                     // computed at launch
    pub grid_rows: usize,
    pub backend_filter: Option<String>,       // family alias / specific id / None
}
```

`AppState` gets one new optional field:

```rust
pub multipane: Option<MultipaneState>,
```

When `Some`, render the multipane UI; ignore standard panes/widgets.
When `None` (the default), nit behaves exactly as it does today —
zero impact on existing flows.

### Why this shape

- **Messages stay in `state.agents.messages`** keyed by `agent_id`, as
  today. Each pane has a unique agent_id (`<base>#pane-K`), so the
  message renderer naturally filters per-pane without a duplicate
  Vec.
- **Active turns + queues** stay in `state.agents.active_turns` /
  `queued_*_turns` keyed by agent_id. The runner already supports
  arbitrary agent_id values; the per-pane suffix is enough.
- **Mission + swarm state** stays in `SwarmRuntime`. A pane's swarm
  mission is just a regular swarm mission with the pane's agent_id as
  planner.
- **Per-pane state is small** (input buffer, history, search) — fits
  cleanly in `PaneSession` without restructuring the rest of state.

### Reusing the existing chat renderer

`crates/nit-tui/src/widgets/agent_console_view.rs::render` already
consumes `&AppState` and assumes it's drawing the *one* chat pane.
We extract a thinner core that takes a `&PaneSession` plus the
shared `state` for messages — call it `render_pane(...)`. The
existing `render` becomes a thin wrapper for backward compat
(non-multipane mode).

## CLI parsing

```rust
// crates/nit/src/cli/mod.rs
#[derive(Subcommand, Debug)]
pub enum Command {
    Gol { ... },
    Games { ... },
    Multipane(MultipaneArgs),
}

#[derive(Args, Debug)]
pub struct MultipaneArgs {
    /// Backend model id (specific lane like `claude-haiku-4-5`) or
    /// family alias (`codex` / `claude` / `gemini` / `local`). Optional —
    /// when omitted, every pane opens in roster mode.
    #[arg(long)]
    pub backend: Option<String>,

    /// Number of panes to open. Clamped to [1, 32]. Grid is roughly
    /// square: ceil(sqrt(N)) columns × ceil(N / cols) rows.
    #[arg(long, default_value_t = 8u8, value_parser = clap::value_parser!(u8).range(1..=32))]
    pub panes: u8,

    /// Starting directory for every pane. Defaults to the current
    /// working directory.
    #[arg(long)]
    pub cwd: Option<PathBuf>,
}
```

## Phased plan

### Phase 1 — CLI + skeleton state (no UI yet)

Files:
- `crates/nit/src/cli/mod.rs` — add `Command::Multipane`
- `crates/nit-core/src/state.rs` — `PaneSession`, `DirSearchState`,
  `MultipaneState`; add `pub multipane: Option<MultipaneState>` to
  `AppState`
- `crates/nit-core/src/lib.rs` — re-export new types
- `crates/nit/src/main.rs` — branch on `Command::Multipane`, validate
  backend, populate `state.multipane = Some(...)`

Acceptance:
- `nit multipane --backend claude-haiku-4-5` builds N pane sessions
  in state and exits without rendering (or renders a stub).
- Missing `--backend` produces a clean error.
- Unknown backend produces an error listing available choices.
- Tests: backend validation table, grid-shape computation table.

### Phase 2 — Grid layout + per-pane render

Files:
- `crates/nit-tui/src/app/runner.rs` — when
  `state.multipane.is_some()`, dispatch to a new
  `multipane::run_loop` instead of the standard one.
- `crates/nit-tui/src/multipane/mod.rs` (new) — main render +
  event loop for multipane mode.
- `crates/nit-tui/src/widgets/agent_console_view.rs` — split
  `render` so a thinner core can be called per-pane.

Acceptance:
- Empty 8-pane grid renders without errors.
- Each pane has its own border and cwd shown in the header.
- Tab cycles focus, focused pane has a brighter border.
- Mouse click focuses a pane.
- Tests: grid-shape for N=1, 2, 4, 6, 8, 16, 32.

### Phase 3 — Per-pane chat dispatch

Files:
- `crates/nit-tui/src/multipane/dispatch.rs` — wraps existing
  `app::dispatch::dispatch_agent_prompt` but injects the pane's cwd
  and agent_id.
- Possibly extend `CodexCommand::RunTurn` / `ClaudeCommand::RunTurn`
  with `cwd` (already there) — verify it's plumbed.

Acceptance:
- Type a prompt in pane 0, hit Enter, agent runs with the pane's
  cwd. Type a different prompt in pane 3, runs with pane 3's cwd.
- Both panes show their own thread independently.
- Existing `/abort`, Ctrl+C, Esc-Esc still work in the focused pane.
- Tests: dispatch test with mock runner, asserts cwd argument.

### Phase 4 — Dir search

Files:
- `crates/nit-tui/src/multipane/dir_search.rs` (new) — parser for
  the `../`-prefix grammar, fuzzy match algorithm, results.
- `crates/nit-tui/src/multipane/dir_search_runner.rs` (new) — async
  worker thread that walks directories and fuzzy-matches without
  blocking the UI.

Performance:
- Index strategy: lazy walk on first activation per `(base, depth)`,
  cache results in `state.multipane.dir_index_cache`.
- For each pane, cap walk depth at 4 unless the user explicitly
  navigates deeper.
- Use `std::fs::read_dir` + a small worker pool (rayon) for
  parallel directory enumeration.
- Fuzzy match algorithm: subsequence match with bonus for
  consecutive characters and word-boundary hits. Reuse logic from
  `fuzzy_search_runner.rs` if practical.

UX:
- A dedicated key (e.g. `Ctrl+/` or just typing in the top row)
  activates dir search mode in the focused pane.
- Esc cancels and returns to chat input.
- Enter on a result switches `pane.cwd` and pushes a system message.
- Up/Down navigate results.

Acceptance:
- Typing `foo` in dir search shows subdirectories of cwd containing
  "foo", ranked by fuzzy score.
- `../foo` ranks siblings of cwd, `../../foo` siblings of parent.
- Enter switches cwd; system message confirms.
- Search results update inside 16ms for 10k-entry trees.
- Tests: parser for prefix counting, fuzzy-rank ordering, cwd-switch
  state mutation.

### Phase 5 — Disable non-pane keys + polish

Files:
- `crates/nit-tui/src/multipane/key_dispatch.rs` (new) — gate the
  global key handler so unrelated keys (Editor focus, Visualizer,
  etc.) become no-ops.
- `crates/nit-tui/src/widgets/top_bar.rs` (or its multipane variant)
  — hide LAB / GOL / MOOD chrome in this mode.

Acceptance:
- Pressing keys that would normally focus the editor or
  visualizer in regular nit do nothing in multipane mode.
- Status bar reflects multipane mode.

### Phase 6 — Tests + docs

- Unit tests for: backend validation, grid math, dir-search parser,
  fuzzy ranking, per-pane dispatch, focus cycling, message
  filtering.
- Integration test: launch headless multipane, dispatch a prompt to
  each of 4 panes with mock runners, assert each saw its own cwd.
- Update `docs/KEYBINDINGS.md` with multipane-specific bindings.
- Update `CLAUDE.md` with the new subcommand and env vars (none new
  expected, but flag presence).

## Performance budget

| Op | Target | Notes |
|---|---|---|
| Initial render of 16-pane grid | < 50 ms | Same render code per pane; one ratatui frame |
| Dir search keystroke → result update | < 16 ms | One frame budget; runs on async worker |
| Walk a 10k-entry directory tree | < 100 ms | Parallelised via rayon, cached after first walk |
| Focus switch (Tab) | < 1 ms | Pure state mutation, no IO |
| Pane redraw on background turn output | < 16 ms | Reuses existing render path |

The chat-thread render path is already ratatui — fast. The dominant
cost is the dir walk, which is why we cache and amortise.

## Open questions (to resolve during implementation)

1. **Agent identity collisions**: if the user has `claude-haiku-4-5`
   in their roster already, our pane agent_id `claude-haiku-4-5#pane-0`
   collides with naming conventions for swarm/chat clones. Pick a
   different separator (e.g. `:pane:`) or namespace (`mp-pane-0`)?
2. **Persistence**: should `pane.cwd` and chat history persist across
   nit restarts? Probably yes — write to
   `<state_dir>/multipane/session-<hash>.json`. Nice-to-have, not
   blocking.
3. **Backend specificity**: `--backend claude-haiku-4-5` (specific
   model) vs `--backend claude` (family, pick default)? Start with
   specific; add family as a convenience later.
4. **Single mission across panes vs per-pane mission**: each pane is
   independent — one mission per pane, max. Cross-pane swarms are out
   of scope.
5. **Resize handling**: when the terminal resizes below the minimum
   per-pane size, show a single "terminal too small" message rather
   than rendering broken panes.

## Out of scope (v1)

- Splitting / closing panes at runtime. Layout is fixed at launch.
- Different backends per pane.
- Cross-pane operations (broadcast a prompt to all panes).
  *(But — this could be `@all-panes <prompt>`. Easy follow-up.)*
- Persistence of pane sessions across nit restarts.
- Mouse drag to resize pane boundaries.

## Coding-agent prompt

Copy-paste-ready brief for `@swarm` or a follow-up Claude/Codex run.
Reference this doc as the source of truth.

---

> **Goal**: Implement multipane mode for nit per `docs/MULTIPANE.md`.
>
> **Scope**: Phases 1-3 of the plan in one mission (CLI + skeleton
> state, grid layout + render, per-pane dispatch). Phases 4-6 are
> follow-up missions.
>
> **Constraints**:
> - Follow `docs/MULTIPANE.md` exactly. Deviations need a comment in
>   the doc explaining why.
> - **Do not modify** any code path that runs when `state.multipane`
>   is `None` — multipane mode must be additive.
> - **Single backend per launch**: validate `--backend` at startup;
>   reject unknown models with a list of valid ones.
> - All new code must pass `just clippy` (no warnings) and have unit
>   tests in the relevant `tests/` module.
> - When the integrator finishes, total `cargo test --all` must
>   match the previous count + new tests.
>
> **Deliverables**:
> 1. `Command::Multipane` in `crates/nit/src/cli/mod.rs` (Phase 1)
> 2. `PaneSession` / `DirSearchState` / `MultipaneState` types in
>    `crates/nit-core/src/state.rs` + re-exports (Phase 1)
> 3. `crates/nit-tui/src/multipane/mod.rs` with the multipane event
>    loop (Phase 2)
> 4. Per-pane render reusing `agent_console_view` core (Phase 2)
> 5. Per-pane dispatch with cwd injection (Phase 3)
> 6. `/abort`, Ctrl+C, Esc-Esc still work in the focused pane
>    (regression check, no new behaviour)
> 7. Updated `CLAUDE.md` and `docs/KEYBINDINGS.md` with the new
>    subcommand
> 8. Unit tests for: backend validation, grid math (N → cols × rows),
>    cwd injection, focus cycling
>
> **Out of scope for this mission**: dir search (Phase 4), feature
> gating polish (Phase 5), persistence. Leave TODOs and create
> follow-up work items.
>
> **Verification**:
> - `nit multipane --backend claude-haiku-4-5 --panes 4` launches 4 panes
>   already pre-picked to that lane and ready for chat.
> - `nit multipane --backend claude --panes 4` launches 4 panes whose
>   roster is filtered to the Claude family; operator picks per pane.
> - `nit multipane --panes 4` launches 4 panes showing the full roster;
>   each pane picks independently with `↑/↓` + `Enter`.
> - Tab cycles pane focus, mouse click focuses, focused pane has a
>   brighter border. Up/Down stays scoped to the focused pane's roster
>   cursor.
> - Typing a prompt in pane 0, then pane 3, runs them in their own
>   cwd (default to `--cwd` arg or process cwd for v1; dir search
>   wires up real switching in Phase 4).
> - `nit` (without multipane) opens the standard editor exactly as
>   before — multipane is purely additive.
>
> **Reading list**:
> - `docs/MULTIPANE.md` (this doc — full plan)
> - `docs/SWARM.md` (how `@swarm` is wired — multipane reuses the
>   per-pane agent dispatch model)
> - `crates/nit-tui/src/widgets/agent_console_view.rs::render`
>   (chat-pane render — to be split into a per-pane core)
> - `crates/nit-tui/src/app/runner.rs::run_loop` (current event loop
>   — multipane will branch off it)
> - `crates/nit-tui/src/codex_runner.rs` /
>   `crates/nit-tui/src/claude_runner.rs` (already accept per-turn
>   `cwd: PathBuf` — no changes needed there)
>
> Use `template=lab` for this mission (single-writer integrator,
> read-only proposers/reviewer/test) to keep the change set
> coherent and the workspace consistent.

---

### Phase 4 prompt — Dir search (SHIPPED)

> **Status**: shipped in three atomic stages — (2a) `dir_search.rs`
> pure parser + ranker, (2b) `dir_search_runner.rs` async walker
> wired into `run_loop` but unbound, (2c) Ctrl+/ key + 10-row
> dropdown + commit-cwd path. The runner mirrors
> `crate::fuzzy_search_runner::FuzzyMatcherRunner`'s shape (named
> `nit-multipane-dirsearch` worker, crossbeam channels,
> `Arc<AtomicU64>` request-id supersession) and reuses
> `fuzzy_score_bytes` for ranking instead of duplicating the
> algorithm.
>
> **Bindings (in focused pane chat mode):** Ctrl+/ toggles the
> overlay (F2 is documented as the fallback for terminals such as
> default macOS Terminal.app that drop Ctrl+/); typing narrows the
> query; Up/Down move selection; Enter commits — `pane.cwd`
> switches and a `cwd → /new/path` `SYSTEM_ALERT_KIND` message is
> pushed to the pane; Esc closes the overlay (a second Esc within
> ~500 ms still aborts the focused pane); Alt+f toggles the
> "show hidden" filter; Tab / Shift+Tab close the overlay and cycle
> pane focus.
>
> **Deferred to follow-up:** F5 cache-invalidate hook,
> `dir_index_cache` on `MultipaneState`, symlink-cycle protection
> via `(dev, ino)` visited set, stay-within-root symlink-escape
> policy, NFC/NFD Unicode normalisation. v1 walks fresh per
> keystroke (sub-ms on typical project trees) and caps the v1
> walk at depth 1; the spec target of 10k-entry/16 ms re-rank is
> unmet but acceptable for typical project trees (<1k entries).
> Track these in a follow-up mission rather than blocking the
> Phase 4 ship.

### Phase 4 historical brief — kept for reference

> **Goal**: Implement the per-pane dir search bar described in
> `docs/MULTIPANE.md` "Dir search modes". Phases 1-3 are already
> landed and `state.multipane` is wired end-to-end; this mission
> only adds the search bar, parser, async fuzzy runner, and cwd
> commit path.
>
> **Constraints**:
> - Stay additive: do not change any non-multipane code path.
> - **Search must feel instant** — keystroke → results inside one
>   16 ms frame on directory trees of 10k+ entries (cached after
>   the first walk per `(base, depth)`).
> - The walker runs on a worker thread, never on the TUI thread.
>   Use a bounded-size cache (`HashMap<PathBuf, Arc<Vec<DirEntry>>>`)
>   in `MultipaneState` to amortise repeated activations.
> - Path parsing rules (operator brief, codified in this doc):
>
>   | Input | Resolves to |
>   |---|---|
>   | `(empty)` | siblings of `pane.cwd` (same as `../`) |
>   | `foo` | subdirectories of `pane.cwd` matching `foo` |
>   | `../` | siblings of `pane.cwd` |
>   | `../foo` | siblings of `pane.cwd` matching `foo` |
>   | `../../` | siblings of parent |
>   | `../../foo` | siblings of parent matching `foo` |
>   | `/abs/path` | absolute path; descend from there |
>   | `~/foo` | expand `$HOME` then descend |
>
> - Fuzzy match algorithm: subsequence match with bonus for
>   consecutive characters and word-boundary hits. Reuse logic from
>   `crates/nit-tui/src/fuzzy_search_runner.rs` if practical;
>   otherwise inline a small ranker (≤ 60 lines). Hidden directories
>   (`.git`, `.cache`, `node_modules`, `target`, `.venv`, etc.) are
>   skipped by default — toggleable via `f`/`F2` in the search bar
>   (matching the editor's fuzzy-search popup convention).
>
> **Deliverables**:
> 1. `crates/nit-tui/src/multipane/dir_search.rs` (new) — pure
>    parser turning the operator's text into a `(base: PathBuf,
>    needle: &str)` pair. Cover edge cases: trailing slashes,
>    multiple `../`, `~`, `/abs`, empty input.
> 2. `crates/nit-tui/src/multipane/dir_search_runner.rs` (new) —
>    `DirSearchRunner` modelled on `fuzzy_search_runner`:
>    - `std::thread::Builder` worker, `mpsc` for requests,
>      `crossbeam` channel for results.
>    - Walks `read_dir` on the worker thread; caps depth at 4 for
>      bare bases, deeper when the user has explicitly drilled in.
>    - Fuzzy-ranks; returns top 50 candidates sorted by score.
>    - Cancellation by request id (newer keystroke supersedes the
>      older one — drop in-flight result if id mismatches).
> 3. UI integration in `crates/nit-tui/src/multipane/mod.rs`:
>    - A dedicated key (`Ctrl+/` proposed; confirm with the
>      operator) toggles dir search mode in the focused pane.
>    - When active, the chat thread dims and a 10-row dropdown
>      replaces the breather table directly under the search bar.
>    - `Up`/`Down` move the selection, `Enter` commits, `Esc`
>      cancels.
>    - Live narrowing on every keystroke (debounced if needed —
>      benchmark first).
>    - `f` toggles "show hidden dirs", `F5` re-walks the cache.
> 4. Commit path: when the operator hits `Enter` on a result,
>    `pane.cwd = chosen_path`, push a system message to the pane's
>    chat (`SYSTEM_ALERT_KIND` with body `cwd → /new/path`), and
>    invalidate the dir-index cache for the new base on first
>    query.
> 5. Tests:
>    - `dir_search::tests::parse_*` — every row in the table above.
>    - `dir_search::tests::fuzzy_rank_*` — assert ordering for
>      "foo" vs "fooooooo" vs "fxxoxxo" (consecutive bonus, word
>      boundary bonus, length penalty).
>    - `dir_search_runner::tests::cancel_supersedes_older_request`.
>    - `multipane::tests::commit_switches_cwd_and_notifies`.
> 6. Update `docs/MULTIPANE.md` "Open questions" section: mark
>    answered questions, leave any new ones surfaced during
>    implementation.
> 7. Update `docs/KEYBINDINGS.md` Multipane section with the dir
>    search bindings.
>
> **Verification**:
> - In a multipane launch, focus pane 0, hit `Ctrl+/`, type
>   `crat` — the dropdown shows `crates/` (highest match) plus
>   any other subdirectories that fuzzy-match.
> - Type `../` → siblings of cwd appear.
> - Type `../../` → siblings of parent appear.
> - Hit Enter on `crates/` → pane header shows
>   `cwd: /Users/me/code/nit/crates`, chat thread shows
>   `↳ [system] cwd → /Users/me/code/nit/crates`.
> - Walk a 10k-entry tree (e.g. point `--cwd $HOME` and search) —
>   first keystroke takes < 100 ms (cold walk), subsequent
>   keystrokes < 16 ms (cached).
> - Mistyped `../../../foo` on a shallow path resolves to whatever
>   the filesystem permits (or empty results), no crash.
>
> **Reading list**:
> - `docs/MULTIPANE.md` "Dir search modes" — the operator-facing
>   contract.
> - `crates/nit-tui/src/fuzzy_search_runner.rs` — the existing
>   pattern for an async fuzzy runner; mirror its structure.
> - `crates/nit-tui/src/multipane/` (whatever Phase 1-3 produced)
>   — wire the dropdown into the existing pane render.
>
> Use `template=lab` again — single-writer integrator keeps the
> dropdown rendering, parser, runner, and tests changes coherent.

---

### Phase 5 prompt — Disable non-pane keys + polish

> **Goal**: Lock multipane mode down to chat-only. Every key that
> would normally focus the editor / job output / file tree /
> visualizer / agent ops dock must become a no-op (or be absent
> entirely) when `state.multipane.is_some()`.
>
> **Constraints**:
> - Stay additive: don't break any standard-mode keybinding.
>   Multipane mode just gates the dispatch.
> - Cosmetic chrome that's meaningless in multipane (LAB / GOL /
>   MOOD / ECG indicators in the top bar; lab status badges; help
>   overlay sections about the editor) should hide cleanly,
>   not render as empty boxes.
> - The status line should reflect multipane mode and surface the
>   focused pane index + cwd, e.g.
>   `MULTIPANE  pane 3/8  cwd=/Users/me/code/nit/crates`.
>
> **Deliverables**:
> 1. `crates/nit-tui/src/multipane/key_dispatch.rs` (new) — top
>    of the global key handler short-circuits when
>    `state.multipane.is_some()`. Allowed keys: typing into chat,
>    Enter (submit), Tab/Shift+Tab (focus), mouse click,
>    Ctrl+C/Esc-Esc (abort), Ctrl+Q (quit, with confirm), the dir
>    search bindings from Phase 4. Everything else: no-op.
> 2. Top-bar variant: hide LAB/GOL/MOOD/ECG/HB/AG indicators when
>    in multipane; replace with the pane index + cwd line above.
>    Keep `STATUS:` so abort messages and clamp warnings still
>    surface.
> 3. Help overlay (`F1` / `?`) — show a multipane-specific page
>    rather than the editor key list. The page should mirror the
>    Multipane section of `docs/KEYBINDINGS.md` exactly.
> 4. Window-too-small fallback: when the terminal can't fit the
>    grid (e.g. < 20 cols × 10 rows per pane), render a single
>    centered message `Terminal too small for N panes — resize or
>    relaunch with --panes M` instead of letting ratatui draw
>    broken borders.
> 5. Tests:
>    - `multipane::key_dispatch::tests::editor_keys_are_noop`.
>    - `multipane::key_dispatch::tests::abort_keys_still_work`.
>    - `multipane::tests::status_line_shows_pane_and_cwd`.
>    - `multipane::tests::too_small_terminal_renders_fallback`.
>
> **Verification**:
> - Press `Ctrl+T` (file tree toggle) in multipane — nothing
>   happens, focus stays on the chat pane.
> - Press `Ctrl+1`/`Ctrl+2`/`Ctrl+3` (pane focus shortcuts) —
>   no-op.
> - Press `:` (command prompt) — no-op.
> - Press `F1` — multipane-specific help page renders, no editor
>   keys listed.
> - Resize the terminal to 30 × 10 with 8 panes requested — the
>   "terminal too small" message renders cleanly.
>
> **Reading list**:
> - `crates/nit-tui/src/app/key_dispatch.rs` — the standard
>   global key handler. Add the multipane gate at the top.
> - `crates/nit-tui/src/widgets/top_bar.rs` — top bar render.
> - `crates/nit-tui/src/widgets/help_overlay.rs` — help overlay
>   rendering.
> - `docs/KEYBINDINGS.md` Multipane section — keep the help
>   overlay copy in lockstep with this.
>
> Use `template=parallel` here. Five small disjoint targets
> (key gate, top bar, help overlay, fallback, tests) parallelise
> well and don't share much surface area.

---

### Phase 6 prompt — Persistence + integration tests + docs polish

> **Goal**: Final polish pass before declaring multipane mode
> shippable. Three deliverables: per-pane state persistence
> across nit restarts, end-to-end integration tests with mock
> runners, and full documentation lockdown.
>
> **Constraints**:
> - Persistence is best-effort. A corrupt state file should never
>   block nit from launching — silently fall back to a fresh
>   layout and log a one-line warning to vitals.
> - Persistence file lives at
>   `<state_dir>/multipane/session-<workspace-hash>.json`. Hash
>   the absolute path of `--cwd` (or process cwd) so each
>   workspace gets its own session file.
> - Integration tests use mock runners (already exist for swarm
>   tests; extend them) — never spawn real Codex/Claude
>   subprocesses.
>
> **Deliverables**:
> 1. `crates/nit-tui/src/multipane/persistence.rs` (new):
>    - `save_session(state: &MultipaneState, workspace: &Path)`
>      writes a small JSON document with: pane count, focused
>      index, per-pane `cwd`, per-pane `agent_id`, per-pane
>      committed selection (or roster cursor / expansion), per-pane
>      chat input draft (capped at 4 KB so a runaway draft
>      doesn't bloat the file).
>    - `load_session(workspace: &Path) -> Option<MultipaneState>`
>      reads, validates, and falls back to defaults on error.
>    - Save fires on focused-pane changes, cwd switches, and at
>      shutdown. Debounced to ≤ 1 write/second.
>    - Drop the file when the operator quits with `Ctrl+Q` from a
>      "fresh" mode (i.e. no committed work) — operator surprise
>      avoidance.
> 2. Integration tests in
>    `crates/nit-tui/src/tests/multipane_integration.rs` (new):
>    - `four_panes_each_dispatch_to_their_own_cwd` — mock both
>      runners, assert `RunTurn { cwd, .. }` for each pane has
>      the expected cwd.
>    - `abort_in_focused_pane_only_kills_that_pane` — start a
>      mission in panes 0, 1, 2; abort pane 1; assert panes 0 and
>      2 are still active.
>    - `roster_mode_dispatch_emits_no_agent_notice` — operator
>      hits Enter in a pane that hasn't committed a selection;
>      expect the "no agent selected" notice instead of a
>      crash or silent drop.
>    - `dir_search_commit_changes_runtime_cwd` — drive the search
>      runner with a temp directory tree; assert the next
>      dispatch lands at the new cwd.
>    - `persistence_roundtrip` — save, reload, assert the
>      `MultipaneState` matches.
> 3. Doc lockdown:
>    - `docs/MULTIPANE.md` — strike the "Status: design proposal"
>      header, replace with "Status: shipped in <version>". Move
>      "Open questions" into a "Resolved decisions" appendix.
>    - `docs/KEYBINDINGS.md` Multipane section — full table of
>      every multipane binding (chat mode + roster mode + dir
>      search), final and authoritative.
>    - `CLAUDE.md` Multipane section — confirm the agent-id
>      separator, the `--backend` family/specific resolution
>      rules, and the persistence file path.
>    - `docs/ARCHITECTURE.md` — short subsection (≤ 30 lines) on
>      multipane: where state lives, how the runner is shared,
>      the additive design.
> 4. Vitals: add a small "multipane" indicator to the bottom
>    status line so operators can see at a glance which mode
>    they're in. Skip if Phase 5 already did this.
>
> **Verification**:
> - `cargo test --workspace` shows the new integration tests
>   passing alongside everything else.
> - Launch nit multipane, switch a pane's cwd, quit. Relaunch
>   with the same `--cwd` — the pane is back at its previous cwd.
> - Corrupt the JSON file by hand; relaunch — nit starts cleanly
>   with a fresh layout and a vitals warning.
> - All four docs (MULTIPANE / KEYBINDINGS / CLAUDE /
>   ARCHITECTURE) are consistent with each other and with the
>   actual code.
>
> **Reading list**:
> - `docs/MULTIPANE.md` — the source of truth, due for its
>   final lockdown pass.
> - Persistence patterns elsewhere in nit (search for
>   `nit_state_dir` and `serde_json::to_writer`) — match the
>   existing convention.
> - `crates/nit-tui/src/tests/swarm.rs` — pattern to copy for
>   integration tests with mock runners.
>
> Use `template=lab` for the persistence + tests. Use a
> separate `template=parallel` mission for the docs (they
> partition by file naturally).

---

### Phase ordering and parallelism

The phases form a soft dependency chain:

- **Phase 4** is independent of Phase 5 — they touch different
  parts of the input / dispatch path. Run them in parallel if
  you have the budget.
- **Phase 6** depends on both 4 and 5 (the integration tests cover
  dir-search commits and the locked-down keymap). Sequence it
  last.

Recommended order:

```
Phase 4 ──┐
          ├─→ Phase 6
Phase 5 ──┘
```

Two simultaneous swarms (one per phase) hit the bottleneck of the
shared per-pane state struct, but the touches are mostly disjoint:
Phase 4 adds fields to `PaneSession`/`DirSearchState`, Phase 5
gates the global key handler. Conflicts will be limited to
`MultipaneState` field additions, which the integrator merges
trivially.
